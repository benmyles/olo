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
    AppCx, Bounds, Child, ClickEvent, Corner, Cx, Element, Hsla, InteractiveElement as _, IntoElement as _, Menu, MenuBar, MouseButton, ParentElement as _, Pixels, Proxy, ScrollHandle, SharedString,
    Styled as _, View, WindowHandle, WindowOptions, canvas, div, icons, menu, metrics as m, palette as olo_theme, point, px, size, style, theme,
    widgets::{
        self, Button, CheckBox, Graph, LIGHT, Light, MenuField, ScrollView, Series, StringView, TabView, TextControl, TextField, TextFieldEvent, button_width, level_bar, light, separator, text_field,
    },
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
    schedule, store,
};

use crate::{
    PROVIDERS,
    aggregate::{
        AccountRollup, LiveReq, MachineScope, RecentReq, Tab, account_rollup, custody, demo_data, demo_enabled, folder, host_brief_sentence, host_stands, host_summary, hue, kept_names,
        last_checked_at, last_synced_at, live_requests, local_custody_summary, merged_activity, middle_truncate, provider_hue, recent_requests, sentence, sorted_accounts, sorted_hosts,
        sorted_providers, tab_lights,
    },
    client_index, clock, duration, plan_name, span, tokens,
};

const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);
const PROCESS_POLL_EVERY: Duration = Duration::from_secs(2);
const HOSTS_POLL_EVERY: Duration = Duration::from_secs(30);
const GRAPH_HEIGHT: f32 = 152.0;
const DETAIL_ROW: f32 = 16.0;
const RECENT_TABLE_LIMIT: usize = 60;

/// Time spans for the Activity chart: (label, minutes, bucket minutes, grid seconds).
const SPANS: [(&str, i64, i64, f64); 3] = [("Last Hour", 60, 1, 600.0), ("Last 6 Hours", 360, 5, 3600.0), ("Last 24 Hours", 1440, 15, 4.0 * 3600.0)];

/// Messages to [`Hub`].
pub enum Msg {
    Changed,
    Clients([bool; 2]),
    ClientSet(Provider, Result<(), String>, Option<bool>),
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
    clients: Option<[bool; 2]>,
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
    clients: Option<[bool; 2]>,
    note: Option<(String, bool)>,
    waiting: Option<Waiting>,
    open: Arc<Open>,
}

impl Hub {
    pub fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, proxy_port: u16, proxy: Proxy) -> Hub {
        Hub { shared, proxy_port, proxy, win: None, hosts, clients: None, note: None, waiting: None, open: Arc::default() }
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
        let options = WindowOptions::new("Token Pool", 640.0, 660.0).min_size(380.0, 220.0).keep_size("main");
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
        if let Some(c) = self.clients.as_mut() {
            c[client_index(p)] = on;
        }
        self.shared.set_paused(p, !on);
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || {
            let pointed = clients::pointed(p, port);
            let r = clients::set(p, on, port);
            let restarted = p == Provider::Codex && r.is_ok() && matches!(codex_daemon::follow(), Ok(Followed::Restarted(_)));
            proxy.send(Msg::ClientSet(p, r, if pointed { None } else { Some(restarted) }))
        });
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
            Msg::ClientSet(prov, r, restarted) => {
                self.note = Some(match r {
                    Ok(()) => {
                        let on = self.clients.is_some_and(|c| c[client_index(prov)]);
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
        let key = format!("{p:?}").to_lowercase();
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

/// Crisp 3/4-arc busy spinner glyph in neutral secondary slate (replaces hollow "off" circle).
fn spinner_arc(across: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let ink = neutral_bar_color(&p);
        let c = b.center();
        let rad = (across * 0.5 - 0.9).max(2.0);
        let mut path = olo_kit::PathBuilder::stroke(px(1.6));
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

/// BeOS-native full-width Gateway Stopped alert rendered above the tab bar on every tab.
fn gateway_stopped_alert(p: &olo_kit::palette::Palette, port: u16, raw_error: &str, cx: &mut Cx<PoolWindow>) -> Element {
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = if dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.05)) } else { style::c([0xfd, 0xec, 0xec, 0xff]) };
    let border = style::c(p.failure);
    let body = if raw_error.to_ascii_lowercase().contains("address already in use") {
        format!("Port {port} is in use by another app, so Claude\u{a0}Code and Codex on this computer can’t reach the pool.")
    } else {
        raw_error.to_owned()
    };
    div()
        .w_full()
        .p(px(12.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(8.0))
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
    if p.mode == olo_theme::ThemeMode::Dark { style::c([0x8c, 0x9d, 0xb3, 0xff]) } else { style::c([0x5e, 0x6e, 0x84, 0xff]) }
}

/// Compact inline single-row quota meter with fixed right-aligned percentage column
/// and left-aligned reset text (`5-hour | [====] |  42%  resets Thu 00:09 (2h 10m)`).
/// Bar colour uses neutral slate (<80%), true amber (#E3A008, 80–94%), or failure red (≥95%);
/// the percentage text carries the warning/error colour too so state is never colour-bar-only.
fn inline_quota_row(p: &olo_kit::palette::Palette, w: &QuotaWindow, t: i64, narrow: bool) -> Element {
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
    let (bar_color, pct_color) = if w.exhausted(t) || pct >= 95.0 {
        (style::c(p.failure), style::c(p.failure))
    } else if pct >= 80.0 {
        (style::c([0xe3, 0xa0, 0x08, 0xff]), style::c(p.warning))
    } else {
        (neutral_bar_color(p), style::c(p.panel_text))
    };
    let fraction = (w.used / 100.0) as f32;
    let bar_key = style::key((fraction.to_bits(), style::ink_key(Some(bar_color))));
    let bar_el = canvas(move |g, b| {
        let pal = theme::palette();
        level_bar(g, b, fraction, Some(bar_color), &pal);
    })
    .paint_key(bar_key)
    .flex_1()
    .min_w(px(60.0))
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
        .child(div().w(px(48.0)).flex_shrink_0().child(StringView::new(label).size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW)))
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

    fn snapshot(&self) -> (Pool, Activity, Vec<Stands>, Option<Running>, Option<[bool; 2]>, HashMap<String, Vec<String>>) {
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

    fn account_menu_items(a: &Account) -> Menu {
        Menu::new("Account")
            .item("Rename…", "rename")
            .item("Refresh Quota", "refresh")
            .item("Use a Reset Credit…", "reset")
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
        // Sections stay in fixed canonical order (Claude Code, then Codex); problems float to the top within each section.
        let providers = sorted_providers(&pool.saved.accounts, t);
        let first_account_id = providers.first().and_then(|&prov| sorted_accounts(&pool.saved.accounts, prov, t).first().map(|a| a.id.as_str()));
        let sel_id = self.selected_account.as_deref().filter(|id| pool.account(id).is_some()).or(first_account_id);
        let wide = ["Sign In…", "Retry Now", "Sync Now", "Install", "Try Again"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);

        let mut list = div().w_full().flex().flex_col();
        let mut any = false;
        let group_bg = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.list_background, p.panel_background, 0.7)) } else { style::c(p.panel_background) };
        for prov in providers {
            let accounts = sorted_accounts(&pool.saved.accounts, prov, t);
            if accounts.is_empty() {
                continue;
            }
            if any {
                list = list.child(separator());
            }
            any = true;
            // Tinted 22pt group header row with provider colour swatch aligned to checkbox column
            list = list.child(
                div()
                    .w_full()
                    .h(px(22.0))
                    .px(px(10.0))
                    .bg(group_bg)
                    .border_b_1()
                    .border_color(style::c(p.control_border))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(swatch_dot(style::c(p.hue(provider_hue(prov))), 8.0)))
                    .child(StringView::new(prov.client()).size(style::SMALL_SIZE).bold(true).height(DETAIL_ROW)),
            );
            for (idx, account) in accounts.iter().enumerate() {
                if idx > 0 {
                    list = list.child(separator());
                }
                let pending = account.kept_on.as_deref().and_then(|hid| unaccepted.get(hid)).map(Vec::as_slice).unwrap_or(&[]);
                let rollup = account_rollup(pool, account, hosts);
                let is_sel = sel_id == Some(account.id.as_str());
                list = list.child(self.account_row(account, hosts, pending, &rollup, is_sel, wide, narrow, p, cx));
            }
        }
        if !any {
            let top_pad = if compact_h { 24.0 } else { 96.0 };
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
                    .child(div().pt(px(4.0)).flex().flex_col().items_center().child(
                        StringView::new("Click Add Account… to add a Claude\u{a0}Code or Codex subscription.").size(style::SMALL_SIZE).color(secondary_ink(p, p.list_background)).height(DETAIL_ROW),
                    ))
            } else {
                div().w_full().pt(px(top_pad)).pb(px(24.0)).px(px(24.0)).flex().justify_center().child(StringView::new("Opening the pool’s accounts…").color(secondary_ink(p, p.list_background)))
            };
            list = list.child(empty_box);
        }

        let well = div().flex_1().min_h_0().child(ScrollView::new(&self.accounts_scroll).background(p.list_background).child(list));

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
        narrow: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let t = now();
        let sec = secondary_ink(p, p.list_background);
        let cust = custody(account, hosts, unaccepted);
        let is_signed_out = account.signed_out.is_some();
        let asking_inline = self.removing_account.as_deref() == Some(account.id.as_str()) || self.resetting_credit.as_deref() == Some(account.id.as_str());

        // Line 1: checkbox mark + bold account name (+ inline red Signed out / grey Paused)
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
        }

        let mut row_action: Option<Button> = None;
        let line1 = if is_signed_out {
            let (prov, id_sign) = (account.provider, account.id.clone());
            row_action = Some(Button::new("Sign In…").id(format!("pool-signin-{}", account.id)).min_width(wide).on_click(move |_: &ClickEvent, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SignIn(prov, id_sign.clone(), false));
                }
            }));
            div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().child(title_group)
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

        // Line 2+: indented straight under the bold account name (13pt gutter + 5pt gap = 18pt)
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

        let mut body = div().w_full().pl(px(19.0)).flex().flex_col().gap(px(3.0));
        if !facts.is_empty() {
            body = body.child(StringView::new(facts.join(" · ")).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
        }
        if let Some(why) = &account.signed_out {
            // Reason in readable secondary ink (matches Remote Rex Hosts' error rows!)
            body = body.child(StringView::new(why.clone()).size(style::SMALL_SIZE).color(sec).wrap(true));
        } else {
            if let Some(until) = schedule::blocked_until(account, "", t) {
                body = body.child(StringView::new(format!("Out of quota until {} (in {})", clock(until), span(until - t))).size(style::SMALL_SIZE).color(style::c(p.warning)));
            }
            for w in &account.windows {
                body = body.child(inline_quota_row(p, w, t, narrow));
            }
            if account.windows.is_empty() {
                body = body.child(StringView::new(if account.synced.is_some() { "No quota reported." } else { "Reading quota…" }).size(style::SMALL_SIZE).color(sec));
            }
            // Concise Today line (+ red failed count when failures occurred); local/remote breakdown lives on Activity.
            let u = &rollup.total_today;
            let failed_n = rollup.recent.iter().filter(|r| r.request.outcome == Outcome::Failed).count();
            let today_base = if u.requests == 0 {
                "Today: 0 requests".to_owned()
            } else {
                format!("Today: {} request{} · {} in · {} out", u.requests, if u.requests == 1 { "" } else { "s" }, tokens(u.input_tokens), tokens(u.output_tokens))
            };
            let mut today_row = div().w_full().pt(px(2.0)).flex().flex_row().items_center().gap(px(4.0)).child(StringView::new(today_base).size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));
            if failed_n > 0 {
                today_row = today_row
                    .child(StringView::new("·").size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW))
                    .child(StringView::new(format!("{failed_n} failed")).size(style::SMALL_SIZE).color(style::c(p.failure)).height(DETAIL_ROW));
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
                format!("Use one of {}’s reset credits now{when}?", account.name),
                "Its quota starts again immediately, and the credit is spent.".into(),
                None,
                Button::new("Cancel").on_click(cx.msg(WinMsg::ResetCredit(None))),
                Button::new("Use Credit").on_click(cx.msg(WinMsg::ResetCredit(Some(account.id.clone())))),
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
            .py(px(8.0))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::SelectAccount(id_click.clone()))))
            .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| cx.send(WinMsg::AccountContextMenu(id_ctx.clone(), e.position))))
            .child(content_col);
        if let Some(btn) = row_action {
            row = row.child(btn);
        }
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
        let compact_h = vp_h < 420.0;
        let two_col = compact_h && vp_w >= 560.0;
        let sec_panel = secondary_ink(p, p.panel_background);
        let sec_list = secondary_ink(p, p.list_background);

        let opts = Self::scope_options(hosts);
        let scope_idx = opts.iter().position(|(s, _)| s == &self.scope).unwrap_or(0);
        let active_scope = opts[scope_idx].0.clone();
        let scope_labels: Vec<SharedString> = opts.iter().map(|(_, l)| l.clone()).collect();
        // Size "All Machines" popup box (~136pt inside the 196pt labeled control) wider than "Last Hour" (124pt).
        let mut scope_pick = MenuField::new("pool-scope", scope_labels, scope_idx).width(if narrow { 130.0 } else { 196.0 }).on_change(cx.msg_with(|i: &usize| WinMsg::ScopeIndex(*i)));
        if !narrow {
            scope_pick = scope_pick.label("Machine:");
        }

        let (span_name, span_mins, step_mins, grid_secs) = SPANS[self.span_idx];
        let span_pick =
            MenuField::new("pool-span", SPANS.iter().map(|s| s.0.into()).collect(), self.span_idx).width(if narrow { 112.0 } else { 124.0 }).on_change(cx.msg_with(|i: &usize| WinMsg::Span(*i)));

        let t_ms = crate::now_ms();
        let live = live_requests(pool, hosts, &active_scope);
        let live_badge = match live.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };

        let top_bar = div().w_full().flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(scope_pick);

        let picked_line: Option<String> = match &self.activity_picked {
            Some(Pick::Live(key)) => live.iter().find(|o| &o.key == key).map(|o| live_details(o, t_ms)),
            Some(Pick::Recent(r)) => Some(recent_details(r)),
            None => None,
        };

        // Requests per Minute chart: two crisp lines (no overlapping muddy wash),
        // bare numeric Y-axis gutter with tight headroom, and X-axis clock band inside the well
        // (automatically switches to clean sparkline below 76pt total height in hthird).
        let t = now();
        let since = t - span_mins * 60;
        let combined = merged_activity(local_activity, hosts, &active_scope);
        let clients_list: Vec<String> = combined.clients();
        let lines: Vec<VecDeque<(f64, f64)>> = clients_list.iter().map(|cl| combined.series(cl, t, span_mins, step_mins)).collect();
        let totals: Vec<_> = clients_list.iter().map(|cl| combined.totals(cl, since)).collect();
        let shown: Vec<usize> = (0..clients_list.len()).filter(|&i| i < 2 || totals[i].requests > 0).collect();
        let series: Vec<Series> = shown.iter().map(|&i| Series::new(clients_list[i].clone(), style::c(p.hue(hue(&clients_list[i]))), Rc::new(lines[i].clone())).fill(false)).collect();
        let fmt = |v: f64| if v >= 10.0 || (v - v.round()).abs() < 1e-6 { format!("{v:.0}/min") } else { format!("{v:.1}/min") };
        let graph =
            Graph::new(format!("pool-activity-{span_name}"), series, t as f64).span((span_mins * 60) as f64).grid(grid_secs).unit(fmt).y_labels(true).x_labels(|sec| clock(sec as i64)).smooth(false);

        // Clean legend with uniform " · " middot spacing
        let mut legend_row =
            if two_col && vp_w < 780.0 { div().w_full().pt(px(2.0)).flex().flex_col().gap(px(2.0)) } else { div().w_full().pt(px(2.0)).flex().flex_row().flex_wrap().items_center().gap(px(16.0)) };
        for &i in &shown {
            let x = totals[i];
            let req_word = format!("{} request{}", x.requests, if x.requests == 1 { "" } else { "s" });
            let tok_tail = if !two_col && x.input_tokens + x.output_tokens > 0 { format!(" · {} in · {} out", tokens(x.input_tokens), tokens(x.output_tokens)) } else { String::new() };
            let mut item = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch_dot(style::c(p.hue(hue(&clients_list[i]))), 7.0))
                .child(StringView::new(clients_list[i].clone()).size(style::SMALL_SIZE).height(DETAIL_ROW));
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

        // Chart header ALWAYS stays paired above the chart with uniform m::CONTROL_HEIGHT so
        // two-column headings and wells align on one exact horizontal line!
        let chart_hdr = div()
            .w_full()
            .h(px(m::CONTROL_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(StringView::new("Requests per Minute").bold(true).height(m::CONTROL_HEIGHT))
            .child(span_pick);

        let mut chart_section = div().w_full().flex().flex_col().gap(px(6.0)).child(chart_hdr);
        if two_col {
            chart_section = chart_section.flex_1().min_h(px(56.0)).child(div().flex_1().min_h(px(40.0)).w_full().child(graph)).child(legend_row);
        } else {
            let gh = if compact_h { 112.0 } else { GRAPH_HEIGHT };
            chart_section = chart_section.child(div().w_full().h(px(gh)).child(graph)).child(legend_row);
        }

        // Recent Requests table: drop columns strictly by available width, cap Status compactly
        // on the right (88pt) so Model absorbs extra width cleanly without a 240pt void!
        let left_w = (vp_w * 0.41).clamp(244.0, 480.0);
        let table_w = if two_col { vp_w - left_w - 44.0 } else { vp_w - 28.0 };
        let recent = recent_requests(pool, hosts, &active_scope, RECENT_TABLE_LIMIT);
        let show_machine_col = active_scope == MachineScope::All && hosts.iter().any(|h| h.wanted || h.status.is_some()) && table_w >= 540.0;
        let show_account_col = table_w >= 390.0;
        let show_io_cols = table_w >= 500.0;
        let time_w = 38.0;
        let machine_w = if table_w < 580.0 { 76.0 } else { 94.0 };
        let account_w = if two_col && table_w > 540.0 { 92.0 } else { 76.0 };
        let in_w = 44.0;
        let out_w = 40.0;
        let status_w = 88.0;

        let hdr_div = || div().w(px(1.0)).h(px(12.0)).bg(style::c(p.control_border)).flex_shrink_0();
        let col_hdr = |label: &'static str, right: bool| {
            let s = StringView::new(label).size(style::SMALL_SIZE).bold(true).color(sec_panel).height(DETAIL_ROW);
            if right { div().w_full().flex().justify_end().child(s) } else { div().child(s) }
        };
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
            .child(div().w(px(time_w)).flex_shrink_0().child(col_hdr("Time", false)));
        if show_machine_col {
            header_row = header_row.child(hdr_div()).child(div().w(px(machine_w)).flex_shrink_0().child(col_hdr("Machine", false)));
        }
        header_row = header_row.child(hdr_div()).child(div().flex_1().min_w(px(108.0)).child(col_hdr("Model", false)));
        if show_account_col {
            header_row = header_row.child(hdr_div()).child(div().w(px(account_w)).flex_shrink_0().child(col_hdr("Account", false)));
        }
        if show_io_cols {
            header_row = header_row
                .child(hdr_div())
                .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().child(col_hdr("In", true)))
                .child(hdr_div())
                .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().child(col_hdr("Out", true)));
        }
        header_row = header_row.child(hdr_div()).child(div().w(px(status_w)).flex_shrink_0().child(col_hdr("Status", true)));

        let sel_bg = style::c(style::selection_ground(p, p.list_background, true));
        let sel_fg = style::c(style::selection_text(p, p.panel_text, true));
        let mut rows = div().w_full().pb(px(4.0)).flex().flex_col();

        // Invisible 1px spacer matching header dividers so every column x-coordinate matches exactly
        let col_gap = || div().w(px(1.0)).flex_shrink_0();

        for o in &live {
            let selected = matches!(&self.activity_picked, Some(Pick::Live(k)) if k == &o.key);
            let fg = if selected { sel_fg } else { style::c(p.panel_text) };
            let dim_fg = if selected { sel_fg } else { sec_list };
            let start_clock = if o.started_ms > 0 { clock(o.started_ms / 1000) } else { "now".into() };
            let key = o.key.clone();
            let mut line = div()
                .w_full()
                .pl(px(m::TIGHT))
                .pr(px(m::TIGHT + 2.0))
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
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(o.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(o.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            line = line.child(col_gap()).child(
                div()
                    .flex_1()
                    .min_w(px(108.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(swatch_dot(style::c(p.hue(provider_hue(o.provider))), 6.0))
                    .child(div().flex_1().min_w_0().child(StringView::new(o.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
            );
            if show_account_col {
                line = line.child(col_gap()).child(div().w(px(account_w)).flex_shrink_0().child(StringView::new(o.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
            }
            if show_io_cols {
                line = line
                    .child(col_gap())
                    .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)))
                    .child(col_gap())
                    .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            }
            // In-progress uses neutral primary ink ("Running · 2.3s"), keeping green for health dots only.
            let elapsed = if o.started_ms > 0 { format!("Running · {}", duration((t_ms - o.started_ms).max(0) as u64)) } else { "Running".to_owned() };
            line = line.child(col_gap()).child(div().w(px(status_w)).flex_shrink_0().flex().justify_end().child(StringView::new(elapsed).size(style::SMALL_SIZE).color(fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        for r in &recent {
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
                .pr(px(m::TIGHT + 2.0))
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
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(r.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(r.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            line = line.child(col_gap()).child(
                div()
                    .flex_1()
                    .min_w(px(108.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(swatch_dot(style::c(p.hue(provider_hue(r.provider))), 6.0))
                    .child(div().flex_1().min_w_0().child(StringView::new(r.request.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
            );
            if show_account_col {
                line = line.child(col_gap()).child(div().w(px(account_w)).flex_shrink_0().child(StringView::new(r.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
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
            line =
                line.child(col_gap()).child(div().w(px(status_w)).flex_shrink_0().flex().justify_end().child(StringView::new(status_text).size(style::SMALL_SIZE).color(status_fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        if live.is_empty() && recent.is_empty() {
            let empty_msg = format!("No requests in the {}.", span_name.to_ascii_lowercase());
            rows = rows.child(div().w_full().py(px(32.0)).flex().justify_center().child(StringView::new(empty_msg).color(sec_list)));
        }

        // Genuine BeOS sunken frame around Recent Requests matching Graph and ScrollView wells!
        let well_frame = canvas(|g, b| {
            let p = theme::palette();
            style::sunken(g, b, &p, false);
        })
        .paint_key(style::key("recent-well"))
        .absolute()
        .inset_0();
        let recent_well = div()
            .relative()
            .flex_1()
            .min_h_0()
            .p(px(2.0))
            .bg(style::c(p.list_background))
            .flex()
            .flex_col()
            .child(header_row)
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.activity_scroll).border(false).background(p.list_background).child(rows)))
            .child(well_frame);

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
            let left_col =
                div().relative().w(px(left_w)).flex_shrink_0().h_full().flex().flex_col().child(canvas(|g, b| olo_kit::review::own_grid(g, b)).paint_key(0).absolute().inset_0()).child(chart_section);
            let mut right_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
            if let Some(bar) = picked_bar {
                right_col = right_col.child(bar);
            }
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(top_bar)
                .child(div().flex_1().min_h_0().flex().flex_row().gap(px(20.0)).child(left_col).child(right_col))
                .into_element();
        }

        let mut recent_section = div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
        if let Some(bar) = picked_bar {
            recent_section = recent_section.child(bar);
        }

        div().flex_1().min_h_0().flex().flex_col().gap(px(if compact_h { 8.0 } else { 12.0 })).child(top_bar).child(chart_section).child(recent_section).into_element()
    }

    // ---- Tab 3: Machines --------------------------------------------------

    fn machines_tab(&self, pool: &Pool, hosts: &[Stands], running: Option<&Running>, uses: Option<[bool; 2]>, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let vp = cx.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);
        let narrow = vp_w < 480.0;
        let compact_h = vp_h < 420.0;
        let tight_h = vp_h < 290.0;
        let sec_panel = secondary_ink(p, p.panel_background);

        // Sort remote hosts by severity FIRST: errors -> in progress -> healthy -> not installed!
        let ordered_hosts = sorted_hosts(hosts);
        let local_section = self.this_computer_section(pool, &ordered_hosts, running, uses, narrow, compact_h, tight_h, p, cx);
        let sel_host = self.selected_host.as_deref().filter(|id| ordered_hosts.iter().any(|h| h.id == *id)).or_else(|| ordered_hosts.first().map(|h| h.id.as_str()));
        let sel_stands = sel_host.and_then(|id| ordered_hosts.iter().find(|h| h.id == id));

        // Remote Rex Hosts heading + red error count matching Activity's red failures
        let err_n = ordered_hosts.iter().filter(|h| h.error.is_some()).count();
        let mut count_row = div().flex().flex_row().items_center().gap(px(4.0));
        if !ordered_hosts.is_empty() {
            count_row = count_row.child(StringView::new(format!("{} configured", ordered_hosts.len())).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(m::LABEL_ROW));
            if err_n > 0 {
                let err_txt = if err_n == 1 { "1 error".to_owned() } else { format!("{err_n} errors") };
                count_row = count_row
                    .child(StringView::new("·").size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(m::LABEL_ROW))
                    .child(StringView::new(err_txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(style::c(p.failure)).height(m::LABEL_ROW));
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
            let top_pad = if compact_h { 20.0 } else { 56.0 };
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
                if idx > 0 {
                    host_list = host_list.child(separator());
                }
                let kept = kept_names(pool, Some(&h.id));
                let is_sel = sel_host == Some(h.id.as_str());
                host_list = host_list.child(self.remote_host_row(h, &kept, is_sel, wide, narrow, compact_h, p, cx));
            }
        }
        let hosts_well = div().flex_1().min_h_0().child(ScrollView::new(&self.hosts_scroll).background(p.list_background).child(host_list));

        // Consistent footer matching Accounts order & labels: [Add Host…] [Remove Host…] … Checked HH:MM [Sync All]
        let any_syncable = ordered_hosts.iter().any(|h| h.wanted && h.doing.is_none());
        let asking_this = sel_host.is_some() && self.host_asking_remove.as_deref() == sel_host;
        let can_remove = !asking_this && sel_stands.is_some_and(|h| h.wanted && h.doing.is_none());
        let rm_id = sel_stands.map(|h| h.id.clone()).unwrap_or_default();
        let checked_text = last_checked_at(&ordered_hosts).map(|at| format!("Checked {}", clock(at))).unwrap_or_default();

        let mut status_box = div().flex_1().min_w_0().flex().justify_end();
        if !checked_text.is_empty() && !narrow && !compact_h {
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
            let left_w = (vp_w * 0.36).clamp(198.0, 290.0);
            let hosts_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_well).child(footer);
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .gap(px(20.0))
                .child(
                    div()
                        .relative()
                        .w(px(left_w))
                        .flex_shrink_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(canvas(|g, b| olo_kit::review::own_grid(g, b)).paint_key(0).absolute().inset_0())
                        .child(local_section),
                )
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
        uses: Option<[bool; 2]>,
        narrow: bool,
        compact_h: bool,
        tight_h: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let sec = secondary_ink(p, p.panel_background);
        // Unified left column matching Remote Rex Hosts below: 13pt gutter + 5pt gap = 18pt exact!
        const INDENT: f32 = 18.0;

        let (gw_lamp, gw_status, gw_err, gw_detail) = match &pool.error {
            Some(e) => {
                let reason = if e.to_ascii_lowercase().contains("address already in use") { format!("Port {} is in use by another app.", self.proxy_port) } else { e.clone() };
                (Light::Down, "Stopped".to_owned(), true, Some(reason))
            }
            None if compact_h => (Light::On, format!("127.0.0.1:{}", self.proxy_port), false, None),
            None => (Light::On, format!("Listening on 127.0.0.1:{}", self.proxy_port), false, None),
        };

        // Bold section heading matching "Remote Rex Hosts" (same x-offset, no BBox frame!)
        let hdr = div().w_full().flex().flex_row().items_center().justify_between().gap(px(8.0)).child(StringView::new("This Computer").bold(true).height(m::LABEL_ROW));

        // 1. Gateway row + named account custody (broken cleanly at " · " when narrow/compact)
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
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(left.to_owned()).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)))
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(cap_right).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)));
            } else {
                gw_block = gw_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(custody_line).size(style::SMALL_SIZE).color(sec).wrap(true)));
            }
        }

        // 2. Claude Code client row + matching right-hand count (no redundant "through the pool"!)
        let claude_on = uses.is_some_and(|c| c[client_index(Provider::Claude)]);
        let claude_procs = running.map(|r| r.claude.as_slice()).unwrap_or(&[]);
        let claude_count = match claude_procs.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };
        let mut claude_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(CheckBox::new("Claude Code uses the pool", claude_on).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                    if !demo_enabled() {
                        olo_kit::proxy().send(Msg::SetClient(Provider::Claude, *on));
                    }
                }))
                .child(StringView::new(claude_count).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec).height(m::MARK_ROW)),
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
        let codex_check = CheckBox::new("Codex uses the pool", codex_on)
            .height(if needs_restart && !narrow && !compact_h { m::CONTROL_HEIGHT } else { m::MARK_ROW })
            .enabled(uses.is_some())
            .on_toggle(|on: &bool, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SetClient(Provider::Codex, *on));
                }
            });

        let mut codex_block = div().w_full().flex().flex_col().gap(px(2.0));
        match daemon {
            None => {
                codex_block = codex_block.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(codex_check)
                        .child(StringView::new("Stopped").size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec).height(m::MARK_ROW)),
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
                            .gap(px(8.0))
                            .child(codex_check)
                            .child(StringView::new(conn_count).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(if needs_restart { style::c(p.failure) } else { sec }).height(m::MARK_ROW)),
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
                            let place = th
                                .cwd
                                .as_deref()
                                .map(|dir| {
                                    let f = folder(dir);
                                    if narrow { format!(" · {}", middle_truncate(&f, 18)) } else { format!(" · {f}") }
                                })
                                .unwrap_or_default();
                            codex_block =
                                codex_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(format!("{title}{place}")).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)));
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

        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(if tight_h { 6.0 } else { 8.0 }))
            .child(div().w_full().pb(px(if tight_h { 2.0 } else { 6.0 })).child(gw_block))
            .child(claude_block)
            .child(codex_block);
        if let Some((note, err)) = &self.told.note {
            content = content.child(StringView::new(note.clone()).size(style::SMALL_SIZE).color(if *err { style::c(p.failure) } else { sec }).wrap(true));
        }

        div().w_full().flex().flex_col().gap(px(6.0)).child(hdr).child(content).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_host_row(&self, s: &Stands, kept: &[String], selected: bool, wide: f32, narrow: bool, compact_h: bool, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
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

        // Uniform title line height across all rows (buttons align centred on the title line without shifting sublines!)
        let narrow_row = narrow || compact_h;
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
                "Claude\u{a0}Code and Codex on this host will return to their own sign-ins.".into(),
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
                            let summary = host_summary(status, kept, s.heard_at, narrow || compact_h);
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
            .py(px(8.0))
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
            content = content.child(gateway_stopped_alert(p, self.proxy_port, err, cx));
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
