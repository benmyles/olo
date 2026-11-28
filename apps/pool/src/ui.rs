//! Token Pool's unified single-window UI (`Accounts`, `Activity`, `Machines`).
//!
//! All three tabs follow one cohesive BeOS framed-list architecture:
//! - a fixed top header / alert bar;
//! - **one** sunken framed list well (`border(true)`, flush scrollbar inside);
//! - a fixed bottom action bar matching the menu bar's commands.
//!
//! Nothing scrolls as an unframed web page; every local vs remote Rex host
//! relationship is shown with native icons (`system`, `network`, `warning`),
//! consistent provider colour encoding (Claude Code = orange, Codex = blue),
//! and full tiling adaptability (including two-column horizontal-third Activity).

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    AppCx, Bounds, Child, ClickEvent, Corner, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, Menu, MenuBar, MouseButton, ParentElement as _, Pixels, Proxy, ScrollHandle,
    SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div, icons, menu, metrics as m, palette as olo_theme, point, px, size, style, theme,
    widgets::{self, Button, CheckBox, Graph, LIGHT, Light, MenuField, ScrollView, Series, StringView, TabView, TextControl, TextField, TextFieldEvent, button_width, level_bar, light, text_field},
};
use olo_token_pool::{
    activity::Activity,
    clients,
    codex_daemon::{self, Followed},
    gateway::{Pool, Shared},
    hosts::{Hosts, Stands},
    import::{self, Found},
    model::{Account, Outcome, Provider, Window as QuotaWindow, now},
    running::{Process, Running},
    store,
};

use crate::{
    PROVIDERS,
    aggregate::{
        AccountRollup, LiveReq, MachineScope, RecentReq, Tab, account_out_of_quota, account_paid_scope, account_rollup, custody, demo_data, demo_enabled, folder, host_brief_sentence, host_stands,
        host_summary, hue, kept_names, last_checked_at, last_synced_at, live_requests, local_custody_summary, merged_activity, middle_truncate, provider_hue, recent_requests, sentence,
        sorted_accounts, sorted_hosts, sorted_providers, tab_lights,
    },
    client_index, clock, duration, plan_name, span, tokens,
};

const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);
const PROCESS_POLL_EVERY: Duration = Duration::from_secs(2);
const HOSTS_POLL_EVERY: Duration = Duration::from_secs(30);
const GRAPH_HEIGHT: f32 = 152.0;
const DETAIL_ROW: f32 = 16.0;
/// An account's or a host's row: the room above and below its words.
/// (Eight points, and the three on each side of the rule between rows
/// that a separator of the kit's had: the rule is the row's own now,
/// [`under_rule`].)
const ROW_PAD: f32 = 11.0;
const RECENT_TABLE_LIMIT: usize = 60;

/// Time spans for the Activity chart: (label, minutes, bucket minutes, grid seconds).
const SPANS: [(&str, i64, i64, f64); 3] = [("Last Hour", 60, 1, 600.0), ("Last 6 Hours", 360, 5, 3600.0), ("Last 24 Hours", 1440, 15, 4.0 * 3600.0)];

/// Messages to [`Hub`].
pub enum Msg {
    Changed,
    Clients([bool; 3]),
    ClientSet(Provider, bool, Result<(), String>, Option<bool>),
    SignedIn(Result<Found, String>, Option<String>),
    Done(Result<String, String>),
    Open,
    Command(String),
    SetClient(Provider, bool),
    SignIn(Provider, String, bool),
    CancelSignIn,
    TabChanged(Tab),
    Closed,
    CloseWindows,
}

struct Waiting {
    provider: Provider,
    target: Option<String>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Default, PartialEq)]
pub struct Told {
    clients: Option<[bool; 3]>,
    note: Option<(String, bool)>,
    waiting: Option<(Provider, Option<String>)>,
}

#[derive(Default)]
pub struct Open {
    open: AtomicBool,
    tab: AtomicUsize,
}

impl Open {
    fn state(&self) -> String {
        serde_json::json!({
            "open": self.open.load(Ordering::Relaxed),
            "tab": self.tab.load(Ordering::Relaxed),
        })
        .to_string()
    }
}

pub struct Hub {
    pub shared: Arc<Shared>,
    proxy_port: u16,
    proxy: Proxy,
    win: Option<WindowHandle<PoolWindow>>,
    hosts: Arc<Hosts>,
    clients: Option<[bool; 3]>,
    client_tx: std::sync::mpsc::Sender<(Provider, bool)>,
    note: Option<(String, bool)>,
    waiting: Option<Waiting>,
    open: Arc<Open>,
}

impl Hub {
    pub fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, proxy_port: u16, proxy: Proxy) -> Hub {
        // Single serialized worker for client toggles so rapid clicks execute
        // strictly in click order and Antigravity enable gates on proxy CA readiness.
        let (client_tx, client_rx) = std::sync::mpsc::channel::<(Provider, bool)>();
        let worker_shared = shared.clone();
        let worker_proxy = proxy.clone();
        let _ = std::thread::Builder::new().name("pool-clients".into()).spawn(move || {
            while let Ok((p, on)) = client_rx.recv() {
                if p == Provider::Antigravity && on {
                    let ready = worker_shared.wait_proxy_ready(Duration::from_secs(5));
                    if !ready.proxy_ok || !ready.agy_ok {
                        let why = ready.agy_error.or(ready.error).unwrap_or_else(|| "Antigravity proxy isn’t ready yet.".into());
                        worker_proxy.send(Msg::ClientSet(p, on, Err(why), None));
                        continue;
                    }
                }
                let pointed = clients::pointed(p, proxy_port);
                let r = clients::set(p, on, proxy_port);
                let restarted = p == Provider::Codex && r.is_ok() && matches!(codex_daemon::follow(), Ok(Followed::Restarted(_)));
                worker_proxy.send(Msg::ClientSet(p, on, r, if pointed { None } else { Some(restarted) }));
            }
        });
        Hub { shared, proxy_port, proxy, win: None, hosts, clients: None, client_tx, note: None, waiting: None, open: Arc::default() }
    }

    fn told(&self) -> Told {
        Told { clients: self.clients, note: self.note.clone(), waiting: self.waiting.as_ref().map(|w| (w.provider, w.target.clone())) }
    }

    fn tell(&self) {
        if let Some(w) = &self.win {
            w.messenger().send(WinMsg::Told(self.told()));
        }
    }

    pub fn open_tab(&mut self, tab: Option<Tab>) {
        let initial = tab.unwrap_or_else(|| Tab::from_index(self.open.tab.load(Ordering::Relaxed)));
        if let Some(t) = tab {
            self.open.tab.store(t as usize, Ordering::Relaxed);
        }
        let existed = self.win.is_some();
        let (shared, hosts, port, told) = (self.shared.clone(), self.hosts.clone(), self.proxy_port, self.told());
        let options = WindowOptions::new("Token Pool", 640.0, 740.0).min_size(380.0, 220.0).keep_size("main");
        olo_kit::open_or_raise(&mut self.win, options, move |cx| PoolWindow::new(shared, hosts, port, told, initial, cx));
        self.open.open.store(true, Ordering::Relaxed);
        if existed
            && let Some(t) = tab
            && let Some(w) = &self.win
        {
            w.messenger().send(WinMsg::Tab(t));
        }
        self.read_clients();
    }

    fn read_clients(&self) {
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || proxy.send(Msg::Clients(PROVIDERS.map(|p| clients::enabled(p, port)))));
    }

    fn set_client(&mut self, p: Provider, on: bool) {
        let _ = self.client_tx.send((p, on));
    }

    fn import_local(&mut self, p: Provider, target: Option<String>) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::SignedIn(import::this_computer(p), target)));
    }

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

    fn sync_all_hosts(&mut self) {
        let list = self.hosts.list();
        let mut started = 0usize;
        let mut first_err: Option<String> = None;
        for h in list.iter().filter(|h| h.wanted && h.doing.is_none()) {
            match self.hosts.act(&h.id, "sync") {
                Ok(()) => started += 1,
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        self.note = Some(if let Some(e) = first_err {
            (e, true)
        } else if started == 0 {
            ("No remote hosts to sync right now.".to_owned(), false)
        } else if started == 1 {
            ("Syncing 1 remote host…".to_owned(), false)
        } else {
            (format!("Syncing {started} remote hosts…"), false)
        });
    }

    fn command(&mut self, command: &str) {
        match command {
            "tab-accounts" => return self.open_tab(Some(Tab::Accounts)),
            "tab-activity" | "clients" => return self.open_tab(Some(Tab::Activity)),
            "tab-machines" | "hosts" => return self.open_tab(Some(Tab::Machines)),
            "sync-hosts" => {
                self.sync_all_hosts();
                return;
            }
            "add-host" => {
                olo_kit::app_menu::open_settings("rex");
                return;
            }
            _ => {}
        }
        let provider = command.rsplit(':').next().and_then(Provider::parse);
        match (command.split(':').next(), provider) {
            (Some("import"), Some(p)) => self.import_local(p, None),
            (Some("separate"), Some(p)) => self.sign_in_separately(p, None),
            _ => {}
        }
    }

    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            if let Some(tab) = requested_tab(&launch.args) {
                self.open_tab(Some(tab));
            } else if !launch.args.iter().any(|a| a == "--service") {
                self.open_tab(None);
            }
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {
                if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::Changed);
                }
                return;
            }
            Msg::Open => {
                olo_kit::instance::listen(crate::APP_ID, self.proxy.clone());
                self.open_tab(None);
                return;
            }
            Msg::Clients(on) => self.clients = Some(on),
            Msg::ClientSet(prov, on, r, restarted) => {
                if r.is_ok() {
                    if let Some(c) = self.clients.as_mut() {
                        c[client_index(prov)] = on;
                    }
                    self.shared.set_paused(prov, !on);
                }
                self.note = Some(match r {
                    Ok(()) => {
                        if prov == Provider::Antigravity && restarted.is_some() {
                            if on {
                                let is_fish = std::env::var("SHELL").is_ok_and(|s| s.ends_with("/fish") || s == "fish");
                                if is_fish {
                                    let fish_p = std::env::var_os("XDG_CONFIG_HOME")
                                        .map(std::path::PathBuf::from)
                                        .filter(|p| !p.as_os_str().is_empty())
                                        .map(|c| folder(&c.join("fish/conf.d/olo-pool-agy.fish")))
                                        .unwrap_or_else(|| "~/.config/fish/conf.d/olo-pool-agy.fish".into());
                                    (format!("Antigravity uses the pool in new fish shells (or run: source {fish_p})."), false)
                                } else {
                                    let sh_p = folder(&clients::agy_sh_path());
                                    (format!("Antigravity uses the pool in new shells (or run: . {sh_p})."), false)
                                }
                            } else {
                                ("Antigravity is back to its own sign-in in new shells.".to_owned(), false)
                            }
                        } else {
                            let when = match (prov, restarted) {
                                (_, None) => "from its next request",
                                (Provider::Codex, Some(true)) => "from now (its service restarted)",
                                (Provider::Codex, Some(false)) => "from now",
                                (Provider::Claude | Provider::Antigravity, Some(_)) => "from its next start",
                            };
                            let verb = if on { "uses the pool" } else { "is back to its own sign-in" };
                            (format!("{} {verb} {when}.", prov.client()), false)
                        }
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
            Msg::TabChanged(tab) => {
                self.open.tab.store(tab as usize, Ordering::Relaxed);
                return;
            }
            Msg::Closed => {
                self.win = None;
                self.open.open.store(false, Ordering::Relaxed);
                return;
            }
            Msg::CloseWindows => {
                if let Some(w) = self.win.take() {
                    w.close();
                }
                return;
            }
        }
        self.tell();
    }
}

pub fn requested_tab(args: &[String]) -> Option<Tab> {
    if let Some(i) = args.iter().position(|a| a == "--tab")
        && let Some(val) = args.get(i + 1)
        && let Some(t) = Tab::parse_name(val)
    {
        return Some(t);
    }
    if args.iter().any(|a| a == "--hosts-window") {
        return Some(Tab::Machines);
    }
    if let Ok(env_tab) = std::env::var("OLO_POOL_DEMO_TAB")
        && let Some(t) = Tab::parse_name(&env_tab)
    {
        return Some(t);
    }
    None
}

fn add_menu() -> Menu {
    let mut menu = Menu::new("Accounts");
    for (i, p) in PROVIDERS.into_iter().enumerate() {
        if i > 0 {
            menu = menu.separator();
        }
        let key = p.key();
        menu = menu.item(format!("Add This Computer’s {} Sign-In", p.client()), format!("import:{key}")).item(format!("Sign In to {} Separately…", p.client()), format!("separate:{key}"));
    }
    menu
}

/// Crisp 16×16 native Olo icon element (`system`, `network`, `warning`, etc.).
fn icon_box(name: &'static str, size_pt: f32) -> Element {
    canvas(move |g, b| {
        icons::paint(g, b, name);
    })
    .paint_key(style::key((name, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(size_pt))
    .flex_shrink_0()
    .into_element()
}

/// 6×6 rounded provider swatch (`Claude Code` orange, `Codex` blue).
fn swatch_dot(color: Hsla, size_pt: f32) -> Element {
    let k = [color.h, color.s, color.l, color.a].map(f32::to_bits);
    canvas(move |g, b| {
        style::dot(g, b.center(), size_pt, color);
    })
    .paint_key(style::key((k, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(size_pt))
    .flex_shrink_0()
    .into_element()
}

/// Provider identity colour: shifts Antigravity purple ~20° warmer toward magenta
/// and ~10% lighter in dark mode so its chart line and dots separate unmistakably
/// from Codex blue while keeping light mode unchanged.
fn provider_color(p: &olo_kit::palette::Palette, prov: Provider) -> Hsla {
    if prov == Provider::Antigravity && p.mode == olo_theme::ThemeMode::Dark { style::c([0xd2, 0x7c, 0xf2, 0xff]) } else { style::c(p.hue(provider_hue(prov))) }
}

fn client_color(p: &olo_kit::palette::Palette, client: &str) -> Hsla {
    if client == "Antigravity" && p.mode == olo_theme::ThemeMode::Dark { style::c([0xd2, 0x7c, 0xf2, 0xff]) } else { style::c(p.hue(hue(client))) }
}

/// Readable secondary text colour (meets ≥4.5:1 contrast on both panel and list well).
fn secondary_ink(p: &olo_kit::palette::Palette, over: olo_theme::Rgba) -> Hsla {
    let target = if p.mode == olo_theme::ThemeMode::Dark { olo_theme::mix(p.panel_text, over, 0.28) } else { olo_theme::mix(p.panel_text, over, 0.36) };
    style::c(olo_theme::readable(target, over, 4.6))
}

fn outcome_color(p: &olo_kit::palette::Palette, o: Outcome) -> Hsla {
    match o {
        Outcome::Completed => style::c(p.success),
        Outcome::RateLimited => style::c(p.warning),
        Outcome::Failed => style::c(p.failure),
        Outcome::Cancelled => secondary_ink(p, p.list_background),
    }
}

fn recent_details(r: &RecentReq) -> String {
    let req = &r.request;
    let mut s = format!("{} · {} · {} on {}", req.outcome.label(), r.machine_name, req.model, r.account);
    if !req.client.is_empty() {
        s.push_str(&format!(" · {}", req.client));
    }
    s.push_str(&format!(" · {} · {}", clock(req.at), duration(req.ms)));
    if req.status != 0 && req.outcome != Outcome::Completed {
        s.push_str(&format!(" · HTTP {}", req.status));
    }
    if req.outcome == Outcome::Completed || req.usage.input_tokens + req.usage.output_tokens > 0 {
        s.push_str(&format!(" · {} in · {} out", tokens(req.usage.input_tokens), tokens(req.usage.output_tokens)));
    }
    s
}

fn live_details(o: &LiveReq, now_ms: i64) -> String {
    let mut s = format!("Running · {} · {} on {}", o.machine_name, o.model, o.account);
    if !o.client.is_empty() {
        s.push_str(&format!(" · {}", o.client));
    }
    if o.started_ms > 0 {
        s.push_str(&format!(" · {} so far", duration((now_ms - o.started_ms).max(0) as u64)));
    }
    s
}

/// Crisp 16pt/14pt neutral-slate machine glyph (`is_local`: desktop tower; remote: stacked server nodes).
/// Uses the neutral slate ramp (#5E6E84 light / #8C9DB3 dark) so list rows never introduce red/green/blue
/// colour noise or disappear into dark backgrounds.
/// A row of a list under the rule that parts it from the row before: one
/// line, the row's own top edge. A chosen row's tint runs up to the rule
/// on both sides of it. Between rows stood the kit's separator, a groove
/// in six points of the list's ground: a tinted row ended three points
/// short of the rule, a strip of white between them (two pixels at 1x,
/// three at 1.25x; both reviewers, 2026-11-26: "should meet the
/// horizontal divider line flush", "end the band exactly on the rule").
fn under_rule(p: &olo_kit::palette::Palette, row: Element) -> Element {
    // (The groove's dark line by day, as it was. By night that is the
    // list's own darkness, 1.0 to 1, and it was the groove's light line
    // that showed: a step of the list's words into its ground.)
    let rule = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.list_background, p.list_item_text, 0.16)) } else { style::shade(p, p.panel_background, 1.2) };
    div().w_full().border_t_1().border_color(rule).child(row).into_element()
}

fn neutral_machine_icon(is_local: bool, size_pt: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let body = if dark { style::c([0x8c, 0x9d, 0xb3, 0xff]) } else { style::c([0x5e, 0x6e, 0x84, 0xff]) };
        let top = if dark { style::c([0xb6, 0xc5, 0xd8, 0xff]) } else { style::c([0x84, 0x95, 0xab, 0xff]) };
        let edge = if dark { style::c([0x3b, 0x4b, 0x5c, 0xff]) } else { style::c([0x3b, 0x47, 0x56, 0xff]) };
        let slot = if dark { style::c([0x1d, 0x2c, 0x38, 0xff]) } else { style::c([0xea, 0xef, 0xf5, 0xff]) };
        // The icon's own square, on device pixels, in the middle of the
        // box it is given: a line's height (below), so that it stands on
        // the same middle as the words beside it at every scale. It was
        // a box of its own size (14 points: 17.5 px at 1.25x), centred
        // in the row by layout, and its shape was cut from whatever that
        // box rounded to: a pixel off its label one way or the other as
        // the row moved (two low at 1.25x, Opus 5.5, 2026-11-26).
        let r = style::snap_centred(g, b.center(), size_pt, size_pt);
        if is_local {
            // Crisp desktop tower centred in `r`
            let w = (f32::from(r.size.width) * 0.64).round().max(8.0);
            let h = (f32::from(r.size.height) * 0.84).round().max(10.0);
            let box_r = style::snap_centred(g, r.center(), w, h);
            let inner = style::inset(g, box_r, 1.0);
            style::fill_rect(g, inner, body);
            let cap = Bounds { origin: inner.origin, size: size(inner.size.width, px(2.0)) };
            style::fill_rect(g, cap, top);
            let bay = Bounds { origin: point(inner.left() + px(1.5), inner.top() + px(3.5)), size: size((inner.size.width - px(3.0)).max(px(2.0)), px(1.5)) };
            style::fill_rect(g, bay, slot);
            let led = style::snap_centred(g, point(inner.right() - px(2.5), inner.bottom() - px(2.5)), 2.0, 2.0);
            style::fill_rect(g, led, slot);
            style::frame_rect(g, box_r, edge);
        } else {
            // Two crisp stacked server blades (remote Rex host)
            let w = (f32::from(r.size.width) * 0.86).round().max(10.0);
            let bh = ((f32::from(r.size.height) * 0.36).round()).max(4.0);
            let x = (f32::from(r.center().x) - w * 0.5).round();
            let y1 = (f32::from(r.center().y) - bh - 1.0).round();
            let y2 = (f32::from(r.center().y) + 1.0).round();
            for y in [y1, y2] {
                let blade = style::snap(g, Bounds { origin: point(px(x), px(y)), size: size(px(w), px(bh)) });
                let inner = style::inset(g, blade, 1.0);
                style::fill_rect(g, inner, body);
                style::hline(g, inner.left(), inner.right(), inner.top(), top);
                let led = style::snap_centred(g, point(inner.left() + px(2.5), inner.center().y), 1.5, 1.5);
                style::fill_rect(g, led, slot);
                style::frame_rect(g, blade, edge);
            }
        }
    })
    .paint_key(style::key(("mach", is_local, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(DETAIL_ROW))
    .flex_shrink_0()
    .into_element()
}

/// Crisp 3/4-arc busy spinner glyph in neutral secondary ink matching 8pt status dots at 1×.
fn spinner_arc(across: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let ink = secondary_ink(&p, p.list_background);
        let c = b.center();
        let rad = (across * 0.5 - 0.75).max(2.2);
        let mut path = olo_kit::PathBuilder::stroke(px(1.5));
        let steps = 18;
        let start = -std::f32::consts::FRAC_PI_2;
        let sweep = std::f32::consts::TAU * 0.72;
        for i in 0..=steps {
            let a = start + sweep * (i as f32 / steps as f32);
            let pt = point(c.x + px(rad * a.cos()), c.y + px(rad * a.sin()));
            if i == 0 {
                path.move_to(pt);
            } else {
                path.line_to(pt);
            }
        }
        if let Ok(built) = path.build() {
            g.paint_path(built, ink);
        }
    })
    .paint_key(style::key(("spinner", across.to_bits())))
    .w(px(across))
    .h(px(across))
    .flex_shrink_0()
    .into_element()
}

/// Left-anchored BeOS diagonal barber-pole indeterminate progress bar inside a sunken well.
fn barber_bar(width_pt: f32, height_pt: f32, color: Hsla) -> Element {
    let ck = style::ink_key(Some(color));
    canvas(move |g, b| {
        let p = theme::palette();
        let inner = style::inset(g, b, 2.0);
        let bg = style::c(p.document_background);
        style::fill_rect(g, inner, bg);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: inner }), |g| {
            let h = f32::from(inner.size.height);
            let w = f32::from(inner.size.width);
            let pitch = 10.0f32;
            let stripe_w = 5.0f32;
            let mut x = -h;
            while x < w + h {
                let mut poly = olo_kit::PathBuilder::fill();
                let p0 = point(inner.left() + px(x), inner.bottom());
                let p1 = point(inner.left() + px(x + stripe_w), inner.bottom());
                let p2 = point(inner.left() + px(x + stripe_w + h), inner.top());
                let p3 = point(inner.left() + px(x + h), inner.top());
                poly.add_polygon(&[p0, p1, p2, p3], true);
                if let Ok(built) = poly.build() {
                    g.paint_path(built, color);
                }
                x += pitch;
            }
        });
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(("barber", width_pt.to_bits(), height_pt.to_bits(), ck)))
    .w(px(width_pt))
    .h(px(height_pt))
    .flex_shrink_0()
    .into_element()
}

/// Extract the actual loopback port from a bind/listen error message so the
/// Gateway Stopped banner and This Computer section always blame the exact
/// listener port that failed (17891 vs 17892).
fn error_port(raw_error: &str, fallback: u16) -> u16 {
    if let Some(rest) = raw_error.split("127.0.0.1:").nth(1) {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(p) = digits.parse::<u16>()
            && p != 0
        {
            return p;
        }
    }
    fallback
}

/// BeOS-native full-width Gateway Stopped alert rendered above the tab bar on every tab.
fn gateway_stopped_alert(p: &olo_kit::palette::Palette, port: u16, raw_error: &str, cx: &mut Cx<PoolWindow>) -> Element {
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = if dark { style::c(olo_theme::mix(p.panel_background, p.failure, 0.14)) } else { style::c([0xfd, 0xec, 0xec, 0xff]) };
    let border = Hsla { a: 0.40, ..style::c(p.failure) };
    let blamed_port = error_port(raw_error, port);
    let body = if raw_error.to_ascii_lowercase().contains("address already in use") {
        format!("Another app is using port {blamed_port}, so Claude\u{a0}Code, Codex, and Antigravity can’t reach the pool.")
    } else {
        raw_error.to_owned()
    };
    div()
        .w_full()
        .p(px(16.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div().w_full().flex().flex_row().items_start().gap(px(10.0)).child(icon_box("stop", 32.0)).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(StringView::new("Gateway stopped").bold(true).height(m::LABEL_ROW))
                    .child(StringView::new(body).size(style::SMALL_SIZE).wrap(true)),
            ),
        )
        .child(
            widgets::button_row()
                .w_full()
                .h(px(m::CONTROL_HEIGHT))
                .child(Button::new("Change Port…").id("pool-change-port").on_click(|_: &ClickEvent, _| olo_kit::app_menu::open_settings("ai")))
                .child(Button::new("Try Again").id("pool-retry-gateway").default(true).on_click(cx.listener(|v, _: &ClickEvent, _| {
                    if !demo_enabled() {
                        v.shared.sync_soon(None);
                    }
                }))),
        )
        .into_element()
}

/// Warning-tinted inline confirmation box matching BeOS alert conventions:
/// 32pt native `warning` icon, bold primary question, clean explanation, and
/// right-aligned buttons below (never overlapping safe action buttons).
fn warning_confirm(p: &olo_kit::palette::Palette, question: String, consequence: String, reassurance: Option<String>, cancel_btn: Button, confirm_btn: Button) -> Element {
    let (bg, border) = if p.mode == olo_theme::ThemeMode::Dark {
        (style::c(olo_theme::mix(p.list_background, p.panel_background, 0.85)), style::c([0xd4, 0x9b, 0x24, 0xff]))
    } else {
        (style::c([0xff, 0xf4, 0xcc, 0xff]), style::c([0xe2, 0xc2, 0x5a, 0xff]))
    };
    let mut copy =
        div().flex_1().min_w_0().flex().flex_col().gap(px(6.0)).child(StringView::new(question).bold(true).wrap(true)).child(StringView::new(consequence).size(style::SMALL_SIZE).wrap(true));
    if let Some(extra) = reassurance {
        copy = copy.child(StringView::new(extra).size(style::SMALL_SIZE).color(secondary_ink(p, p.panel_background)).wrap(true));
    }
    div()
        .w_full()
        .p(px(10.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().w_full().flex().flex_row().items_start().gap(px(10.0)).child(icon_box("warning", 32.0)).child(copy))
        .child(widgets::button_row().w_full().h(px(m::CONTROL_HEIGHT)).child(cancel_btn).child(confirm_btn))
        .into_element()
}

/// Neutral slate fill for quota and indeterminate progress bars so provider identity
/// colours (Claude orange, Codex blue) never collide with generic meters.
fn neutral_bar_color(p: &olo_kit::palette::Palette) -> Hsla {
    style::neutral_bar(p)
}

/// Compute ONE shared quota-label column width across every account in the list
/// so every meter bar in Claude Code, Codex, and Antigravity starts on one exact
/// vertical axis and no scoped bucket label ("Gemini Pro", "Weekly") ever truncates.
fn shared_quota_label_width(accounts: &[Account], narrow: bool) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let max_w = accounts
        .iter()
        .flat_map(|a| a.windows.iter())
        .map(|w| {
            let l = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
            f32::from(style::text_width(l, style::SMALL_SIZE, font.clone()))
        })
        .fold(0.0f32, f32::max);
    let max_cap = if narrow { 92.0 } else { 124.0 };
    (max_w + 12.0).ceil().clamp(48.0, max_cap)
}

/// Compact inline single-row quota meter with fixed right-aligned percentage column
/// and left-aligned reset text (`5-hour | [====] |  42%  resets Thu 00:09 (2h 10m)`).
/// Bar colour uses neutral slate (<80%) or warning amber (≥80% / exhausted); failure red
/// is reserved strictly for actionable errors (`Signed out`, `failed`, `Can't sync`).
fn inline_quota_row(p: &olo_kit::palette::Palette, w: &QuotaWindow, t: i64, label_w: f32, narrow: bool) -> Element {
    let label = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
    let pct = w.used.clamp(0.0, 999.0);
    let pct_text = format!("{pct:.0}%");
    let reset_text = if let Some(r) = w.resets_at.filter(|&r| r > t) {
        if narrow { span(r - t) } else { format!("resets {} ({})", clock(r), span(r - t)) }
    } else if w.used <= 0.0 {
        "not started".to_owned()
    } else {
        String::new()
    };
    let (bar_color, pct_color) = if w.exhausted(t) || pct >= 80.0 { (style::c([0xe3, 0xa0, 0x08, 0xff]), style::c(p.warning)) } else { (neutral_bar_color(p), style::c(p.panel_text)) };
    let fraction = (w.used / 100.0) as f32;
    let bar_key = style::key((fraction.to_bits(), style::ink_key(Some(bar_color))));
    let bar_el = canvas(move |g, b| {
        let pal = theme::palette();
        level_bar(g, b, fraction, Some(bar_color), &pal);
    })
    .paint_key(bar_key)
    .flex_1()
    .min_w(px(60.0))
    .max_w(px(280.0))
    .h(px(10.0));

    let sec = secondary_ink(p, p.list_background);
    let reset_w = if narrow { 62.0 } else { 158.0 };
    div()
        .w_full()
        .h(px(18.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(div().w(px(label_w)).flex_shrink_0().child(StringView::new(label).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)))
        .child(bar_el)
        .child(div().w(px(32.0)).flex_shrink_0().flex().justify_end().child(StringView::new(pct_text).size(style::SMALL_SIZE).color(pct_color).height(DETAIL_ROW)))
        .child(div().w(px(reset_w)).flex_shrink_0().child(StringView::new(reset_text).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)))
        .into_element()
}

#[derive(Clone, PartialEq)]
pub enum Pick {
    Live(String),
    Recent(RecentReq),
}

#[derive(Clone)]
pub enum WinMsg {
    Changed,
    Told(Told),
    Tick,
    HostsTick,
    Tab(Tab),
    SelectAccount(String),
    SelectHost(String),
    ViewHostActivity(String),
    ScopeIndex(usize),
    Span(usize),
    Pick(Pick),
    ClearPick,
    AddMenu(Bounds<Pixels>),
    AccountMenu(String, Bounds<Pixels>),
    AccountContextMenu(String, olo_kit::Point<Pixels>),
    HostContextMenu(String, olo_kit::Point<Pixels>),
    Account(String, String),
    Renamed(bool),
    RemoveAccount(Option<String>),
    ResetCredit(Option<String>),
    FoundRunning(Box<Running>),
    RestartCodex,
    CodexRestarted(Result<i32, String>),
    HostDo(String, &'static str),
    HostAskRemove(Option<String>),
    SyncAllHosts,
}

pub struct PoolWindow {
    shared: Arc<Shared>,
    hosts: Arc<Hosts>,
    proxy_port: u16,
    told: Told,
    tab: Tab,
    // Accounts tab
    accounts_scroll: ScrollHandle,
    selected_account: Option<String>,
    renaming: Option<(String, Child<TextField>)>,
    removing_account: Option<String>,
    resetting_credit: Option<String>,
    // Activity tab
    scope: MachineScope,
    span_idx: usize,
    activity_picked: Option<Pick>,
    activity_scroll: ScrollHandle,
    // Machines tab
    hosts_scroll: ScrollHandle,
    selected_host: Option<String>,
    running: Option<Running>,
    scanning: Option<Arc<AtomicBool>>,
    restarting_codex: bool,
    codex_note: Option<(String, bool)>,
    host_asking_remove: Option<String>,
    host_refused: Option<(String, String)>,
}

impl PoolWindow {
    fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, proxy_port: u16, told: Told, tab: Tab, cx: &mut Cx<Self>) -> PoolWindow {
        if !demo_enabled() {
            hosts.refresh();
            cx.after(HOSTS_POLL_EVERY, WinMsg::HostsTick);
        }
        let mut w = PoolWindow {
            shared,
            hosts,
            proxy_port,
            told,
            tab,
            accounts_scroll: ScrollHandle::new(),
            selected_account: None,
            renaming: None,
            removing_account: None,
            resetting_credit: None,
            scope: MachineScope::All,
            span_idx: 0,
            activity_picked: None,
            activity_scroll: ScrollHandle::new(),
            hosts_scroll: ScrollHandle::new(),
            selected_host: None,
            running: None,
            scanning: None,
            restarting_codex: false,
            codex_note: None,
            host_asking_remove: None,
            host_refused: None,
        };
        if tab == Tab::Machines {
            w.start_scanner(cx);
        }
        w.schedule_tick(cx);
        w
    }

    fn start_scanner(&mut self, cx: &mut Cx<Self>) {
        if demo_enabled() || self.scanning.is_some() {
            return;
        }
        let flag = Arc::new(AtomicBool::new(true));
        self.scanning = Some(flag.clone());
        let (port, tx) = (self.proxy_port, cx.messenger());
        let _ = std::thread::Builder::new().name("pool-clients-scan".into()).spawn(move || {
            while flag.load(Ordering::Relaxed) {
                tx.send(WinMsg::FoundRunning(Box::new(olo_token_pool::running::scan(port))));
                for _ in 0..10 {
                    if !flag.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(PROCESS_POLL_EVERY / 10);
                }
            }
        });
    }

    fn stop_scanner(&mut self) {
        if let Some(flag) = self.scanning.take() {
            flag.store(false, Ordering::Relaxed);
        }
    }

    fn switch_tab(&mut self, tab: Tab, cx: &mut Cx<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == Tab::Machines {
            self.start_scanner(cx);
        } else {
            self.stop_scanner();
        }
        olo_kit::proxy().send(Msg::TabChanged(tab));
    }

    fn schedule_tick(&self, cx: &mut Cx<Self>) {
        if demo_enabled() {
            return;
        }
        let busy = !self.shared.snapshot().open.is_empty() || self.hosts.list().iter().any(|h| h.status.as_ref().and_then(|s| s["open"].as_array()).is_some_and(|o| !o.is_empty()));
        cx.after(Duration::from_secs(if busy { 1 } else { 20 }), WinMsg::Tick);
    }

    #[allow(clippy::type_complexity)]
    fn snapshot(&self) -> (Pool, Activity, Vec<Stands>, Option<Running>, Option<[bool; 3]>, HashMap<String, Vec<String>>) {
        if demo_enabled() {
            let d = demo_data();
            return (d.pool, d.activity, d.hosts, Some(d.running), Some(d.uses), d.unaccepted);
        }
        let pool = self.shared.snapshot();
        let activity = self.shared.activity(Clone::clone);
        let hosts = self.hosts.list();
        let mut unaccepted = HashMap::new();
        for h in &hosts {
            let u = self.shared.unaccepted_handovers(&h.id);
            if !u.is_empty() {
                unaccepted.insert(h.id.clone(), u);
            }
        }
        (pool, activity, hosts, self.running.clone(), self.told.clients, unaccepted)
    }

    fn menus(&self) -> Vec<Menu> {
        let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
        let sorted = sorted_hosts(&hosts);
        let has_wanted_host = sorted.iter().any(|h| h.wanted);
        let sel_host = self.selected_host.as_deref().or_else(|| sorted.first().map(|h| h.id.as_str()));
        let can_show_act = sel_host.and_then(|id| sorted.iter().find(|h| h.id == id)).is_some_and(|h| h.wanted || h.status.is_some());
        let has_daemon = if demo_enabled() { demo_data().running.daemon.is_some() } else { self.running.as_ref().and_then(|r| r.daemon.as_ref()).is_some() };
        vec![
            Menu::new("Pool").app_items(&["ai", "rex", "deskbar"]).item("Refresh Quota", "refresh").key("r").item("Reconnect Local Clients", "capture").close_item(),
            Menu::new("View")
                .item("Accounts", "tab-accounts")
                .key("1")
                .checked(self.tab == Tab::Accounts)
                .item("Activity", "tab-activity")
                .key("2")
                .checked(self.tab == Tab::Activity)
                .item("Machines", "tab-machines")
                .key("3")
                .checked(self.tab == Tab::Machines),
            add_menu(),
            Menu::new("Machines")
                .item("Add Host…", "add-host")
                .item("Show Activity", "show-host-activity")
                .enabled(can_show_act)
                .item("Sync All", "sync-hosts")
                .shift_key("s")
                .enabled(has_wanted_host)
                .separator()
                .item("Restart Codex Service", "restart-codex")
                .enabled(has_daemon),
        ]
    }

    fn scope_options(hosts: &[Stands]) -> Vec<(MachineScope, SharedString)> {
        let mut opts = vec![(MachineScope::All, olo_kit::words::tr("All Machines").into()), (MachineScope::Local, olo_kit::words::tr("This Computer").into())];
        for h in hosts {
            if h.wanted || h.status.is_some() {
                opts.push((MachineScope::Host(h.id.clone()), h.name.clone().into()));
            }
        }
        opts
    }

    fn effective_port(&self) -> u16 {
        let bound = self.shared.bound_port();
        if bound != 0 {
            bound
        } else if self.proxy_port != 0 {
            self.proxy_port
        } else {
            olo_token_pool::intercept::PORT
        }
    }

    fn account_menu_items(a: &Account) -> Menu {
        Menu::new("Account")
            .item("Rename…", "rename")
            .item("Refresh Quota", "refresh")
            .item("Claim Banked Reset…", "reset")
            .enabled(a.credit().is_some())
            .separator()
            .item("Sign In Again in Terminal…", "sign-in")
            .item(format!("Use This Computer’s {} Sign-In", a.provider.client()), "sign-in-local")
            .separator()
            .item("Remove from Pool…", "remove")
    }

    fn account_command(&mut self, id: &str, command: &str, cx: &mut Cx<Self>) {
        let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
        let Some(a) = pool.account(id).cloned() else { return };
        self.selected_account = Some(a.id.clone());
        match command {
            "rename" => {
                let field = text_field(cx, |f| f.name("Name").with_text(a.name.clone()));
                field.update(cx, |f, cx| f.caret_to_end(cx));
                field.focus(cx);
                cx.subscribe(&field, |_, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        cx.send(WinMsg::Renamed(true));
                    }
                });
                self.renaming = Some((a.id, field));
            }
            "refresh" => {
                if !demo_enabled() {
                    self.shared.sync_soon(Some(a.id));
                }
            }
            "sign-in" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, false)),
            "sign-in-local" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, true)),
            "remove" => self.removing_account = Some(a.id),
            "reset" => self.resetting_credit = Some(a.id),
            _ => {}
        }
    }

    // ---- Tab 1: Accounts --------------------------------------------------

    fn accounts_tab(&self, pool: &Pool, hosts: &[Stands], unaccepted: &HashMap<String, Vec<String>>, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let t = now();
        let vp = cx.viewport_size();
        let narrow = f32::from(vp.width) < 480.0;
        let compact_h = f32::from(vp.height) < 420.0;
        let sec_panel = secondary_ink(p, p.panel_background);

        // ONE sunken framed list well for all provider groups & account rows.
        // Sections stay in fixed canonical order (Claude Code, Codex, Antigravity);
        // problems float to the top within each section.
        let providers = sorted_providers(&pool.saved.accounts, t);
        let first_account_id = providers.first().and_then(|&prov| sorted_accounts(&pool.saved.accounts, prov, t).first().map(|a| a.id.as_str()));
        let sel_id = self.selected_account.as_deref().filter(|id| pool.account(id).is_some()).or(first_account_id);
        let wide = ["Sign In…", "Retry Now", "Sync Now", "Install", "Try Again"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
        let shared_label_w = shared_quota_label_width(&pool.saved.accounts, narrow);

        let group_bg = style::c(p.panel_background);
        let make_group_hdr = |prov: Provider, after_another: bool| {
            div()
                .w_full()
                .h(px(22.0))
                .px(px(10.0))
                .bg(group_bg)
                .border_b_1()
                .border_t(px(if after_another { 1.0 } else { 0.0 }))
                .border_color(style::c(p.control_border))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(swatch_dot(provider_color(p, prov), 8.0)))
                .child(StringView::new(prov.client()).size(style::SMALL_SIZE).bold(true).height(DETAIL_ROW))
                .into_element()
        };

        let mut items: Vec<Element> = Vec::new();
        let mut group_indices: Vec<(Provider, usize)> = Vec::new();
        let mut any = false;
        for prov in providers {
            let accounts = sorted_accounts(&pool.saved.accounts, prov, t);
            if accounts.is_empty() {
                continue;
            }
            // (A group after another has its band's own top edge for the
            // rule between them, as a row has: the last row's tint runs
            // to it. A separator of the kit's stood here too.)
            let after_another = any;
            any = true;
            group_indices.push((prov, items.len()));
            items.push(make_group_hdr(prov, after_another));
            for (idx, account) in accounts.iter().enumerate() {
                let pending = account.kept_on.as_deref().and_then(|hid| unaccepted.get(hid)).map(Vec::as_slice).unwrap_or(&[]);
                let rollup = account_rollup(pool, account, hosts);
                let is_sel = sel_id == Some(account.id.as_str());
                let row = self.account_row(account, hosts, pending, &rollup, is_sel, wide, shared_label_w, narrow, p, cx);
                items.push(if idx > 0 { under_rule(p, row) } else { row });
            }
        }
        if !any {
            let top_pad = if compact_h { 24.0 } else { 168.0 };
            let empty_box = if pool.loaded {
                div()
                    .w_full()
                    .pt(px(top_pad))
                    .pb(px(24.0))
                    .px(px(24.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon_box("token-pool", 48.0))
                    .child(div().pt(px(8.0)).child(StringView::new("No accounts yet").bold(true).height(m::LABEL_ROW)))
                    .child(
                        div().pt(px(4.0)).flex().flex_col().items_center().child(
                            StringView::new("Click Add Account… to add a Claude\u{a0}Code, Codex, or Antigravity subscription.")
                                .size(style::SMALL_SIZE)
                                .color(secondary_ink(p, p.list_background))
                                .wrap(true),
                        ),
                    )
            } else {
                div().w_full().pt(px(top_pad)).pb(px(24.0)).px(px(24.0)).flex().justify_center().child(StringView::new("Opening the pool’s accounts…").color(secondary_ink(p, p.list_background)))
            };
            items.push(empty_box.into_element());
        }

        // Sticky active provider header so scrolling past a section banner never
        // leaves orphaned account/Today rows without provider context.
        let active_group = |handle: &ScrollHandle, groups: &[(Provider, usize)]| -> Option<(Provider, usize)> {
            let y = -handle.offset().y;
            let mut found = None;
            for &(prov, idx) in groups {
                if let Some(b) = handle.bounds_for_item(idx)
                    && y > b.top()
                {
                    found = Some((prov, idx));
                }
            }
            found
        };
        let stuck = active_group(&self.accounts_scroll, &group_indices);
        let mut sv = ScrollView::new(&self.accounts_scroll).background(p.list_background).children(items);
        if let Some(&(first_prov, first_idx)) = group_indices.first() {
            let (sticky_prov, sticky_idx) = stuck.unwrap_or((first_prov, first_idx));
            sv = sv.sticky(sticky_idx, 22.0, make_group_hdr(sticky_prov, false));
            let handle = self.accounts_scroll.clone();
            let groups = group_indices;
            sv = sv.on_scroll(move |_, cx| {
                if active_group(&handle, &groups) != stuck {
                    cx.invalidate_owner();
                }
            });
        }
        let well = div().flex_1().min_h_0().child(sv);

        // Consistent footer order matching Machines: [Add Account…] [Edit Account…] … Updated HH:MM [Refresh Quota]
        let footer_text: (String, bool) = match (&self.told.waiting, &self.told.note) {
            (Some((provider, target)), _) => {
                let whose = target.as_deref().and_then(|id| pool.account(id)).map(|x| format!("{}’s", x.name)).unwrap_or_else(|| format!("the {}", provider.client()));
                (format!("Waiting for {whose} sign-in in Terminal…"), false)
            }
            (None, Some(n)) => n.clone(),
            (None, None) => match last_synced_at(pool) {
                Some(at) => (format!("Updated {}", clock(at)), false),
                None => (String::new(), false),
            },
        };
        let sel_owned = sel_id.map(str::to_owned);
        let can_edit = sel_owned.is_some();
        let has_accounts = !pool.saved.accounts.is_empty();
        let edit_id = sel_owned.unwrap_or_default();

        let mut footer = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .child(Button::new("Add Account…").id("pool-add-account").default(pool.loaded && !has_accounts).on_click(cx.listener(|_, _: &ClickEvent, cx| {
                let at = cx.listener_bounds();
                cx.send(WinMsg::AddMenu(at));
            })))
            .child(Button::new("Edit Account…").id("pool-account-details").enabled(can_edit).on_click(cx.listener(move |_, _: &ClickEvent, cx| {
                if !edit_id.is_empty() {
                    let at = cx.listener_bounds();
                    cx.send(WinMsg::AccountMenu(edit_id.clone(), at));
                }
            })));
        if self.told.waiting.is_some() {
            footer = footer.child(Button::new("Cancel Sign-In").on_click(|_: &ClickEvent, _| olo_kit::proxy().send(Msg::CancelSignIn)));
        }
        let mut status_box = div().flex_1().min_w_0().flex().justify_end();
        if !footer_text.0.is_empty() && !narrow {
            let col = if footer_text.1 { style::c(p.failure) } else { sec_panel };
            status_box = status_box.child(StringView::new(footer_text.0).color(col).height(m::CONTROL_HEIGHT));
        }
        footer = footer.child(status_box).child(Button::new("Refresh Quota").id("pool-refresh").enabled(has_accounts).on_click(cx.listener(|v, _: &ClickEvent, _| {
            if !demo_enabled() {
                v.shared.sync_soon(None);
                v.hosts.refresh();
            }
        })));

        div().flex_1().min_h_0().flex().flex_col().gap(px(8.0)).child(well).child(footer).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn account_row(
        &self,
        account: &Account,
        hosts: &[Stands],
        unaccepted: &[String],
        rollup: &AccountRollup,
        selected: bool,
        wide: f32,
        label_w: f32,
        narrow: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let t = now();
        let sec = secondary_ink(p, p.list_background);
        let cust = custody(account, hosts, unaccepted);
        let is_signed_out = account.signed_out.is_some();
        let is_out_of_quota = !is_signed_out && account.enabled && account_out_of_quota(account, t).is_some();
        let asking_inline = self.removing_account.as_deref() == Some(account.id.as_str()) || self.resetting_credit.as_deref() == Some(account.id.as_str());

        // Line 1: checkbox mark + bold account name (+ inline red Signed out / amber Out of quota / grey Paused)
        // Right side: inline Sign In… button when signed out (matching Retry Now on Machines),
        // else crisp machine custody chip with 16pt computer/server icon.
        let (shared, id_toggle) = (self.shared.clone(), account.id.clone());
        let check = CheckBox::new(account.name.clone(), account.enabled).id(format!("use-{}", account.id)).mark_only().on_toggle(move |on: &bool, _| {
            if !demo_enabled() {
                shared.set_enabled(&id_toggle, *on);
            }
        });
        let mut title_group = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(check))
            .child(StringView::new(account.name.clone()).bold(true).height(DETAIL_ROW));
        if is_signed_out {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Signed out").color(style::c(p.failure)).height(DETAIL_ROW));
        } else if !account.enabled {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Paused").color(sec).height(DETAIL_ROW));
        } else if is_out_of_quota {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Out of quota").color(style::c(p.warning)).height(DETAIL_ROW));
        }

        let line1 = if is_signed_out {
            let (prov, id_sign) = (account.provider, account.id.clone());
            let btn = Button::new("Sign In…").id(format!("pool-signin-{}", account.id)).min_width(wide).on_click(move |_: &ClickEvent, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SignIn(prov, id_sign.clone(), false));
                }
            });
            div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(8.0)).child(title_group).child(btn)
        } else {
            let chip_text = if cust.is_pending() { format!("{} · Sync pending", cust.machine_name()) } else { cust.machine_name() };
            let machine_chip = div()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(neutral_machine_icon(cust.is_local(), 14.0))
                .child(StringView::new(chip_text).size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));
            div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(8.0)).child(title_group).child(machine_chip)
        };

        // Line 2+: indented straight under the bold account name (13pt gutter + 6pt gap = 19pt)
        let mut facts: Vec<String> = Vec::new();
        if let Some(e) = &account.email {
            facts.push(e.clone());
        }
        if let Some(plan) = &account.plan {
            facts.push(format!("{} plan", plan_name(plan)));
        }
        let credits: u32 = account.credits.iter().map(|c| c.remaining).sum();
        if credits > 0 {
            facts.push(format!("{credits} banked reset{}", if credits == 1 { "" } else { "s" }));
        }

        let mut body = div().w_full().pl(px(19.0)).flex().flex_col().gap(px(3.0));
        if !facts.is_empty() {
            body = body.child(StringView::new(facts.join(" · ")).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
        }
        if let Some(why) = &account.signed_out {
            // Reason in readable secondary ink (matches Remote Rex Hosts' error rows!)
            body = body.child(StringView::new(why.clone()).size(style::SMALL_SIZE).color(sec).wrap(true));
        } else {
            if let Some((paid_fam, until)) = account_paid_scope(account, t) {
                let note = match paid_fam {
                    Some(fam) => format!("{fam} waiting for reset at {} · paid usage credits only if nothing else can serve.", clock(until)),
                    None => format!("Waiting for reset at {} · paid usage credits only if nothing else can serve.", clock(until)),
                };
                body = body.child(div().w_full().pb(px(3.0)).child(StringView::new(note).size(style::SMALL_SIZE).color(sec).wrap(true)));
            }
            for w in &account.windows {
                body = body.child(inline_quota_row(p, w, t, label_w, narrow));
            }
            if account.windows.is_empty() {
                body = body.child(StringView::new(if account.synced.is_some() { "No quota reported." } else { "Reading quota…" }).size(style::SMALL_SIZE).color(sec));
            }
            // Concise Today line (+ red failed count when failures occurred); local/remote breakdown lives on Activity.
            let u = &rollup.total_today;
            let failed_n = rollup.recent.iter().filter(|r| r.request.outcome == Outcome::Failed).count();
            let mut today_base = if u.requests == 0 {
                "Today: 0 requests".to_owned()
            } else {
                format!("Today: {} request{} · {} in · {} out", u.requests, if u.requests == 1 { "" } else { "s" }, tokens(u.input_tokens), tokens(u.output_tokens))
            };
            if failed_n > 0 {
                today_base.push_str(" · ");
            }
            let mut today_row = div().w_full().pt(px(2.0)).flex().flex_row().items_center().child(StringView::new(today_base).size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));
            if failed_n > 0 {
                today_row = today_row.child(StringView::new(format!("{failed_n} failed")).size(style::SMALL_SIZE).color(style::c(p.failure)).height(DETAIL_ROW));
            }
            body = body.child(today_row);
        }

        // Inline edit/confirm dialogs for this account
        if let Some((_, field)) = self.renaming.as_ref().filter(|(id, _)| *id == account.id) {
            body = body.child(div().pt(px(4.0)).flex().items_center().gap(px(m::SPACING)).child(TextControl::new(cx, field).width(180.0)).child(
                widgets::button_row().flex_1().child(Button::new("Cancel").on_click(cx.msg(WinMsg::Renamed(false)))).child(Button::new("Rename").default(true).on_click(cx.msg(WinMsg::Renamed(true)))),
            ));
        } else if self.resetting_credit.as_deref() == Some(account.id.as_str()) {
            let when = account.credit().and_then(|c| c.expires_at).map(|e| format!(" (expires {})", clock(e))).unwrap_or_default();
            body = body.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                format!("Claim one of {}’s banked resets now{when}?", account.name),
                "Its quota refills immediately, and one banked reset is spent.".into(),
                None,
                Button::new("Cancel").on_click(cx.msg(WinMsg::ResetCredit(None))),
                Button::new("Claim Reset").on_click(cx.msg(WinMsg::ResetCredit(Some(account.id.clone())))),
            )));
        } else if self.removing_account.as_deref() == Some(account.id.as_str()) {
            body = body.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                format!("Remove {} from Token Pool?", account.name),
                "Its sign-in stored here is deleted; the subscription account itself isn’t touched.".into(),
                None,
                Button::new("Cancel").on_click(cx.msg(WinMsg::RemoveAccount(None))),
                Button::new("Remove").on_click(cx.msg(WinMsg::RemoveAccount(Some(account.id.clone())))),
            )));
        }

        let sel_tint = if selected && !asking_inline {
            Some(if p.mode == olo_theme::ThemeMode::Dark {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.16))
            } else {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.09))
            })
        } else {
            None
        };
        let id_click = account.id.clone();
        let id_ctx = account.id.clone();
        let content_col = div().flex_1().min_w_0().flex().flex_col().child(line1).child(body);
        let mut row = div()
            .w_full()
            .px(px(10.0))
            .py(px(ROW_PAD))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::SelectAccount(id_click.clone()))))
            .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| cx.send(WinMsg::AccountContextMenu(id_ctx.clone(), e.position))))
            .child(content_col);
        if let Some(bg) = sel_tint {
            row = row.bg(bg);
        }
        row.into_element()
    }

    // ---- Tab 2: Activity --------------------------------------------------

    fn activity_tab(&self, pool: &Pool, local_activity: &Activity, hosts: &[Stands], p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let vp = cx.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);
        let narrow = vp_w < 480.0;
        let compact_h = vp_h < 520.0;
        let two_col = compact_h && vp_w >= 560.0;
        // Short two-column tiles (quarter / horizontal half / horizontal third): merge
        // Machine + Span popups onto the left column's top row aligned with "Recent Requests"
        // on the right so chart + table both keep generous vertical height!
        let merged_two_col = two_col && vp_h < 480.0;
        // Quarter / narrow vertical: pair Machine + Span on the top bar so "Requests per Minute"
        // stands cleanly without colliding with the span popup.
        let pair_top_popups = !two_col && vp_w < 500.0;
        let sec_panel = secondary_ink(p, p.panel_background);

        let opts = Self::scope_options(hosts);
        let scope_idx = opts.iter().position(|(s, _)| s == &self.scope).unwrap_or(0);
        let active_scope = opts[scope_idx].0.clone();
        let scope_labels: Vec<SharedString> = opts.iter().map(|(_, l)| l.clone()).collect();
        // Keep "Machine:" label unless space is tight in merged horizontal-third or very narrow.
        let show_machine_lbl = vp_w >= 360.0 && !merged_two_col && !pair_top_popups;
        let scope_w = if merged_two_col || pair_top_popups {
            132.0
        } else if !show_machine_lbl {
            130.0
        } else if narrow {
            172.0
        } else {
            196.0
        };
        let mut scope_pick = MenuField::new("pool-scope", scope_labels, scope_idx).width(scope_w).on_change(cx.msg_with(|i: &usize| WinMsg::ScopeIndex(*i)));
        if show_machine_lbl {
            scope_pick = scope_pick.label("Machine:");
        }

        let (span_name, span_mins, step_mins, grid_secs) = SPANS[self.span_idx];
        let span_pick = MenuField::new("pool-span", SPANS.iter().map(|s| s.0.into()).collect(), self.span_idx)
            .width(if narrow || merged_two_col || pair_top_popups { 112.0 } else { 124.0 })
            .on_change(cx.msg_with(|i: &usize| WinMsg::Span(*i)));

        let t_ms = crate::now_ms();
        let live = live_requests(pool, hosts, &active_scope);
        let live_badge = match live.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };

        let picked_line: Option<String> = match &self.activity_picked {
            Some(Pick::Live(key)) => live.iter().find(|o| &o.key == key).map(|o| live_details(o, t_ms)),
            Some(Pick::Recent(r)) => Some(recent_details(r)),
            None => None,
        };

        // Requests per Minute chart: crisp lines (no overlapping muddy wash),
        // draw series largest-total-first so smaller curves (Codex) stay visible on top,
        // and use plain `HH:MM` X-axis ticks (weekday only on `00:00` midnight).
        let t = now();
        let since = t - span_mins * 60;
        let combined = merged_activity(local_activity, hosts, &active_scope);
        let clients_list: Vec<String> = combined.clients();
        let lines: Vec<VecDeque<(f64, f64)>> = clients_list.iter().map(|cl| combined.series(cl, t, span_mins, step_mins)).collect();
        let totals: Vec<_> = clients_list.iter().map(|cl| combined.totals(cl, since)).collect();
        let shown: Vec<usize> = (0..clients_list.len()).filter(|&i| i < PROVIDERS.len() || totals[i].requests > 0).collect();
        let mut draw_order = shown.clone();
        draw_order.sort_by(|&a, &b| totals[b].requests.cmp(&totals[a].requests).then_with(|| a.cmp(&b)));
        let series: Vec<Series> = draw_order.iter().map(|&i| Series::new(clients_list[i].clone(), client_color(p, &clients_list[i]), Rc::new(lines[i].clone())).fill(false)).collect();
        let fmt = |v: f64| if v >= 10.0 || (v - v.round()).abs() < 1e-6 { format!("{v:.0}/min") } else { format!("{v:.1}/min") };
        let span_secs = (span_mins * 60) as f64;
        let graph = Graph::new(format!("pool-activity-{span_name}"), series, t as f64 + span_secs * 0.015)
            .span(span_secs)
            .grid(grid_secs)
            .unit(fmt)
            .y_labels(true)
            .x_labels(|sec| {
                use chrono::{Local, TimeZone, Timelike};
                let Some(at) = Local.timestamp_opt(sec as i64, 0).single() else { return String::new() };
                if at.hour() == 0 && at.minute() == 0 { at.format("%a %H:%M").to_string() } else { at.format("%H:%M").to_string() }
            })
            .smooth(false);

        // Stacked 18pt legend with fixed provider column whenever 3 entries would wrap
        // into an orphan 2+1 line; in wide+short horizontal-third (`two_col && left_w >= 380`)
        // use one concise horizontal row so the sparkline keeps full vertical breathing room.
        let left_w = (vp_w * 0.41).clamp(244.0, 480.0);
        let stack_legend = if two_col { left_w < 380.0 } else { vp_w < 860.0 };
        let compact_inline_legend = two_col && !stack_legend;
        let mut legend_row = if stack_legend {
            div().w_full().pt(px(2.0)).flex().flex_col().gap(px(0.0))
        } else if compact_inline_legend {
            div().w_full().h(px(18.0)).flex().flex_row().items_center().gap(px(12.0))
        } else {
            div().w_full().pt(px(2.0)).flex().flex_row().flex_wrap().items_center().gap(px(16.0))
        };
        for &i in &shown {
            let x = totals[i];
            let req_word = if compact_inline_legend { format!("{}", x.requests) } else { format!("{} request{}", x.requests, if x.requests == 1 { "" } else { "s" }) };
            let tok_tail = if !two_col && x.input_tokens + x.output_tokens > 0 { format!(" · {} in · {} out", tokens(x.input_tokens), tokens(x.output_tokens)) } else { String::new() };
            let name_el = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(swatch_dot(client_color(p, &clients_list[i]), 7.0))
                .child(StringView::new(clients_list[i].clone()).size(style::SMALL_SIZE).height(DETAIL_ROW));
            let name_col = if stack_legend { name_el.w(px(88.0)).flex_shrink_0() } else { name_el };
            let mut item = if stack_legend {
                div().w_full().h(px(18.0)).flex().flex_row().items_center().gap(px(6.0)).child(name_col)
            } else {
                div().flex().flex_row().items_center().gap(px(5.0)).child(name_col)
            };
            if x.failed > 0 {
                let mut stats = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(StringView::new(format!("{req_word} · ")).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW))
                    .child(StringView::new(format!("{} failed", x.failed)).size(style::SMALL_SIZE).color(style::c(p.failure)).height(DETAIL_ROW));
                if !tok_tail.is_empty() {
                    stats = stats.child(StringView::new(tok_tail).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW));
                }
                item = item.child(stats);
            } else {
                item = item.child(StringView::new(format!("{req_word}{tok_tail}")).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW));
            }
            legend_row = legend_row.child(item);
        }

        let (top_bar_opt, chart_hdr) = if merged_two_col {
            // Horizontal-third: both popups share the left column's top row,
            // aligned on the exact same baseline as "Recent Requests" on the right.
            let hdr = div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(6.0)).child(scope_pick).child(span_pick);
            (None, hdr)
        } else if pair_top_popups {
            // Quarter / narrow: Machine + Span share the top bar so the chart heading never collides.
            let top = div().w_full().flex().flex_row().items_center().justify_between().gap(px(6.0)).child(scope_pick).child(span_pick);
            let hdr = div().w_full().h(px(m::LABEL_ROW)).flex().flex_row().items_center().child(StringView::new("Requests per Minute").bold(true).height(m::LABEL_ROW));
            (Some(top), hdr)
        } else {
            let top = div().w_full().flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(scope_pick);
            let hdr = div()
                .w_full()
                .h(px(m::CONTROL_HEIGHT))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(StringView::new("Requests per Minute").bold(true).height(m::CONTROL_HEIGHT))
                .child(span_pick);
            (Some(top), hdr)
        };

        let mut chart_section = div().w_full().flex().flex_col().gap(px(6.0)).child(chart_hdr);
        if two_col {
            chart_section = chart_section.flex_1().min_h(px(56.0)).child(div().flex_1().min_h(px(40.0)).w_full().child(graph)).child(legend_row);
        } else {
            let gh = if compact_h { 112.0 } else { GRAPH_HEIGHT };
            chart_section = chart_section.child(div().w_full().h(px(gh)).child(graph)).child(legend_row);
        }

        // Recent Requests table: compute exact inner content width (subtracting outer pad,
        // TabView inset, well border, vertical scrollbar, and row padding), drop Machine
        // FIRST when all 7 columns don't fit, and cap Model on wide/zoomed windows so
        // Account absorbs slack right beside Model instead of floating across a void.
        let outer_pad = if vp_h < 340.0 || vp_w < 460.0 { 6.0 } else { 10.0 };
        let well_w = if two_col { vp_w - left_w - 20.0 - 2.0 * outer_pad - 24.0 } else { vp_w - 2.0 * outer_pad - 24.0 };
        let inner_w = well_w - 4.0 - widgets::SCROLL_BAR - 2.0 * m::TIGHT;
        let recent = recent_requests(pool, hosts, &active_scope, RECENT_TABLE_LIMIT);
        let has_hosts = hosts.iter().any(|h| h.wanted || h.status.is_some());
        let recent_well = self.recent_requests_well(&live, &recent, &active_scope, has_hosts, inner_w, t_ms, span_name, p, cx);

        let recent_hdr_h = if two_col { m::CONTROL_HEIGHT } else { m::LABEL_ROW };
        let recent_hdr = div()
            .w_full()
            .h(px(recent_hdr_h))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(StringView::new("Recent Requests").bold(true).height(recent_hdr_h))
            .child(StringView::new(live_badge).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(recent_hdr_h));

        let picked_bar = picked_line.map(|detail| {
            div()
                .w_full()
                .pt(px(2.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(detail).size(style::SMALL_SIZE).color(sec_panel).cut(true).height(DETAIL_ROW)))
                .child(Button::new("Clear").id("pool-clear-pick").on_click(cx.msg::<ClickEvent>(WinMsg::ClearPick)))
        });

        // Wide + short horizontal third or quarter: side-by-side 2-column layout with 20pt gutter
        if two_col {
            let left_col = div().relative().w(px(left_w)).flex_shrink_0().h_full().flex().flex_col().child(canvas(olo_kit::review::own_grid).paint_key(0).absolute().inset_0()).child(chart_section);
            let mut right_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
            if let Some(bar) = picked_bar {
                right_col = right_col.child(bar);
            }
            let body = div().flex_1().min_h_0().flex().flex_row().gap(px(20.0)).child(left_col).child(right_col);
            return match top_bar_opt {
                Some(top_bar) => div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(top_bar).child(body).into_element(),
                None => body.into_element(),
            };
        }

        let mut recent_section = div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
        if let Some(bar) = picked_bar {
            recent_section = recent_section.child(bar);
        }

        let mut root = div().flex_1().min_h_0().flex().flex_col().gap(px(if compact_h { 8.0 } else { 20.0 }));
        if let Some(top_bar) = top_bar_opt {
            root = root.child(top_bar);
        }
        root.child(chart_section).child(recent_section).into_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn recent_requests_well(
        &self,
        live: &[LiveReq],
        recent: &[RecentReq],
        active_scope: &MachineScope,
        has_hosts: bool,
        inner_w: f32,
        t_ms: i64,
        span_name: &str,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let sec_panel = secondary_ink(p, p.panel_background);
        let sec_list = secondary_ink(p, p.list_background);
        let time_w = 38.0;
        let machine_w = 96.0;
        let account_w = 88.0;
        let in_w = 42.0;
        let out_w = 38.0;
        let status_w = 96.0;
        // Exact column sums (including 1pt divider + 10pt gaps per boundary, model_min = 128pt):
        // 7 cols: 592pt; 6 cols (no Machine): 485pt; 4 cols (no Machine/In/Out): 383pt; 3 cols: 284pt.
        let show_machine_col = *active_scope == MachineScope::All && has_hosts && inner_w >= 596.0;
        let show_io_cols = inner_w >= 488.0;
        let show_account_col = inner_w >= 386.0;
        let wide_account_flex = show_account_col && inner_w > 700.0;

        let hdr_div = || div().w(px(1.0)).h(px(12.0)).bg(style::c(p.control_border)).flex_shrink_0();
        let hdr_label = |label: &'static str| StringView::new(label).size(style::SMALL_SIZE).bold(true).color(sec_panel).height(DETAIL_ROW);
        let mut header_row = div()
            .w_full()
            .pl(px(m::TIGHT))
            .pr(px(m::TIGHT + widgets::SCROLL_BAR))
            .pt(px(3.0))
            .pb(px(2.0))
            .bg(style::c(p.panel_background))
            .border_b_1()
            .border_color(style::c(p.control_border))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .child(div().w(px(time_w)).flex_shrink_0().child(hdr_label("Time")));
        if show_machine_col {
            header_row = header_row.child(hdr_div()).child(div().w(px(machine_w)).flex_shrink_0().child(hdr_label("Machine")));
        }
        header_row = header_row.child(hdr_div()).child(if wide_account_flex {
            div().w(px(176.0)).pr(px(8.0)).flex_shrink_0().child(hdr_label("Model"))
        } else {
            div().flex_1().min_w(px(116.0)).pr(px(8.0)).child(hdr_label("Model"))
        });
        if show_account_col {
            header_row = header_row.child(hdr_div()).child(if wide_account_flex {
                div().flex_1().min_w(px(96.0)).child(hdr_label("Account"))
            } else {
                div().w(px(account_w)).flex_shrink_0().child(hdr_label("Account"))
            });
        }
        if show_io_cols {
            header_row = header_row
                .child(hdr_div())
                .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(hdr_label("In")))
                .child(hdr_div())
                .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(hdr_label("Out")));
        }
        header_row = header_row.child(hdr_div()).child(div().w(px(status_w)).flex_shrink_0().flex().justify_end().child(hdr_label("Status")));

        let sel_bg = style::c(style::selection_ground(p, p.list_background, true));
        let sel_fg = style::c(style::selection_text(p, p.panel_text, true));
        let mut rows = div().w_full().pb(px(4.0)).flex().flex_col();

        let col_gap = || div().w(px(1.0)).flex_shrink_0();

        for o in live {
            let selected = matches!(&self.activity_picked, Some(Pick::Live(k)) if k == &o.key);
            let fg = if selected { sel_fg } else { style::c(p.panel_text) };
            let dim_fg = if selected { sel_fg } else { sec_list };
            let start_clock = if o.started_ms > 0 { clock(o.started_ms / 1000) } else { "now".into() };
            let key = o.key.clone();
            let mut line = div()
                .w_full()
                .pl(px(m::TIGHT))
                .pr(px(m::TIGHT))
                .py(px(3.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::Pick(Pick::Live(key.clone())))));
            if selected {
                line = line.bg(sel_bg);
            }
            line = line.child(div().w(px(time_w)).flex_shrink_0().child(StringView::new(start_clock).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            if show_machine_col {
                line = line.child(col_gap()).child(
                    div()
                        .w(px(machine_w))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(o.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(o.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            let model_cell = div()
                .pr(px(8.0))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch_dot(provider_color(p, o.provider), 6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(o.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
            line = line.child(col_gap()).child(if wide_account_flex { model_cell.w(px(176.0)).flex_shrink_0() } else { model_cell.flex_1().min_w(px(116.0)) });
            if show_account_col {
                let acct_txt = StringView::new(o.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW);
                line = line.child(col_gap()).child(if wide_account_flex {
                    div().flex_1().min_w(px(96.0)).overflow_hidden().child(acct_txt)
                } else {
                    div().w(px(account_w)).flex_shrink_0().overflow_hidden().child(acct_txt)
                });
            }
            if show_io_cols {
                line = line
                    .child(col_gap())
                    .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)))
                    .child(col_gap())
                    .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            }
            let elapsed = if o.started_ms > 0 { format!("Running · {}", duration((t_ms - o.started_ms).max(0) as u64)) } else { "Running".to_owned() };
            line = line
                .child(col_gap())
                .child(div().w(px(status_w)).flex_shrink_0().overflow_hidden().flex().justify_end().child(StringView::new(elapsed).size(style::SMALL_SIZE).color(fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        for r in recent {
            let selected = matches!(&self.activity_picked, Some(Pick::Recent(pk)) if pk.key == r.key);
            let fg = if selected { sel_fg } else { style::c(p.panel_text) };
            let dim_fg = if selected { sel_fg } else { sec_list };
            let (status_text, status_fg) = match r.request.outcome {
                Outcome::Completed => (duration(r.request.ms), dim_fg),
                Outcome::Failed if r.request.status != 0 => (format!("Failed · {}", r.request.status), if selected { sel_fg } else { style::c(p.failure) }),
                other => (other.label().to_owned(), if selected { sel_fg } else { outcome_color(p, other) }),
            };
            let picked = r.clone();
            let mut line = div()
                .w_full()
                .pl(px(m::TIGHT))
                .pr(px(m::TIGHT))
                .py(px(3.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::Pick(Pick::Recent(picked.clone())))));
            if selected {
                line = line.bg(sel_bg);
            }
            line = line.child(div().w(px(time_w)).flex_shrink_0().child(StringView::new(clock(r.request.at)).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            if show_machine_col {
                line = line.child(col_gap()).child(
                    div()
                        .w(px(machine_w))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(r.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(r.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            let model_cell = div()
                .pr(px(8.0))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch_dot(provider_color(p, r.provider), 6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(r.request.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
            line = line.child(col_gap()).child(if wide_account_flex { model_cell.w(px(176.0)).flex_shrink_0() } else { model_cell.flex_1().min_w(px(116.0)) });
            if show_account_col {
                let acct_txt = StringView::new(r.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW);
                line = line.child(col_gap()).child(if wide_account_flex {
                    div().flex_1().min_w(px(96.0)).overflow_hidden().child(acct_txt)
                } else {
                    div().w(px(account_w)).flex_shrink_0().overflow_hidden().child(acct_txt)
                });
            }
            if show_io_cols {
                let in_s = if r.request.usage.input_tokens > 0 { tokens(r.request.usage.input_tokens) } else { "—".into() };
                let out_s = if r.request.usage.output_tokens > 0 { tokens(r.request.usage.output_tokens) } else { "—".into() };
                line = line
                    .child(col_gap())
                    .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new(in_s).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)))
                    .child(col_gap())
                    .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new(out_s).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            }
            line = line
                .child(col_gap())
                .child(div().w(px(status_w)).flex_shrink_0().overflow_hidden().flex().justify_end().child(StringView::new(status_text).size(style::SMALL_SIZE).color(status_fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        if live.is_empty() && recent.is_empty() {
            let empty_msg = format!("No requests in the {}.", span_name.to_ascii_lowercase());
            rows = rows.child(div().w_full().py(px(32.0)).flex().justify_center().child(StringView::new(empty_msg).color(sec_list)));
        }

        let well_frame = canvas(|g, b| {
            let p = theme::palette();
            style::sunken(g, b, &p, false);
        })
        .paint_key(style::key("recent-well"))
        .absolute()
        .inset_0();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .p(px(2.0))
            .bg(style::c(p.list_background))
            .flex()
            .flex_col()
            .child(header_row)
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.activity_scroll).border(false).background(p.list_background).child(rows)))
            .child(well_frame)
            .into_element()
    }

    // ---- Tab 3: Machines --------------------------------------------------

    fn machines_tab(&self, pool: &Pool, hosts: &[Stands], running: Option<&Running>, uses: Option<[bool; 3]>, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let vp = cx.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);
        let narrow = vp_w < 480.0;
        let compact_h = vp_h < 520.0;
        let tight_h = vp_h < 290.0;
        let sec_panel = secondary_ink(p, p.panel_background);

        // Sort remote hosts by severity FIRST: errors -> in progress -> healthy -> not installed!
        let ordered_hosts = sorted_hosts(hosts);
        let local_section = self.this_computer_section(pool, &ordered_hosts, running, uses, narrow, compact_h, tight_h, p, cx);
        let sel_host = self.selected_host.as_deref().filter(|id| ordered_hosts.iter().any(|h| h.id == *id)).or_else(|| ordered_hosts.first().map(|h| h.id.as_str()));
        let sel_stands = sel_host.and_then(|id| ordered_hosts.iter().find(|h| h.id == id));

        let left_w = if vp_w >= 860.0 { (vp_w * 0.34).clamp(240.0, 360.0) } else { (vp_w * 0.38).clamp(212.0, 300.0) };
        let hosts_col_w = if compact_h { vp_w - left_w - 44.0 } else { vp_w - 28.0 };

        // Remote Rex Hosts heading + red error count matching Activity's red failures
        let err_n = ordered_hosts.iter().filter(|h| h.error.is_some()).count();
        let mut count_row = div().flex().flex_row().items_center();
        if !ordered_hosts.is_empty() {
            let mut cfg_txt = format!("{} configured", ordered_hosts.len());
            if err_n > 0 {
                cfg_txt.push_str(" · ");
            }
            count_row = count_row.child(StringView::new(cfg_txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(m::LABEL_ROW));
            if err_n > 0 {
                let err_txt = if err_n == 1 { "1 error".to_owned() } else { format!("{err_n} errors") };
                count_row = count_row.child(StringView::new(err_txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(style::c(p.failure)).height(m::LABEL_ROW));
            }
        }
        let hosts_hdr = div().w_full().flex().flex_row().items_center().justify_between().child(StringView::new("Remote Rex Hosts").bold(true).height(m::LABEL_ROW)).child(count_row);

        // ONE sunken framed list well for Remote Rex Hosts (takes all remaining height!)
        let wide = ["Sign In…", "Retry Now", "Sync Now", "Install", "Try Again"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
        let mut host_list = div().w_full().flex().flex_col();
        if !self.hosts.carried() && !demo_enabled() {
            host_list = host_list.child(
                div()
                    .w_full()
                    .p(px(10.0))
                    .child(StringView::new("This Olo was built without the Token Pool for hosts, so none can be installed.").size(style::SMALL_SIZE).wrap(true).color(style::c(p.warning))),
            );
        }
        if ordered_hosts.is_empty() {
            let top_pad = if compact_h { 20.0 } else { 96.0 };
            host_list = host_list.child(
                div()
                    .w_full()
                    .pt(px(top_pad))
                    .pb(px(24.0))
                    .px(px(24.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon_box("network", 48.0))
                    .child(div().pt(px(8.0)).child(StringView::new("No remote hosts yet").bold(true).height(m::LABEL_ROW)))
                    .child(
                        div()
                            .pt(px(4.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(StringView::new("Click Add Host… to install Token Pool on a Rex host").size(style::SMALL_SIZE).color(secondary_ink(p, p.list_background)).height(DETAIL_ROW))
                            .child(StringView::new("so your machines share accounts.").size(style::SMALL_SIZE).color(secondary_ink(p, p.list_background)).height(DETAIL_ROW)),
                    ),
            );
        } else {
            for (idx, h) in ordered_hosts.iter().enumerate() {
                let kept = kept_names(pool, Some(&h.id));
                let is_sel = sel_host == Some(h.id.as_str());
                let row = self.remote_host_row(h, &kept, is_sel, wide, hosts_col_w, p, cx);
                host_list = host_list.child(if idx > 0 { under_rule(p, row) } else { row });
            }
        }
        let hosts_well = div().flex_1().min_h_0().child(ScrollView::new(&self.hosts_scroll).background(p.list_background).child(host_list));

        // Consistent footer matching Accounts order & labels: [Add Host…] [Remove Host…] … Updated HH:MM [Sync All]
        let any_syncable = ordered_hosts.iter().any(|h| h.wanted && h.doing.is_none());
        let asking_this = sel_host.is_some() && self.host_asking_remove.as_deref() == sel_host;
        let can_remove = !asking_this && sel_stands.is_some_and(|h| h.wanted && h.doing.is_none());
        let rm_id = sel_stands.map(|h| h.id.clone()).unwrap_or_default();
        let checked_text = last_checked_at(&ordered_hosts).map(|at| format!("Updated {}", clock(at))).unwrap_or_default();

        let mut status_box = div().flex_1().min_w_0().flex().justify_end();
        if !checked_text.is_empty() && hosts_col_w >= 440.0 {
            status_box = status_box.child(StringView::new(checked_text).color(sec_panel).height(m::CONTROL_HEIGHT));
        }

        let footer = div()
            .w_full()
            .h(px(m::CONTROL_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(if compact_h { 6.0 } else { m::SPACING }))
            .child(Button::new("Add Host…").id("pool-add-host").on_click(|_: &ClickEvent, _| olo_kit::app_menu::open_settings("rex")))
            .child(Button::new("Remove Host…").id("pool-remove-host").enabled(can_remove).on_click(cx.msg::<ClickEvent>(WinMsg::HostAskRemove(Some(rm_id)))))
            .child(status_box)
            .child(Button::new("Sync All").id("pool-sync-all").enabled(any_syncable).on_click(cx.msg::<ClickEvent>(WinMsg::SyncAllHosts)));

        if compact_h {
            // In short tiles (quarter / horizontal third), both columns share one top heading line
            // with a generous 20pt gutter, and host buttons sit right beneath Remote Rex Hosts!
            let hosts_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_well).child(footer);
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .gap(px(20.0))
                .child(div().relative().w(px(left_w)).flex_shrink_0().h_full().flex().flex_col().child(canvas(olo_kit::review::own_grid).paint_key(0).absolute().inset_0()).child(local_section))
                .child(hosts_col)
                .into_element();
        }

        let hosts_section = div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_well);

        div().flex_1().min_h_0().flex().flex_col().gap(px(10.0)).child(div().w_full().pb(px(10.0)).child(local_section)).child(hosts_section).child(footer).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn this_computer_section(
        &self,
        pool: &Pool,
        hosts: &[Stands],
        running: Option<&Running>,
        uses: Option<[bool; 3]>,
        narrow: bool,
        compact_h: bool,
        tight_h: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let sec = secondary_ink(p, p.panel_background);
        // Unified left column matching Remote Rex Hosts below: 13pt gutter + 5pt gap = 18pt exact!
        const INDENT: f32 = 18.0;

        let port = self.effective_port();
        let (gw_lamp, gw_status, gw_err, gw_detail) = match &pool.error {
            Some(e) => {
                let blamed = error_port(e, port);
                let reason = if e.to_ascii_lowercase().contains("address already in use") { format!("Port {blamed} is in use by another app.") } else { e.clone() };
                (Light::Down, "Stopped".to_owned(), true, Some(reason))
            }
            None if compact_h => (Light::On, format!("127.0.0.1:{port}"), false, None),
            None => (Light::On, format!("Listening on 127.0.0.1:{port}"), false, None),
        };

        // Bold section heading matching "Remote Rex Hosts" (same x-offset, no BBox frame!)
        let hdr = div().w_full().flex().flex_row().items_center().justify_between().gap(px(8.0)).child(StringView::new("This Computer").bold(true).height(m::LABEL_ROW));

        // 1. Gateway row + named account custody (wrapped cleanly at " · " when narrow/compact)
        let mut gw_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .h(px(20.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(light(gw_lamp)))
                .child(StringView::new("Gateway").bold(true).height(DETAIL_ROW))
                .child(StringView::new("·").color(sec).height(DETAIL_ROW))
                .child(div().flex_1().min_w_0().child(StringView::new(gw_status).color(if gw_err { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW))),
        );
        if let Some(reason) = gw_detail {
            gw_block = gw_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(reason).size(style::SMALL_SIZE).color(sec).wrap(true)));
        } else if !tight_h && let Some(custody_line) = local_custody_summary(pool, hosts) {
            if (narrow || compact_h)
                && let Some((left, right)) = custody_line.split_once(" · ")
            {
                let cap_right = sentence(right);
                gw_block = gw_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(left.to_owned()).size(style::SMALL_SIZE).color(sec).wrap(true)))
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(cap_right).size(style::SMALL_SIZE).color(sec).wrap(true)));
            } else {
                gw_block = gw_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(custody_line).size(style::SMALL_SIZE).color(sec).wrap(true)));
            }
        }

        // 2. Client routing toggles grouped under a concise "Use the pool" label so
        //    quarter & horizontal-half never cramp before the right-aligned running count.
        let use_pool_hdr = div().w_full().child(StringView::new("Use the pool").size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));

        let claude_on = uses.is_some_and(|c| c[client_index(Provider::Claude)]);
        let claude_procs = running.map(|r| r.claude.as_slice()).unwrap_or(&[]);
        let claude_count = match claude_procs.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };
        let status_cell = |txt: String, col: Hsla| {
            div().w(px(58.0)).flex_shrink_0().flex().justify_end().child(StringView::new(txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(col).height(m::MARK_ROW))
        };
        let mut claude_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(div().flex_1().min_w_0().child(CheckBox::new("Claude Code", claude_on).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                    if !demo_enabled() {
                        olo_kit::proxy().send(Msg::SetClient(Provider::Claude, *on));
                    }
                })))
                .child(status_cell(claude_count, sec)),
        );
        if !tight_h {
            for proc in claude_procs {
                let Process { started, cwd, connections, .. } = proc;
                let raw_place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let place = if narrow { middle_truncate(&raw_place, 18) } else { raw_place };
                let wrong = claude_on && connections.direct > 0 && connections.pool == 0;
                let line = if wrong { format!("{place} · bypassing pool · since {}", clock(*started)) } else { format!("{place} · since {}", clock(*started)) };
                claude_block = claude_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(if wrong { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW)));
            }
        }

        // 3. Codex client row: quiet healthy row (no permanent Restart button, no right-hand dot!).
        // Show Restart inline ONLY when daemon needs attention (stale / misrouted); otherwise in Machines menu.
        let codex_on = uses.is_some_and(|c| c[client_index(Provider::Codex)]);
        let daemon = running.and_then(|r| r.daemon.as_ref());
        let needs_restart = daemon.is_some_and(|d| d.stale || (codex_on && d.connections.direct > 0 && d.connections.pool == 0));
        let codex_check =
            CheckBox::new("Codex", codex_on).height(if needs_restart && !narrow && !compact_h { m::CONTROL_HEIGHT } else { m::MARK_ROW }).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SetClient(Provider::Codex, *on));
                }
            });

        let mut codex_block = div().w_full().flex().flex_col().gap(px(2.0));
        match daemon {
            None => {
                codex_block = codex_block.child(
                    div().w_full().flex().flex_row().items_center().justify_between().gap(px(12.0)).child(div().flex_1().min_w_0().child(codex_check)).child(status_cell("Stopped".to_owned(), sec)),
                );
            }
            Some(d) => {
                // Consistent noun matching Claude Code above ("1 running", "2 running", "Idle")
                let conn_count = match d.connections.pool + d.connections.direct {
                    0 => "Idle".to_owned(),
                    n => format!("{n} running"),
                };
                if needs_restart && !narrow && !compact_h {
                    codex_block = codex_block.child(
                        div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().gap(px(8.0)).child(div().flex_1().min_w_0().child(codex_check)).child(
                            Button::new(if self.restarting_codex { "Restarting…" } else { "Restart" })
                                .id("pool-restart-codex")
                                .enabled(!self.restarting_codex)
                                .on_click(cx.msg::<ClickEvent>(WinMsg::RestartCodex)),
                        ),
                    );
                } else {
                    codex_block = codex_block.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(div().flex_1().min_w_0().child(codex_check))
                            .child(status_cell(conn_count, if needs_restart { style::c(p.failure) } else { sec })),
                    );
                }
                if !tight_h {
                    if d.stale {
                        codex_block = codex_block.child(
                            div()
                                .w_full()
                                .pl(px(INDENT))
                                .child(StringView::new("Started before pool settings changed · restart recommended").size(style::SMALL_SIZE).color(style::c(p.failure)).wrap(true)),
                        );
                    } else if codex_on && d.connections.direct > 0 && d.connections.pool == 0 {
                        codex_block = codex_block
                            .child(div().w_full().pl(px(INDENT)).child(StringView::new("Service bypassing pool · restart recommended").size(style::SMALL_SIZE).color(style::c(p.failure)).wrap(true)));
                    } else if let Ok(threads) = &d.threads
                        && !threads.is_empty()
                    {
                        for th in threads {
                            let title = if th.title.is_empty() { "A conversation".to_owned() } else { format!("“{}”", th.title) };
                            let line = match th.cwd.as_deref() {
                                Some(dir) => {
                                    let f = folder(dir);
                                    let place = if narrow { middle_truncate(&f, 18) } else { f };
                                    format!("{place} · since {} · {title}", clock(d.started))
                                }
                                None => format!("since {} · {title}", clock(d.started)),
                            };
                            codex_block = codex_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)));
                        }
                    } else {
                        codex_block = codex_block.child(
                            div()
                                .w_full()
                                .pl(px(INDENT))
                                .child(StringView::new(format!("Service running · since {}", clock(d.started))).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)),
                        );
                    }
                    if let Some((msg, err)) = &self.codex_note {
                        codex_block =
                            codex_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(msg.clone()).size(style::SMALL_SIZE).color(if *err { style::c(p.failure) } else { sec }).wrap(true)));
                    }
                    if needs_restart && (narrow || compact_h) {
                        codex_block = codex_block.child(
                            div().w_full().pt(px(4.0)).flex().justify_end().child(
                                Button::new(if self.restarting_codex { "Restarting…" } else { "Restart" })
                                    .id("pool-restart-codex")
                                    .enabled(!self.restarting_codex)
                                    .on_click(cx.msg::<ClickEvent>(WinMsg::RestartCodex)),
                            ),
                        );
                    }
                }
            }
        }

        // 4. Antigravity client row + running `agy` processes (matches Claude Code above)
        let agy_on = uses.is_some_and(|c| c[client_index(Provider::Antigravity)]);
        let agy_procs = running.map(|r| r.agy.as_slice()).unwrap_or(&[]);
        let agy_count = match agy_procs.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };
        let mut agy_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(div().flex_1().min_w_0().child(CheckBox::new("Antigravity", agy_on).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                    if !demo_enabled() {
                        olo_kit::proxy().send(Msg::SetClient(Provider::Antigravity, *on));
                    }
                })))
                .child(status_cell(agy_count, sec)),
        );
        if !tight_h {
            for proc in agy_procs {
                let Process { started, cwd, connections, .. } = proc;
                let raw_place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let place = if narrow { middle_truncate(&raw_place, 18) } else { raw_place };
                let wrong = agy_on && connections.direct > 0 && connections.pool == 0;
                let line = if wrong { format!("{place} · bypassing pool · since {}", clock(*started)) } else { format!("{place} · since {}", clock(*started)) };
                agy_block = agy_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(if wrong { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW)));
            }
        }

        let mut content = div().w_full().flex().flex_col().gap(px(if tight_h { 4.0 } else { 7.0 })).child(div().w_full().pb(px(if tight_h { 1.0 } else { 5.0 })).child(gw_block));
        if !tight_h {
            content = content.child(use_pool_hdr);
        }
        content = content.child(claude_block).child(codex_block).child(agy_block);
        if let Some((note, err)) = &self.told.note {
            content = content.child(StringView::new(note.clone()).size(style::SMALL_SIZE).color(if *err { style::c(p.failure) } else { sec }).wrap(true));
        }

        div().w_full().flex().flex_col().gap(px(6.0)).child(hdr).child(content).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_host_row(&self, s: &Stands, kept: &[String], selected: bool, wide: f32, hosts_col_w: f32, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let sec = secondary_ink(p, p.list_background);
        let (lamp, state) = host_stands(s);
        // Busy indicator uses crisp neutral 3/4-arc spinner (never hollow "off" circle or orange!)
        let lamp_el = lamp.map_or_else(|| spinner_arc(LIGHT), light);
        let asking = self.host_asking_remove.as_deref() == Some(s.id.as_str());

        // State-driven quiet row buttons: ONLY show a row button when action is actually needed!
        // Healthy hosts have ZERO noisy buttons (double-click / footer / right-click drive actions).
        let row_btn: Option<Button> = if asking || s.doing.is_some() || (s.hopeless && !s.wanted) {
            None
        } else if s.wanted && s.error.is_some() {
            Some(Button::new("Retry Now").id(format!("pool-host-{}-retry", s.id)).min_width(wide).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "sync"))))
        } else if s.wanted && !s.in_step {
            Some(Button::new("Sync Now").id(format!("pool-host-{}-sync", s.id)).min_width(wide).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "sync"))))
        } else if !s.wanted {
            Some(
                Button::new(if s.error.is_some() { "Try Again" } else { "Install" })
                    .id(format!("pool-host-{}-install", s.id))
                    .min_width(wide)
                    .enabled(self.hosts.carried() || demo_enabled())
                    .on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "install"))),
            )
        } else {
            None
        };

        // No redundant cluster icon on every row: status dot in 13pt gutter + 5pt gap = 18pt left edge,
        // so host name and sub-lines share ONE crisp alignment matching Gateway above!
        let state_color = if s.error.is_some() && s.doing.is_none() { style::c(p.failure) } else { sec };
        let title_group = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(lamp_el))
            .child(StringView::new(s.name.clone()).bold(true).height(DETAIL_ROW))
            .child(StringView::new("·").color(sec).height(DETAIL_ROW))
            .child(div().flex_1().min_w_0().child(StringView::new(state).color(state_color).cut(true).height(DETAIL_ROW)));

        // Width-driven layout: keep action button inline whenever the hosts column has ≥360pt
        // (including horizontal third!), wrapping only in genuinely narrow columns.
        let narrow_row = hosts_col_w < 360.0;
        let line1 = div().w_full().h(px(if narrow_row { 20.0 } else { m::CONTROL_HEIGHT })).flex().flex_row().items_center().child(title_group);
        let (right_btn, wrapped_btn) = match row_btn {
            Some(btn) if narrow_row => (None, Some(btn)),
            Some(btn) => (Some(btn), None),
            None => (None, None),
        };

        // Sub-lines aligned straight under the host name (18pt exact!)
        let mut under = div().w_full().pl(px(18.0)).flex().flex_col().gap(px(3.0));
        if asking {
            under = under.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                olo_kit::tr!("Remove Token Pool from {}?", s.name).to_string(),
                "Claude\u{a0}Code, Codex, and Antigravity on this host will return to their own sign-ins.".into(),
                Some("Any sign-ins kept there are copied back to This Computer first.".into()),
                Button::new("Cancel").id(format!("pool-host-{}-cancel", s.id)).on_click(cx.msg::<ClickEvent>(WinMsg::HostAskRemove(None))),
                Button::new("Remove").id(format!("pool-host-{}-confirm-remove", s.id)).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "remove"))),
            )));
        } else {
            under = under.child(StringView::new(s.address.clone()).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
            if s.doing.is_some() {
                under = under.child(div().w_full().h(px(14.0)).flex().items_center().child(barber_bar(if narrow_row { 140.0 } else { 200.0 }, 8.0, neutral_bar_color(p))));
            } else {
                let refused = self.host_refused.as_ref().filter(|(id, _)| *id == s.id).map(|(_, why)| why.clone());
                match (refused.or_else(|| s.error.clone()), s.wanted) {
                    // Red ONLY on the dot & "Can't sync"; explanation stays readable secondary text with consistent punctuation!
                    (Some(why), true) => {
                        let msg = if why.contains("sign-in") { why } else { format!("{} · Olo keeps trying.", host_brief_sentence(&why)) };
                        under = under.child(StringView::new(msg).size(style::SMALL_SIZE).color(sec).wrap(true));
                    }
                    (Some(why), false) => {
                        under = under.child(StringView::new(why).size(style::SMALL_SIZE).color(sec).wrap(true));
                    }
                    (None, _) => {
                        if let Some(status) = &s.status {
                            let summary = host_summary(status, kept, s.heard_at, hosts_col_w < 460.0);
                            under = under.child(StringView::new(summary).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
                        }
                    }
                }
            }
            if let Some(btn) = wrapped_btn {
                under = under.child(div().w_full().pt(px(6.0)).flex().justify_end().child(btn));
            }
        }

        // Clear selection tint when inline Remove confirmation is open (no yellow card on blue tint!)
        let sel_tint = if selected && !asking {
            Some(if p.mode == olo_theme::ThemeMode::Dark {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.16))
            } else {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.09))
            })
        } else {
            None
        };
        let id_click = s.id.clone();
        let can_jump = s.wanted || s.status.is_some();
        let id_ctx = s.id.clone();
        let content_col = div().flex_1().min_w_0().flex().flex_col().child(line1).child(under);
        let mut row = div()
            .w_full()
            .px(px(10.0))
            .py(px(ROW_PAD))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| {
                    if e.click_count >= 2 && can_jump {
                        cx.send(WinMsg::ViewHostActivity(id_click.clone()));
                    } else {
                        cx.send(WinMsg::SelectHost(id_click.clone()));
                    }
                }),
            )
            .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| cx.send(WinMsg::HostContextMenu(id_ctx.clone(), e.position))))
            .child(content_col);
        if let Some(btn) = right_btn {
            row = row.child(btn);
        }
        if let Some(bg) = sel_tint {
            row = row.bg(bg);
        }
        row.into_element()
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
            WinMsg::Tick => self.schedule_tick(cx),
            WinMsg::HostsTick => {
                if !demo_enabled() {
                    self.hosts.refresh();
                    cx.after(HOSTS_POLL_EVERY, WinMsg::HostsTick);
                }
            }
            WinMsg::Tab(tab) => self.switch_tab(tab, cx),
            WinMsg::SelectAccount(id) => self.selected_account = Some(id),
            WinMsg::SelectHost(id) => self.selected_host = Some(id),
            WinMsg::ViewHostActivity(host_id) => {
                if !host_id.is_empty() {
                    self.scope = MachineScope::Host(host_id);
                    self.activity_picked = None;
                    self.switch_tab(Tab::Activity, cx);
                }
            }
            WinMsg::ScopeIndex(idx) => {
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let opts = Self::scope_options(&hosts);
                if let Some((sc, _)) = opts.get(idx) {
                    self.scope = sc.clone();
                    self.activity_picked = None;
                }
            }
            WinMsg::Span(i) => self.span_idx = i.min(SPANS.len() - 1),
            WinMsg::Pick(pick) => {
                self.activity_picked = if self.activity_picked.as_ref() == Some(&pick) { None } else { Some(pick) };
            }
            WinMsg::ClearPick => self.activity_picked = None,
            WinMsg::AddMenu(at) => {
                let chosen = cx.msg_with(|command: &str| WinMsg::Account(String::new(), command.to_owned()));
                menu::popup(cx, add_menu(), at, Corner::BottomLeft, chosen);
                return;
            }
            WinMsg::AccountMenu(id, at) => {
                let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
                let Some(a) = pool.account(&id).cloned() else { return };
                let m = Self::account_menu_items(&a);
                let chosen = cx.msg_with(move |command: &str| WinMsg::Account(id.clone(), command.to_owned()));
                menu::popup(cx, m, at, Corner::BottomLeft, chosen);
                return;
            }
            WinMsg::AccountContextMenu(id, pos) => {
                self.selected_account = Some(id.clone());
                let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
                let Some(a) = pool.account(&id).cloned() else { return };
                let m = Self::account_menu_items(&a);
                let chosen = cx.msg_with(move |command: &str| WinMsg::Account(id.clone(), command.to_owned()));
                menu::context_menu(cx, m, pos, chosen);
                return;
            }
            WinMsg::HostContextMenu(id, pos) => {
                self.selected_host = Some(id.clone());
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let Some(h) = hosts.iter().find(|x| x.id == id) else { return };
                let mut m = Menu::new(&h.name);
                if h.wanted || h.status.is_some() {
                    m = m.item("Show Activity", "activity");
                }
                if h.wanted && h.doing.is_none() {
                    m = m.item("Sync Now", "sync").separator().item("Remove…", "remove");
                } else if !h.wanted && !h.hopeless && h.doing.is_none() {
                    m = m.item("Install Token Pool", "install");
                }
                let chosen = cx.msg_with(move |cmd: &str| match cmd {
                    "activity" => WinMsg::ViewHostActivity(id.clone()),
                    "sync" => WinMsg::HostDo(id.clone(), "sync"),
                    "install" => WinMsg::HostDo(id.clone(), "install"),
                    "remove" => WinMsg::HostAskRemove(Some(id.clone())),
                    _ => WinMsg::Changed,
                });
                menu::context_menu(cx, m, pos, chosen);
                return;
            }
            WinMsg::Account(id, command) if id.is_empty() => olo_kit::proxy().send(Msg::Command(command)),
            WinMsg::Account(id, command) => self.account_command(&id, &command, cx),
            WinMsg::Renamed(rename) => {
                if let Some((id, field)) = self.renaming.take()
                    && rename
                {
                    let name = field.read(cx).text().trim().to_owned();
                    if !name.is_empty() && !demo_enabled() {
                        self.shared.rename(&id, &name);
                    }
                }
            }
            WinMsg::RemoveAccount(id) => {
                if let Some(id) = id
                    && !demo_enabled()
                {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    olo_kit::proxy().send(Msg::Done(self.shared.remove(&id).map(|()| format!("{name} left the pool."))));
                }
                self.removing_account = None;
            }
            WinMsg::ResetCredit(id) => {
                if let Some(id) = id
                    && !demo_enabled()
                {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    let (shared, proxy) = (self.shared.clone(), olo_kit::proxy());
                    self.shared.spawn(async move {
                        let r = shared.redeem(&id).await.map(|()| format!("{name}’s quota was reset."));
                        proxy.send(Msg::Done(r));
                    });
                }
                self.resetting_credit = None;
            }
            WinMsg::FoundRunning(r) => self.running = Some(*r),
            WinMsg::RestartCodex => {
                if !demo_enabled() {
                    self.restarting_codex = true;
                    self.codex_note = None;
                    cx.work(|| codex_daemon::restart_now().map_err(|e| e.to_string()), WinMsg::CodexRestarted);
                }
            }
            WinMsg::CodexRestarted(r) => {
                self.restarting_codex = false;
                self.codex_note = Some(match r {
                    Ok(pid) => (format!("Restarted Codex service (pid {pid})."), false),
                    Err(e) => (e, true),
                });
            }
            WinMsg::HostAskRemove(host) => self.host_asking_remove = host,
            WinMsg::HostDo(id, what) => {
                self.host_asking_remove = None;
                if !demo_enabled() {
                    self.host_refused = self.hosts.act(&id, what).err().map(|why| (id, why));
                }
            }
            WinMsg::SyncAllHosts => {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::Command("sync-hosts".into()));
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        match name {
            "close" => cx.close_window(),
            "refresh" => {
                if !demo_enabled() {
                    self.shared.sync_soon(None);
                    self.hosts.refresh();
                }
            }
            "capture" => {
                if !demo_enabled() {
                    self.shared.capture(true);
                }
            }
            "restart-codex" => {
                self.message(WinMsg::RestartCodex, cx);
            }
            "show-host-activity" => {
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let ordered = sorted_hosts(&hosts);
                let sel = self.selected_host.as_deref().filter(|id| ordered.iter().any(|h| h.id == *id)).or_else(|| ordered.iter().find(|h| h.wanted || h.status.is_some()).map(|h| h.id.as_str()));
                if let Some(id) = sel.map(str::to_owned) {
                    self.message(WinMsg::ViewHostActivity(id), cx);
                }
            }
            "tab-accounts" => {
                self.switch_tab(Tab::Accounts, cx);
                cx.invalidate();
            }
            "tab-activity" | "clients" => {
                self.switch_tab(Tab::Activity, cx);
                cx.invalidate();
            }
            "tab-machines" | "hosts" => {
                self.switch_tab(Tab::Machines, cx);
                cx.invalidate();
            }
            _ => olo_kit::proxy().send(Msg::Command(name.to_owned())),
        }
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.stop_scanner();
        olo_kit::proxy().send(Msg::Closed);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let p = &*p;
        let (pool, activity, hosts, running, uses, unaccepted) = self.snapshot();
        let lights = tab_lights(&pool, &hosts, running.as_ref(), uses);

        let page = match self.tab {
            Tab::Accounts => self.accounts_tab(&pool, &hosts, &unaccepted, p, cx),
            Tab::Activity => self.activity_tab(&pool, &activity, &hosts, p, cx),
            Tab::Machines => self.machines_tab(&pool, &hosts, running.as_ref(), uses, p, cx),
        };

        let tabs = TabView::new("pool-tabs", vec!["Accounts".into(), "Activity".into(), "Machines".into()], self.tab as usize)
            .lights(lights)
            .on_select(cx.msg_with(|i: &usize| WinMsg::Tab(Tab::from_index(*i))))
            .child(page);

        let vp = cx.viewport_size();
        let outer_pad = if f32::from(vp.height) < 340.0 || f32::from(vp.width) < 460.0 { 6.0 } else { 10.0 };
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let mut content = div().flex_1().min_h_0().p(px(outer_pad)).flex().flex_col().gap(px(10.0));
        if let Some(err) = pool.error.as_deref() {
            content = content.child(gateway_stopped_alert(p, self.effective_port(), err, cx));
        }
        content = content.child(div().flex_1().min_h_0().child(tabs));
        div().size_full().flex().flex_col().child(bar).child(content)
    }
}

pub fn start(app: &mut AppCx, hub: Hub, service: bool) {
    let restored: Option<serde_json::Value> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_tab = requested_tab(&args);
    let restored_tab = restored.as_ref().and_then(|r| {
        if let Some(i) = r["tab"].as_u64() {
            Some(Tab::from_index(i as usize))
        } else if r["hosts"] == true {
            Some(Tab::Machines)
        } else if r["clients"] == true {
            Some(Tab::Activity)
        } else {
            None
        }
    });
    let open = match &restored {
        Some(r) => r["open"] == true || r["hosts"] == true || r["clients"] == true,
        None => !service || arg_tab.is_some(),
    };
    let (shared, flags, proxy) = (hub.shared.clone(), hub.open.clone(), hub.proxy.clone());
    let hub = Rc::new(RefCell::new(hub));
    if open {
        hub.borrow_mut().open_tab(arg_tab.or(restored_tab));
    }
    olo_kit::message_handler(app, move |message, _| hub.borrow_mut().message(message));
    let saved = flags.clone();
    olo_kit::on_upgrade(move |_| Some(saved.state()));

    let due = shared.clone();
    olo_kit::on_upgrade_due(move || {
        if hand_over(&due, &flags, &proxy) {
            return;
        }
        due.drain.begin();
        let weak = Arc::downgrade(&due);
        let _ = std::thread::Builder::new().name("pool-drain".into()).spawn(move || {
            while let Some(s) = weak.upgrade() {
                if !s.drain.draining() {
                    break;
                }
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

pub const KEPT_ENV: &str = "OLO_POOL_KEPT";
pub const KEEPER_ENV: &str = "OLO_POOL_KEEPER";

fn hand_over(shared: &Arc<Shared>, open: &Open, proxy: &Proxy) -> bool {
    use std::os::unix::process::CommandExt as _;
    let state = open.state();
    let Some(listeners) = shared.drain.for_a_copy() else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args_os().skip(1)).env(olo_token_pool::drain::LISTENERS_ENV, listeners).env(olo_xdg::NEW_INSTANCE, "1").env(KEPT_ENV, "1");
    let finishing = shared.drain.finishing();
    match &finishing {
        Some(fd) => cmd.env(olo_token_pool::drain::FINISHING_ENV, std::os::fd::AsRawFd::as_raw_fd(fd).to_string()),
        None => cmd.env(olo_token_pool::drain::PREDECESSOR_ENV, std::process::id().to_string()),
    };
    olo_kit::restore::hand_over(&mut cmd, Some(&state));
    olo_kit::instance::close();
    let claims = shared.claims_closed(Duration::from_secs(120));
    if claims.is_none() {
        tracing::warn!("pool: a reset claim is still under way after two minutes; handing over beside it");
    }
    shared.flush();
    let started = cmd.spawn();
    drop(finishing);
    shared.drain.sealed();
    let child = match started {
        Ok(c) => c,
        Err(e) => {
            shared.drain.finished();
            tracing::warn!("pool: couldn’t start the new version beside this one ({e}); upgrading the old way");
            olo_kit::instance::listen(crate::APP_ID, proxy.clone());
            return false;
        }
    };
    tracing::info!(pid = child.id(), "pool: the new version takes every new request; finishing what this one has");
    shared.drain.retire();
    drop(claims);
    proxy.send(Msg::CloseWindows);
    let weak = Arc::downgrade(shared);
    let kept = std::env::var_os(KEPT_ENV).is_some();
    let _ = std::thread::Builder::new().name("pool-handed-over".into()).spawn(move || {
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
        if let Some(s) = weak.upgrade() {
            s.drain.finished();
        }
        if kept {
            tracing::info!("pool: finished; the new version has it all");
            olo_kit::restore::mark_waiting(false);
            std::process::exit(0);
        }
        tracing::info!("pool: finished; staying as the keeper of the new version");
        let e = std::process::Command::new(&exe).env(KEEPER_ENV, "1").env_remove(KEPT_ENV).exec();
        tracing::warn!("pool: couldn’t become the keeper ({e}); ending");
        std::process::exit(0);
    });
    drop(child);
    true
}
