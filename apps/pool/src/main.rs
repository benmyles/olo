//! olo-pool (Token Pool): several Claude and ChatGPT subscriptions
//! shared by Claude Code and Codex on this computer. A gateway on
//! loopback (`olo_token_pool::gateway`) takes the clients' requests
//! and sends each with whichever account's quota resets soonest, so less
//! of it goes unused; a sign-in the provider refuses is refreshed, an
//! account out of quota waits for its reset while the next serves.
//!
//! The session runs it in the background (`--service`); the window shows
//! the gateway, which clients use it, and each account's quota, usage
//! and state, live, and adds accounts: this computer's own sign-in, or a
//! separate one made in Terminal by the client's own sign-in. Sign-ins
//! are kept sealed in `pool.vault`. `--status` (`--json`) tells agents
//! the same, without the sign-ins.

use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use olo_kit::{
    App, Cx, Menu, Proxy, WindowId, WindowOptions,
    egui::{self, Key},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, BBox, Button, CellGrid, CheckBox, GridCell, ScrollView, StatusBar, StringView, TextControl},
};
use olo_token_pool::{
    clients,
    codex_daemon::{self, Followed},
    gateway::{self, Pool, Shared},
    import::{self, Found},
    model::{Account, Outcome, Provider, RECENT, Request, Window, now},
    schedule, store,
};

const APP_ID: &str = "olo.pool";
const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];
/// How long a separate sign-in in Terminal is waited for.
const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);

enum Msg {
    /// The pool changed (a request, a refresh, quota read).
    Changed,
    /// Which clients use the pool.
    Clients([bool; 2]),
    /// A client was pointed at the pool or back (and, for Codex, whether
    /// its daemon is on the new settings yet).
    ClientSet(Provider, Result<(), String>, bool),
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

struct PoolApp {
    shared: Arc<Shared>,
    /// The clients' proxy's port (`intercept`).
    proxy_port: u16,
    proxy: Proxy,
    win: Option<WindowId>,
    /// Which clients use the pool (None: not read yet).
    clients: Option<[bool; 2]>,
    /// The last thing done or gone wrong (error?).
    note: Option<(String, bool)>,
    waiting: Option<Waiting>,
    /// (account, the name being typed).
    renaming: Option<(String, String)>,
    /// The account whose removal is being confirmed.
    removing: Option<String>,
    /// The account a reset credit is being spent for (confirming).
    resetting: Option<String>,
    /// The request whose details are shown.
    picked: Option<Pick>,
}

/// A request picked in the heatmap.
#[derive(Clone, PartialEq)]
enum Pick {
    /// Going on now (its id).
    Open(u64),
    /// One of an account's finished requests.
    Done(String, Request),
}

fn client_index(p: Provider) -> usize {
    PROVIDERS.iter().position(|&q| q == p).unwrap_or(0)
}

/// "14:05", or "Tue 14:05" beyond today.
fn clock(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    if at.date_naive() == Local::now().date_naive() { at.format("%H:%M").to_string() } else { at.format("%a %H:%M").to_string() }
}

/// "2h 10m", "3d 4h", "45s".
fn span(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// "340k", "1.2M".
fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn plan_name(plan: &str) -> String {
    let mut c = plan.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
}

impl PoolApp {
    fn open(&mut self, cx: &mut Cx) {
        if let Some(id) = self.win {
            cx.activate(id);
            return;
        }
        self.win = Some(cx.open_window(WindowOptions::new("Token Pool", 460, 560).min_size(380, 320)));
        self.read_clients();
    }

    /// Read which clients use the pool (their settings files) off the UI
    /// thread.
    fn read_clients(&self) {
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || proxy.send(Msg::Clients(PROVIDERS.map(|p| clients::enabled(p, port)))));
    }

    fn set_client(&mut self, p: Provider, on: bool) {
        if let Some(c) = self.clients.as_mut() {
            c[client_index(p)] = on;
        }
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || {
            let r = clients::set(p, on, port);
            // Codex's daemon, between turns, restarted onto them now.
            let waiting = p == Provider::Codex && r.is_ok() && matches!(codex_daemon::follow(), Ok(Followed::Waiting));
            proxy.send(Msg::ClientSet(p, r, waiting))
        });
    }

    /// Bring in this computer's sign-in for `p` (read off the UI thread).
    fn import_local(&mut self, p: Provider) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::SignedIn(import::this_computer(p), None)));
    }

    /// Sign `p`'s client in to an account in Terminal, apart from this
    /// computer's own sign-in, and take it when it's done.
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
        if let Err(e) = olo_kit::open::spawn(&argv) {
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

    /// Take a sign-in that was read: a new account, or a new sign-in for
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

    fn command_for(&mut self, cx: &mut Cx, command: &str) {
        match command {
            "refresh" => self.shared.sync_soon(None),
            "close" => {
                if let Some(w) = self.win.take() {
                    cx.close_window(w);
                }
            }
            _ => self.add_command(command),
        }
    }

    fn add_menu(&self) -> Menu {
        let mut menu = Menu::new("Accounts");
        for (i, p) in PROVIDERS.into_iter().enumerate() {
            if i > 0 {
                menu = menu.separator();
            }
            let key = format!("{p:?}").to_lowercase();
            menu = menu
                .item(format!("Add This Computer’s {} Sign-In", p.client()), format!("import:{key}"), None)
                .item(format!("Sign In to {} Separately…", p.client()), format!("separate:{key}"), None);
        }
        menu
    }

    fn account_command(&mut self, id: &str, command: &str) {
        let Some(a) = self.shared.snapshot().account(id).cloned() else { return };
        match command {
            "rename" => self.renaming = Some((a.id, a.name)),
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
    }

    fn header(&mut self, ui: &mut egui::Ui, pool: &Pool) {
        let p = style::palette(ui.ctx());
        let busy: u32 = pool.in_flight.values().sum();
        match &pool.error {
            Some(e) => {
                ui.add(StringView::new(e).wrap(true).color(c(p.failure)));
            }
            None => {
                let line = match busy {
                    0 => format!("Listening on 127.0.0.1:{}", self.proxy_port),
                    n => format!("Listening on 127.0.0.1:{} · {n} request{} now", self.proxy_port, if n == 1 { "" } else { "s" }),
                };
                ui.add(StringView::new(&line));
            }
        }
        self.now_grid(ui, pool);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING * 2.0;
            for prov in PROVIDERS {
                let i = client_index(prov);
                let mut on = self.clients.is_some_and(|c| c[i]);
                let label = format!("{} uses the pool", prov.client());
                let r = ui.add_enabled(self.clients.is_some(), CheckBox::new(&mut on, &label));
                if r.changed() {
                    self.set_client(prov, on);
                }
            }
        });
    }

    /// The requests going through now: a cell each, lit while it runs.
    fn now_grid(&mut self, ui: &mut egui::Ui, pool: &Pool) {
        let p = style::palette(ui.ctx());
        let t = now_ms();
        let cells: Vec<GridCell> = pool
            .open
            .iter()
            .map(|o| GridCell {
                color: Some(c(p.status_bar)),
                mark: Some(mark(o.provider).into()),
                name: format!("{} on {}, {}", o.model, o.account, duration((t - o.started).max(0) as u64)),
                selected: self.picked == Some(Pick::Open(o.id)),
            })
            .collect();
        let r = CellGrid::new("pool-now", &cells).rows(2).cell_size(14.0).fill().show(ui);
        if let Some(i) = r.clicked {
            let pick = Pick::Open(pool.open[i].id);
            self.picked = if self.picked.as_ref() == Some(&pick) { None } else { Some(pick) };
        }
        // One line, always there (so nothing jumps): the picked request.
        let details = match &self.picked {
            Some(Pick::Open(id)) => match pool.open.iter().find(|o| o.id == *id) {
                Some(o) => Some(format!("Running · {} on {} · {} · {} so far", o.model, o.account, o.client, duration((t - o.started).max(0) as u64))),
                // It finished: its account's strip has it now.
                None => {
                    self.picked = None;
                    None
                }
            },
            Some(Pick::Done(id, r)) => Some(done_details(&pool.account(id).map(|a| a.name.clone()).unwrap_or_default(), r)),
            None => None,
        };
        let text = details.unwrap_or_else(|| match pool.open.len() {
            0 => "No requests now · click a cell for its details".into(),
            _ => "Click a cell for its details".into(),
        });
        ui.add(StringView::new(&text).dim(true));
    }

    /// An account's latest requests, oldest first, coloured by how they
    /// ended.
    fn strip(&mut self, ui: &mut egui::Ui, a: &Account) {
        let p = style::palette(ui.ctx());
        let cells: Vec<GridCell> = a
            .recent
            .iter()
            .map(|r| GridCell {
                color: Some(outcome_color(&p, r.outcome)),
                mark: None,
                name: format!("{}: {} at {}", r.outcome.label(), r.model, clock(r.at)),
                selected: self.picked == Some(Pick::Done(a.id.clone(), r.clone())),
            })
            .collect();
        let r = CellGrid::new(("pool-recent", &a.id), &cells).cell_size(12.0).slots(RECENT).show(ui);
        if let Some(i) = r.clicked {
            let pick = Pick::Done(a.id.clone(), a.recent[i].clone());
            self.picked = if self.picked.as_ref() == Some(&pick) { None } else { Some(pick) };
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, pool: &Pool, a: &Account) {
        let p = style::palette(ui.ctx());
        let t = now();
        let title = if a.signed_out.is_some() {
            format!("{} — Signed Out", a.name)
        } else if !a.enabled {
            format!("{} — Paused", a.name)
        } else {
            a.name.clone()
        };
        BBox::new(&title).show(ui, |ui| {
            let mut facts: Vec<String> = Vec::new();
            if let Some(e) = &a.email {
                facts.push(e.clone());
            }
            if let Some(plan) = &a.plan {
                facts.push(format!("{} plan", plan_name(plan)));
            }
            let credits: u32 = a.credits.iter().map(|c| c.remaining).sum();
            if credits > 0 {
                facts.push(format!("{credits} reset credit{}", if credits == 1 { "" } else { "s" }));
            }
            if pool.busy(&a.id) {
                facts.push("answering now".into());
            } else if let Some(l) = a.last_used {
                facts.push(format!("used {} ago", span(t - l)));
            }
            if !facts.is_empty() {
                ui.add(StringView::new(&facts.join(" · ")).dim(true));
            }
            if let Some(why) = &a.signed_out {
                ui.add(StringView::new(&format!("Needs signing in again: {why}")).wrap(true).color(c(p.failure)));
            } else if let Some(until) = schedule::blocked_until(a, "", t) {
                ui.add(StringView::new(&format!("Out of quota until {} (in {})", clock(until), span(until - t))));
            }
            for w in &a.windows {
                window_bar(ui, w, t);
            }
            if a.windows.is_empty() && a.signed_out.is_none() {
                ui.add(StringView::new(if a.synced.is_some() { "No quota reported." } else { "Reading its quota…" }).dim(true));
            }
            let today = if a.today.0 == olo_token_pool::model::today() { a.today.1.clone() } else { Default::default() };
            let used = format!(
                "Today: {} request{} · {} in · {} out",
                today.requests,
                if today.requests == 1 { "" } else { "s" },
                tokens(today.input_tokens),
                tokens(today.output_tokens)
            );
            ui.add(StringView::new(&used));
            self.strip(ui, a);
            self.card_controls(ui, a);
        });
        ui.add_space(m::SPACING);
    }

    fn card_controls(&mut self, ui: &mut egui::Ui, a: &Account) {
        if let Some((id, text)) = self.renaming.as_mut().filter(|(id, _)| *id == a.id) {
            let (id, mut done) = (id.clone(), None);
            ui.horizontal(|ui| {
                let r = ui.add(TextControl::new(text).autofocus(true).width(180.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                widgets::button_row(ui, |ui| {
                    if ui.add(Button::new("Rename").default(true)).clicked() || enter {
                        done = Some(true);
                    }
                    if ui.add(Button::new("Cancel")).clicked() {
                        done = Some(false);
                    }
                });
            });
            match done {
                Some(true) => {
                    let name = text.trim().to_owned();
                    if !name.is_empty() {
                        self.shared.rename(&id, &name);
                    }
                    self.renaming = None;
                }
                Some(false) => self.renaming = None,
                None => {}
            }
            return;
        }
        if self.resetting.as_deref() == Some(a.id.as_str()) {
            let when = a.credit().and_then(|c| c.expires_at).map(|e| format!(" (it expires {})", clock(e))).unwrap_or_default();
            ui.add(StringView::new(&format!("Use one of its reset credits now{when}? Its quota starts again, and the credit is gone.")).wrap(true));
            row(ui, |ui| {
                if ui.add(Button::new("Use Credit")).clicked() {
                    let (shared, proxy, id, name) = (self.shared.clone(), self.proxy.clone(), a.id.clone(), a.name.clone());
                    self.shared.spawn(async move {
                        let r = shared.redeem(&id).await.map(|()| format!("{name}’s quota was reset."));
                        proxy.send(Msg::Done(r));
                    });
                    self.resetting = None;
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    self.resetting = None;
                }
            });
            return;
        }
        if self.removing.as_deref() == Some(a.id.as_str()) {
            ui.add(StringView::new("Remove it from the pool? Its sign-in here is deleted; the account itself isn’t touched.").wrap(true));
            row(ui, |ui| {
                if ui.add(Button::new("Remove")).clicked() {
                    self.note = Some(match self.shared.remove(&a.id) {
                        Ok(()) => (format!("{} left the pool.", a.name), false),
                        Err(e) => (e, true),
                    });
                    self.removing = None;
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    self.removing = None;
                }
            });
            return;
        }
        ui.horizontal(|ui| {
            let mut on = a.enabled;
            if ui.add(CheckBox::new(&mut on, "Use this account")).changed() {
                self.shared.set_enabled(&a.id, on);
            }
            widgets::button_row(ui, |ui| {
                let r = ui.add(Button::new("Account").min_width(m::BUTTON_MIN_WIDTH));
                let source = r.id.with("account-menu");
                if r.clicked() {
                    let menu = Menu::new("Account")
                        .item("Rename…", "rename", None)
                        .item("Read Quota Now", "refresh", None)
                        .action(olo_kit::Action::new("Use a Reset Credit…", "reset").enabled(a.credit().is_some()))
                        .separator()
                        .item("Sign In Again in Terminal…", "sign-in", None)
                        .item(format!("Use This Computer’s {} Sign-In", a.provider.client()), "sign-in-local", None)
                        .separator()
                        .item("Remove from Pool…", "remove", None);
                    widgets::popup_menu_at(ui.ctx(), source, r.rect.left_bottom(), menu);
                }
                if let Some(cmd) = widgets::popup_result(ui.ctx(), source) {
                    self.account_command(&a.id, &cmd);
                }
            });
        });
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
fn outcome_color(p: &olo_kit::theme::Palette, o: Outcome) -> egui::Color32 {
    match o {
        Outcome::Completed => c(p.success),
        Outcome::RateLimited => c(p.tab_focused),
        Outcome::Failed => c(p.failure),
        Outcome::Cancelled => style::disabled_text(p),
    }
}

/// "850ms", "3.2s", "2m 5s".
fn duration(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms}ms"),
        1000..60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        _ => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1000),
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

/// A row of buttons at the right, one control tall (in a card, where
/// `button_row` alone would centre them in all the height left).
fn row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let size = egui::vec2(ui.available_width(), m::CONTROL_HEIGHT);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING + 2.0;
        add(ui)
    });
}

/// One quota window: how much is used, and when it resets.
fn window_bar(ui: &mut egui::Ui, w: &Window, t: i64) {
    let p = style::palette(ui.ctx());
    let mut trailing = format!("{:.0}% used", w.used.clamp(0.0, 999.0));
    if let Some(r) = w.resets_at.filter(|&r| r > t) {
        trailing.push_str(&format!(" · resets {} ({})", clock(r), span(r - t)));
    }
    let label = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
    let mut bar = StatusBar::new((w.used / 100.0) as f32).label(&label).trailing(&trailing).bar_height(10.0);
    if w.exhausted(t) {
        bar = bar.color(c(p.failure));
    } else if w.used >= 80.0 {
        bar = bar.color(c(p.keyboard_navigation));
    }
    ui.add(bar);
}

impl App for PoolApp {
    fn ui(&mut self, _cx: &mut Cx, _window: WindowId, ui: &mut egui::Ui) {
        let pool = self.shared.snapshot();
        // Times count down (running requests' by the second).
        ui.ctx().request_repaint_after(Duration::from_secs(if pool.open.is_empty() { 20 } else { 1 }));
        let p = style::palette(ui.ctx());
        widgets::window_content(ui, |ui| {
            self.header(ui, &pool);
            widgets::separator(ui);
            // The bottom: what's happening, and adding accounts.
            let gap = ui.spacing().item_spacing.y;
            let bottom_h = 6.0 + m::CONTROL_HEIGHT + 2.0 * gap;
            let list_h = (ui.available_height() - bottom_h).max(60.0);
            let list = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), list_h));
            ScrollView::new("pool-accounts").border(false).show_in(ui, list, |ui, _| {
                let mut any = false;
                for prov in PROVIDERS {
                    let accounts: Vec<&Account> = pool.saved.accounts.iter().filter(|a| a.provider == prov).collect();
                    if accounts.is_empty() {
                        continue;
                    }
                    any = true;
                    widgets::heading(ui, prov.client());
                    ui.add_space(m::TIGHT);
                    for a in accounts {
                        self.card(ui, &pool, a);
                    }
                }
                if !any {
                    let text = if pool.loaded {
                        "No accounts yet. Add your subscriptions with Add Account: this computer’s own sign-in, or others signed in separately in Terminal. Then check the clients that should use the pool."
                    } else {
                        "Opening the pool’s accounts…"
                    };
                    ui.add(StringView::new(text).wrap(true).dim(true));
                }
            });
            ui.advance_cursor_after_rect(list);
            widgets::separator(ui);
            let status = match (&self.waiting, &self.note) {
                (Some(w), _) => {
                    let whose = w.target.as_deref().and_then(|id| pool.account(id)).map(|a| format!("{}’s", a.name)).unwrap_or_else(|| format!("the {}", w.provider.client()));
                    Some((format!("Waiting for {whose} sign-in in Terminal…"), false))
                }
                (None, Some(n)) => Some(n.clone()),
                (None, None) => pool.served.back().map(|s| {
                    let (r, u) = (&s.request, &s.request.usage);
                    (format!("Last: {} via {} at {} · {} in, {} out", r.model, s.account, clock(r.at), tokens(u.input_tokens), tokens(u.output_tokens)), false)
                }),
            };
            ui.horizontal(|ui| {
                let mut add = None;
                widgets::button_row(ui, |ui| {
                    let r = ui.add(Button::new("Add Account"));
                    let source = r.id.with("add-menu");
                    if r.clicked() {
                        widgets::popup_menu_at(ui.ctx(), source, r.rect.left_bottom(), self.add_menu());
                    }
                    add = widgets::popup_result(ui.ctx(), source);
                    if let Some(w) = &self.waiting
                        && ui.add(Button::new("Cancel Sign-In")).clicked()
                    {
                        w.cancel.store(true, Ordering::Relaxed);
                        self.waiting = None;
                    }
                    if let Some((text, error)) = &status {
                        let color = if *error { c(p.failure) } else { style::disabled_text(&p) };
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(StringView::new(text).color(color));
                        });
                    }
                });
                if let Some(cmd) = add {
                    self.add_command(&cmd);
                }
            });
        });
    }

    fn menu_bar(&mut self, _window: WindowId) -> Option<Vec<Menu>> {
        Some(vec![
            Menu::new("Pool").item("Read Quota Now", "refresh", Some(Key::R)).separator().item("Close", "close", Some(Key::W)),
            self.add_menu(),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        self.command_for(cx, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        // The gateway keeps serving.
        cx.close_window(window);
        self.win = None;
        self.renaming = None;
        self.removing = None;
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<Launch>() {
            if !launch.args.iter().any(|a| a == "--service") {
                self.open(cx);
            }
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {}
            Msg::Clients(on) => self.clients = Some(on),
            Msg::ClientSet(prov, r, waiting) => {
                self.note = Some(match r {
                    Ok(()) => {
                        let on = self.clients.is_some_and(|c| c[client_index(prov)]);
                        // Codex's sessions run in its daemon, restarted onto
                        // the settings between turns.
                        let when = match prov {
                            Provider::Codex if waiting => "once it finishes the turn it’s on",
                            Provider::Codex => "from its next turn",
                            Provider::Claude => "from its next start",
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
        if let Some(w) = self.win {
            cx.redraw(w);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        Some(json!({"open": self.win.is_some()}).to_string())
    }
}

impl PoolApp {
    fn add_command(&mut self, cmd: &str) {
        let provider = cmd.rsplit(':').next().and_then(Provider::parse);
        match (cmd.split(':').next(), provider) {
            (Some("import"), Some(p)) => self.import_local(p),
            (Some("separate"), Some(p)) => self.sign_in_separately(p, None),
            _ => {}
        }
    }
}

// ---- the command line -------------------------------------------------

const USAGE: &str = "usage: olo-pool [--service | --status [--json]]
  (none)            open Token Pool (starting the gateway if it isn't running)
  --service         run the gateway in the background (the session does)
  --status [--json] the pool's accounts, quota and state (no sign-ins)";

/// The running gateway's status, over loopback.
fn fetch_status(port: u16) -> Result<Value, String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|_| "Token Pool isn’t running.".to_owned())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    write!(s, "GET /pool/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    serde_json::from_str(body).map_err(|_| "Token Pool answered in a way this version doesn’t understand.".to_owned())
}

fn status(port: u16, as_json: bool) -> std::process::ExitCode {
    let v = match fetch_status(port) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("olo-pool: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(e) = v["error"].as_str() {
        println!("! {e}");
    }
    let t = now();
    let accounts = v["accounts"].as_array().cloned().unwrap_or_default();
    if accounts.is_empty() {
        println!("No accounts in the pool.");
    }
    for a in accounts {
        let state = if let Some(why) = a["signed_out"].as_str() {
            format!("signed out ({why})")
        } else if a["enabled"] == false {
            "paused".into()
        } else if let Some(u) = a["blocked_until"].as_i64() {
            format!("out of quota until {}", clock(u))
        } else {
            "ready".into()
        };
        let credits: u64 = a["reset_credits"].as_array().into_iter().flatten().filter_map(|c| c["remaining"].as_u64()).sum();
        let state = if credits > 0 { format!("{state}, {credits} reset credit{}", if credits == 1 { "" } else { "s" }) } else { state };
        println!("{} [{}] {}", a["name"].as_str().unwrap_or(""), a["provider"].as_str().unwrap_or(""), state);
        for w in a["windows"].as_array().into_iter().flatten() {
            let reset = w["resets_at"].as_i64().filter(|&r| r > t).map(|r| format!(", resets {} ({})", clock(r), span(r - t))).unwrap_or_default();
            println!("  {}: {:.0}% used{reset}", w["label"].as_str().unwrap_or(""), w["used"].as_f64().unwrap_or(0.0));
        }
        let d = &a["today"];
        println!(
            "  today: {} request{}, {} in, {} out",
            d["requests"].as_u64().unwrap_or(0),
            if d["requests"] == 1 { "" } else { "s" },
            tokens(d["input_tokens"].as_u64().unwrap_or(0)),
            tokens(d["output_tokens"].as_u64().unwrap_or(0))
        );
    }
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = gateway::port();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("-h") || has("--help") {
        println!("{USAGE}");
        return std::process::ExitCode::SUCCESS;
    }
    if has("--status") {
        return status(port, has("--json"));
    }
    let service = has("--service");
    // One gateway per computer: a test or nested session runs its own
    // only on a port of its own.
    if service && std::env::var_os("OLO_REAL_SESSION").is_none() && std::env::var_os("OLO_POOL_PORT").is_none() {
        eprintln!("olo-pool: --service is for the real session (set OLO_POOL_PORT to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, move |cx| {
        cx.stay_resident();
        let proxy = cx.proxy();
        instance::listen(APP_ID, proxy.clone());
        let to_ui = proxy.clone();
        let shared = Shared::launch(port, move || to_ui.send(Msg::Changed));
        // Clients pointed at the pool the old way (an API endpoint of its
        // own) go through its proxy instead, as on their own subscription.
        let proxy_port = olo_token_pool::intercept::port();
        std::thread::spawn(move || clients::migrate(proxy_port));
        // Codex's daemon, which outlives its windows, kept on the settings.
        codex_daemon::keep_following(Duration::from_secs(10));
        let restored: Option<Value> = cx.restored_state().and_then(|s| serde_json::from_str(&s).ok());
        let mut app = PoolApp {
            shared,
            proxy_port,
            proxy,
            win: None,
            clients: None,
            note: None,
            waiting: None,
            renaming: None,
            removing: None,
            resetting: None,
            picked: None,
        };
        let open = match &restored {
            Some(r) => r["open"] == true,
            None => !service,
        };
        if open {
            app.open(cx);
        }
        app
    });
    std::process::ExitCode::SUCCESS
}
