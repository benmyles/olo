//! Network: Wi-Fi networks in range (join, leave, forget) and each
//! interface's addresses. Talks to NetworkManager natively
//! (`olo_system::network`) on a worker thread that reads its state every
//! two seconds, scans now and then, and makes the changes, so the window
//! never waits on it.

use olo_sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_system::network::{self as nm, Device, Enterprise, Kind, Network, Status};
use olo_ui::{
    WindowOptions,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{AnyElement, App, AppContext as _, Bounds, Context, Entity, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, Render, SharedString, Styled as _, Window, div, point, px, size},
    metrics as m, style,
    widgets::{Button, CheckBox, Column, ColumnListView, EmptyState, ListModel, MenuField, StringView, TabView, TextControl, TextField, TextFieldEvent, separator},
};

use crate::{Msg, PanelView, form::Ui, panel};

enum Request {
    Wireless(bool),
    Join(String, Network, Option<String>),
    JoinEnterprise(String, Network, Box<nm::Enterprise>),
    Disconnect(String),
    Forget(String),
}

#[derive(Default)]
struct Shared {
    /// None until first read; Err when NetworkManager isn't there.
    status: Option<Result<Status, String>>,
    busy: bool,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

/// How often to ask the Wi-Fi adapter for a fresh look around.
const SCAN_EVERY: Duration = Duration::from_secs(30);

/// NetworkManager's worker (one for the app), telling the windows
/// through `proxy` when anything changes.
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        if fake_networks() {
            shared.lock().unwrap().status = Some(Ok(fake_status()));
        }
        let (tx, rx) = channel::<Request>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("network".into())
            .spawn(move || {
                let wake = || proxy.send(Msg::Redraw);
                if fake_networks() {
                    return fake_worker(&sh, &wake, &rx);
                }
                let mut scanned: Option<Instant> = None;
                loop {
                    let status = nm::status().map_err(|e| e.to_string());
                    if let Ok(s) = &status
                        && s.wireless_enabled
                        && scanned.is_none_or(|t| t.elapsed() > SCAN_EVERY)
                    {
                        for d in s.devices.iter().filter(|d| d.kind == Kind::Wifi) {
                            let _ = nm::scan(&d.path);
                        }
                        scanned = Some(Instant::now());
                    }
                    let status = Some(status);
                    let changed = {
                        let mut s = sh.lock().unwrap();
                        std::mem::replace(&mut s.status, status.clone()) != status
                    };
                    if changed {
                        wake();
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    sh.lock().unwrap().busy = true;
                    wake();
                    let r = match req {
                        Request::Wireless(on) => nm::set_wireless_enabled(on).map_err(|e| (e, "Wi-Fi couldn't be turned on or off.")),
                        Request::Join(dev, net, mut pw) => {
                            let r = nm::join(&dev, &net, pw.as_deref()).map_err(|e| (e, "The network couldn't be joined."));
                            if let Some(pw) = pw.as_mut() {
                                // SAFETY: zero bytes keep the string valid UTF-8.
                                unsafe { pw.as_bytes_mut().fill(0) };
                            }
                            r
                        }
                        Request::JoinEnterprise(dev, net, mut e) => {
                            let r = nm::join_enterprise(&dev, &net, &e).map_err(|e| (e, "The network couldn't be joined."));
                            for secret in [&mut e.password, &mut e.key_password] {
                                // SAFETY: zero bytes keep the string valid UTF-8.
                                unsafe { secret.as_bytes_mut().fill(0) };
                            }
                            r
                        }
                        Request::Disconnect(dev) => nm::disconnect(&dev).map_err(|e| (e, "The network couldn't be left.")),
                        Request::Forget(ssid) => nm::forget(&ssid).map_err(|e| (e, "The network couldn't be forgotten.")),
                    };
                    let mut s = sh.lock().unwrap();
                    s.busy = false;
                    s.error = r.err().map(|(e, what)| {
                        tracing::warn!("NetworkManager: {e}");
                        what.to_owned()
                    });
                    drop(s);
                    wake();
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// `OLO_FAKE_NETWORKS=1` (debug builds): made-up Wi-Fi networks, so
/// the panel and the enterprise sheet can be tried headless without
/// touching NetworkManager or showing the networks around the machine.
/// Tests always have them.
fn fake_networks() -> bool {
    cfg!(test) || (cfg!(debug_assertions) && std::env::var_os("OLO_FAKE_NETWORKS").is_some())
}

fn fake_status() -> Status {
    let net = |ssid: &str, strength, secure, enterprise| Network { ssid: ssid.into(), strength, secure, enterprise, key_mgmt: "wpa-psk", ap: format!("/ap/{ssid}"), active: false, known: false };
    Status {
        wireless_enabled: true,
        wireless_hardware: true,
        devices: vec![Device {
            path: "/fake/wlan0".into(),
            interface: "wlan0".into(),
            kind: Kind::Wifi,
            state: 30,
            reason: 0,
            hw_address: "02:00:00:00:00:01".into(),
            connection: None,
            addresses: Vec::new(),
            gateway: None,
            dns: Vec::new(),
            speed: None,
        }],
        networks: vec![net("Olo Campus", 90, true, true), net("Olo Home", 70, true, false), net("Olo Guest", 40, false, false)],
    }
}

fn fake_worker(sh: &Arc<Mutex<Shared>>, wake: &dyn Fn(), rx: &std::sync::mpsc::Receiver<Request>) {
    let mut status = fake_status();
    loop {
        sh.lock().unwrap().status = Some(Ok(status.clone()));
        wake();
        let Ok(req) = rx.recv() else { return };
        let joined = match req {
            Request::Join(_, n, _) => Some(n.ssid),
            Request::JoinEnterprise(_, n, e) => {
                tracing::info!(
                    "fake join of {}: {:?}",
                    n.ssid,
                    nm::enterprise_settings(&Enterprise { password: "…".into(), key_password: String::new(), ..*e }).get("802-1x").map(|x| x.keys().collect::<Vec<_>>())
                );
                Some(n.ssid)
            }
            Request::Disconnect(_) => {
                status.networks.iter_mut().for_each(|n| n.active = false);
                None
            }
            Request::Forget(ssid) => {
                status.networks.iter_mut().filter(|n| n.ssid == ssid).for_each(|n| n.known = false);
                None
            }
            Request::Wireless(on) => {
                status.wireless_enabled = on;
                None
            }
        };
        if let Some(ssid) = joined {
            for n in &mut status.networks {
                n.active = n.ssid == ssid;
                n.known |= n.active;
            }
            let d = &mut status.devices[0];
            d.state = 100;
            d.connection = Some(ssid);
        }
    }
}

pub struct State {
    tab: usize,
    list: Entity<ListModel>,
    /// The selected network's name (the list reorders as signals change).
    selected: Option<String>,
    password: Entity<TextField>,
    device: usize,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let password = olo_ui::widgets::text_field(cx, |f| f.password(true));
        // Return joins, once the password can be one.
        cx.subscribe(&password, |v, _, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => {
                if let Some(st) = state_of(v) {
                    st.join(cx);
                }
                cx.notify();
            }
            TextFieldEvent::Changed => cx.notify(),
            _ => {}
        })
        .detach();
        State { tab: 0, list: ListModel::new(cx, false), selected: None, password, device: 0, link: link(olo_ui::proxy(cx)) }
    }

    fn clear_password(&mut self, cx: &mut App) {
        self.password.update(cx, |f, cx| f.clear_secret(cx));
    }

    /// Join (or open the sheet for) the network chosen, if it can be.
    fn join(&mut self, cx: &mut Context<PanelView>) {
        let (status, busy) = {
            let sh = self.link.shared.lock().unwrap();
            (sh.status.clone(), sh.busy)
        };
        let Some(Ok(status)) = status else { return };
        let Some(dev) = status.devices.iter().find(|d| d.kind == Kind::Wifi) else { return };
        let Some(net) = self.selected.as_ref().and_then(|s| status.networks.iter().find(|n| n.ssid == *s)).cloned() else { return };
        if busy || net.active {
            return;
        }
        let needs_password = net.secure && !net.known && !net.enterprise;
        let needs_sheet = net.enterprise && !net.known;
        if needs_sheet {
            open_sheet(self.link, dev.path.clone(), net, cx);
            return;
        }
        let pw = needs_password.then(|| self.password.read(cx).text().to_owned());
        // WPA passwords are 8 to 63 characters.
        if pw.as_ref().is_some_and(|p| !(8..=63).contains(&p.len())) {
            return;
        }
        let _ = self.link.tx.send(Request::Join(dev.path.clone(), net, pw));
        self.clear_password(cx);
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Network(st) => Some(st),
        _ => None,
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let (status, busy, error) = {
        let sh = st.link.shared.lock().unwrap();
        (sh.status.clone(), sh.busy, sh.error.clone())
    };
    let status = match status {
        None => return ui.add(EmptyState::new("Looking for network devices…").icon("network-prefs")),
        Some(Err(_)) => return ui.add(EmptyState::new("NetworkManager isn't running.").icon("network-prefs")),
        Some(Ok(s)) => s,
    };
    let items = ui.collect(|ui| {
        match st.tab {
            0 => wifi(ui, st, &status, busy),
            _ => details(ui, st, &status),
        }
        if busy {
            ui.note("Working…");
        } else if let Some(e) = &error {
            ui.label(e.clone());
        }
    });
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.tab = i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Wi-Fi", "Details"].into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("network-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

// ---- Wi-Fi ---------------------------------------------------------------------

fn wifi(ui: &mut Ui, st: &mut State, status: &Status, busy: bool) {
    let link = st.link;
    let Some(dev) = status.devices.iter().find(|d| d.kind == Kind::Wifi).cloned() else {
        ui.note("There's no Wi-Fi adapter.");
        return;
    };
    let text = if !status.wireless_hardware {
        "Turned off by the hardware switch.".to_owned()
    } else if !status.wireless_enabled {
        "Off".to_owned()
    } else if let (true, Some(name)) = (dev.connected(), &dev.connection) {
        format!("Connected to “{name}”")
    } else if dev.state == 120 || (dev.state == 30 && dev.failure().is_some()) {
        dev.failure().unwrap_or("Couldn't connect").to_owned()
    } else {
        dev.state_text().to_owned()
    };
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING * 3.0))
            .child(CheckBox::new("Wi-Fi", status.wireless_enabled).enabled(status.wireless_hardware && !busy).on_toggle(move |on, _, _| {
                let _ = link.tx.send(Request::Wireless(on));
            }))
            .child(StringView::new(text).dim(true)),
    );
    if !status.wireless_enabled {
        return;
    }

    let nets = status.networks.clone();
    // Keep the selection on the same network as the list reorders.
    let at = st.selected.as_ref().and_then(|s| nets.iter().position(|n| n.ssid == *s));
    st.list.update(ui.cx, |l, _| {
        l.state.selected.clear();
        if let Some(i) = at {
            l.state.select(i);
        }
    });
    let columns = vec![Column::new("", 22.0), Column::new("Network", 210.0), Column::new("Security", 100.0), Column::new("Signal", 50.0)];
    let cells = nets.clone();
    let model = st.list.clone();
    let ssids: Vec<String> = nets.iter().map(|n| n.ssid.clone()).collect();
    let pick = ui.on_view(move |v, _: (), _, cx| {
        let row = model.read(cx).state.single();
        if let Some(st) = state_of(v) {
            st.selected = row.and_then(|i| ssids.get(i).cloned());
            st.clear_password(cx);
        }
        cx.notify();
    });
    let open = ui.on_view(|v, _: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.join(cx);
        }
        cx.notify();
    });
    let list = ColumnListView::new("networks", &st.list, columns, nets.len(), move |cell, window, cx| {
        let n = &cells[cell.row];
        match cell.column {
            0 if n.active => check(window, cell.bounds, cell.text_color),
            1 => cell.text(window, cx, n.ssid.clone()),
            2 => cell.text(
                window,
                cx,
                match (n.enterprise, n.secure, n.known) {
                    (true, ..) => "Enterprise",
                    (_, true, true) => "Secured, saved",
                    (_, true, false) => "Secured",
                    (_, false, true) => "Open, saved",
                    (_, false, false) => "Open",
                },
            ),
            3 => bars(window, cell.bounds, n.strength, cell.text_color),
            _ => {}
        }
    })
    .on_select(move |window, cx| pick((), window, cx))
    .on_open(open);
    ui.add(div().flex_1().min_h(px(90.0)).child(list));
    let Some(net) = at.and_then(|i| nets.get(i)).cloned() else {
        ui.note("Choose a network to join it.");
        return;
    };
    let needs_password = net.secure && !net.known && !net.active && !net.enterprise;
    // A new enterprise network asks for more than fits here: a sheet.
    let needs_sheet = net.enterprise && !net.known && !net.active;
    if needs_password {
        ui.add(TextControl::new(st.password.clone()).label("Password:").divider(72.0));
    }
    let mut row: Vec<AnyElement> = Vec::new();
    if net.active {
        let path = dev.path.clone();
        row.push(
            Button::new("Disconnect")
                .enabled(!busy)
                .on_click(move |_, _| {
                    let _ = link.tx.send(Request::Disconnect(path.clone()));
                })
                .into_any_element(),
        );
    } else {
        // WPA passwords are 8 to 63 characters.
        let ready = !needs_password || (8..=63).contains(&st.password.read(ui.cx).text().len());
        let label = if needs_sheet { "Join…" } else { "Join" };
        let join = ui.on_click_view(|v, _, cx| {
            if let Some(st) = state_of(v) {
                st.join(cx);
            }
            cx.notify();
        });
        row.push(Button::new(label).default(true).enabled(ready && !busy).on_click(join).into_any_element());
    }
    if net.known {
        let ssid = net.ssid.clone();
        row.push(
            Button::new("Forget")
                .enabled(!busy)
                .on_click(move |_, _| {
                    let _ = link.tx.send(Request::Forget(ssid.clone()));
                })
                .into_any_element(),
        );
    }
    ui.add(div().flex().flex_row().gap(px(m::SPACING)).children(row));
}

// ---- Enterprise sheet ------------------------------------------------------------

/// Joining a WPA-Enterprise (802.1X) network: method, credentials and how
/// to trust the server, as macOS asks. A window of its own over the
/// panel's.
pub struct EnterpriseForm {
    link: &'static Link,
    device: String,
    net: Network,
    eap: usize,
    inner: usize,
    /// System certificates, a file, or don't verify.
    ca: usize,
    ca_file: Option<PathBuf>,
    client_cert: Option<PathBuf>,
    private_key: Option<PathBuf>,
    identity: Entity<TextField>,
    password: Entity<TextField>,
    key_password: Entity<TextField>,
    anonymous: Entity<TextField>,
    domain: Entity<TextField>,
    /// Joined (Return in a field): the window closes.
    done: bool,
}

pub const SHEET_WIDTH: f32 = 440.0;

/// Certificates and keys, as PEM, DER or PKCS#12.
pub const CERT_TYPES: &[&str] = &["pem", "crt", "cer", "der", "key", "p12", "pfx"];
const SHEET_DIVIDER: f32 = 132.0;

/// The sheet for joining `net` (one per network: an open one comes forward).
fn open_sheet(link: &'static Link, device: String, net: Network, cx: &mut App) {
    let title = format!("Join “{}”", net.ssid);
    // (As tall as its rows: they come and go with the method chosen.)
    let options = WindowOptions { fixed: true, ..WindowOptions::new(title, SHEET_WIDTH, 200.0).dialog() };
    let _ = olo_ui::open_window(cx, options, move |window, cx| {
        window.activate_window();
        cx.new(|cx| {
            let field = |cx: &mut Context<EnterpriseForm>, hint: &str, password: bool| {
                let hint = hint.to_owned();
                let f = olo_ui::widgets::text_field(cx, |f| f.hint(hint).password(password));
                cx.subscribe(&f, |form: &mut EnterpriseForm, _, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => form.join(cx),
                    TextFieldEvent::Changed => cx.notify(),
                    _ => {}
                })
                .detach();
                f
            };
            let identity = field(cx, "", false);
            identity.update(cx, |f, _| f.focus(window));
            EnterpriseForm {
                link,
                device,
                net,
                eap: 0,
                inner: 0,
                ca: 0,
                ca_file: None,
                client_cert: None,
                private_key: None,
                password: field(cx, "", true),
                key_password: field(cx, "", true),
                anonymous: field(cx, "Optional", false),
                domain: field(cx, "Optional, e.g. example.edu", false),
                identity,
                done: false,
            }
        })
    });
}

impl EnterpriseForm {
    fn enterprise(&self, cx: &App) -> Enterprise {
        let eap = nm::Eap::ALL[self.eap.min(2)];
        let inner = eap.inner().get(self.inner).or(eap.inner().first()).map(|s| s.to_string()).unwrap_or_default();
        Enterprise {
            eap,
            inner,
            identity: self.identity.read(cx).text().to_owned(),
            password: self.password.read(cx).text().to_owned(),
            anonymous_identity: self.anonymous.read(cx).text().to_owned(),
            ca: match self.ca {
                0 => nm::CaCert::System,
                1 => nm::CaCert::File(self.ca_file.clone().unwrap_or_default()),
                _ => nm::CaCert::None,
            },
            domain: if self.ca == 2 { String::new() } else { self.domain.read(cx).text().to_owned() },
            client_cert: self.client_cert.clone(),
            private_key: self.private_key.clone(),
            key_password: self.key_password.read(cx).text().to_owned(),
        }
    }

    fn join(&mut self, cx: &mut Context<Self>) {
        let e = self.enterprise(cx);
        if !e.ready() {
            return;
        }
        let _ = self.link.tx.send(Request::JoinEnterprise(self.device.clone(), self.net.clone(), Box::new(e)));
        self.clear_secrets(cx);
        // (Its window closes as it's next drawn.)
        self.done = true;
        cx.notify();
    }

    fn clear_secrets(&mut self, cx: &mut App) {
        for f in [&self.password, &self.key_password] {
            f.update(cx, |f, cx| f.clear_secret(cx));
        }
    }

    /// The file panel for one of the sheet's files.
    fn choose(&self, which: u8, cx: &mut Context<Self>) {
        let title = match which {
            0 => "Choose a CA Certificate",
            1 => "Choose Your Certificate",
            _ => "Choose Your Private Key",
        };
        let dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let me = cx.weak_entity();
        file_panel::show(cx, title, FilePanel::open(dir).types(CERT_TYPES), move |a, cx| {
            let PanelResult::Chosen(path) = a.result else { return };
            let _ = me.update(cx, |f, cx| {
                match which {
                    0 => f.ca_file = Some(path),
                    1 => f.client_cert = Some(path),
                    _ => f.private_key = Some(path),
                }
                cx.notify();
            });
        });
    }
}

impl Render for EnterpriseForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.done {
            window.remove_window();
            return div();
        }
        let eap = nm::Eap::ALL[self.eap.min(2)];
        let me = cx.weak_entity();
        let set = move |f: fn(&mut EnterpriseForm, usize)| {
            let me = me.clone();
            move |i: usize, _: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |form, cx| {
                    f(form, i);
                    cx.notify();
                });
            }
        };
        let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET));
        col = col.child(StringView::new(format!("“{}” is an enterprise network.", self.net.ssid)));
        let eaps: Vec<SharedString> = nm::Eap::ALL.iter().map(|e| SharedString::from(e.name())).collect();
        col = col.child(MenuField::new("ent-eap", eaps, self.eap).label("Security:").divider(SHEET_DIVIDER).on_change(set(|f, i| f.eap = i)));
        let file_row = |label: &str, path: Option<&std::path::Path>, which: u8, cx: &mut Context<Self>| {
            let name = path.and_then(|p| p.file_name()).map_or("None".into(), |n| n.to_string_lossy().into_owned());
            let me = cx.weak_entity();
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(div().w(px(SHEET_DIVIDER)).flex_shrink_0().child(StringView::new(label.to_owned())))
                .child(div().flex_1().child(StringView::new(name).dim(path.is_none())))
                .child(Button::new("Choose…").on_click(move |_, cx| {
                    let _ = me.update(cx, |f, cx| f.choose(which, cx));
                }))
        };
        if eap == nm::Eap::Tls {
            col = col.child(TextControl::new(self.identity.clone()).label("Identity:").divider(SHEET_DIVIDER));
            col = col.child(file_row("Certificate:", self.client_cert.as_deref(), 1, cx));
            col = col.child(file_row("Private key:", self.private_key.as_deref(), 2, cx));
            col = col.child(TextControl::new(self.key_password.clone()).label("Key password:").divider(SHEET_DIVIDER));
        } else {
            let names: Vec<SharedString> = eap.inner().iter().map(|i| SharedString::from(inner_name(i))).collect();
            self.inner = self.inner.min(names.len().saturating_sub(1));
            col = col.child(MenuField::new("ent-inner", names, self.inner).label("Authentication:").divider(SHEET_DIVIDER).on_change(set(|f, i| f.inner = i)));
            col = col.child(TextControl::new(self.identity.clone()).label("Username:").divider(SHEET_DIVIDER));
            col = col.child(TextControl::new(self.password.clone()).label("Password:").divider(SHEET_DIVIDER));
        }
        col = col.child(TextControl::new(self.anonymous.clone()).label("Anonymous identity:").divider(SHEET_DIVIDER));
        let cas: Vec<SharedString> = ["System certificates", "A certificate file", "Don't verify"].into_iter().map(SharedString::from).collect();
        col = col.child(MenuField::new("ent-ca", cas, self.ca).label("CA certificate:").divider(SHEET_DIVIDER).on_change(set(|f, i| f.ca = i)));
        if self.ca == 1 {
            col = col.child(file_row("", self.ca_file.as_deref(), 0, cx));
        }
        self.domain.update(cx, |f, cx| f.set_enabled(self.ca != 2, cx));
        col = col.child(TextControl::new(self.domain.clone()).label("Domain:").divider(SHEET_DIVIDER));
        // Buttons at the bottom right, Join the default.
        let ready = self.enterprise(cx).ready();
        let (j, c) = (cx.weak_entity(), cx.weak_entity());
        col = col.child(div().flex_1()).child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").on_click(move |window, cx| {
                    let _ = c.update(cx, |f, cx| f.clear_secrets(cx));
                    window.remove_window();
                }))
                .child(Button::new("Join").default(true).enabled(ready).on_click(move |_, cx| {
                    let _ = j.update(cx, |f, cx| f.join(cx));
                })),
        );
        col
    }
}

fn inner_name(nm_name: &str) -> &'static str {
    match nm_name {
        "mschapv2" => "MSCHAPv2",
        "mschap" => "MSCHAP",
        "gtc" => "GTC",
        "md5" => "MD5",
        "pap" => "PAP",
        "chap" => "CHAP",
        _ => "Other",
    }
}

/// The tick beside the network in use.
fn check(window: &mut Window, r: Bounds<Pixels>, color: Hsla) {
    let c0 = r.center();
    let at = |x: f32, y: f32| point(c0.x + px(x), c0.y + px(y));
    let mut path = PathBuilder::stroke(px(2.0));
    path.move_to(at(-4.5, 0.0));
    path.line_to(at(-1.5, 3.5));
    path.line_to(at(4.5, -4.0));
    if let Ok(p) = path.build() {
        window.paint_path(p, color);
    }
}

/// Four rising bars, lit to the signal's strength.
fn bars(window: &mut Window, r: Bounds<Pixels>, strength: u8, color: Hsla) {
    let lit = match strength {
        0..=20 => 1,
        21..=45 => 2,
        46..=70 => 3,
        _ => 4,
    };
    // In whole device pixels, so they rise in even steps at every scale.
    let s = window.scale_factor();
    let d = |v: f32| (v * s).round();
    let (width, pitch, base, step) = (d(3.0), d(5.0), d(3.0), d(2.5));
    let (x0, bottom) = (d(f32::from(r.left()) + m::CELL_INSET), d(f32::from(r.center().y) + 5.0));
    for i in 0..4 {
        let (x, h) = (x0 + i as f32 * pitch, base + i as f32 * step);
        let bar = Bounds { origin: point(px(x / s), px((bottom - h) / s)), size: size(px(width / s), px(h / s)) };
        let fill = if i < lit { color } else { color.opacity(0.25) };
        style::fill_rect(window, bar, fill);
    }
}

// ---- Details -------------------------------------------------------------------

fn details(ui: &mut Ui, st: &mut State, status: &Status) {
    if status.devices.is_empty() {
        ui.note("There are no network interfaces.");
        return;
    }
    let names: Vec<SharedString> = status.devices.iter().map(|d| format!("{} ({})", d.interface, if d.kind == Kind::Wifi { "Wi-Fi" } else { "Ethernet" }).into()).collect();
    st.device = st.device.min(names.len() - 1);
    const DIVIDER: f32 = 130.0;
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.device = i;
        }
        cx.notify();
    });
    ui.add(MenuField::new("network-device", names, st.device).label("Interface:").divider(DIVIDER).on_change(pick));
    ui.add(separator());
    let d: &Device = &status.devices[st.device];
    let none = "—".to_owned();
    let rows = [
        ("Status:", d.state_text().to_owned()),
        ("Connection:", d.connection.clone().unwrap_or_else(|| none.clone())),
        ("IP address:", if d.addresses.is_empty() { none.clone() } else { d.addresses.join(", ") }),
        ("Router:", d.gateway.clone().unwrap_or_else(|| none.clone())),
        ("DNS servers:", if d.dns.is_empty() { none.clone() } else { d.dns.join(", ") }),
        ("Hardware address:", if d.hw_address.is_empty() { none.clone() } else { d.hw_address.clone() }),
        ("Speed:", d.speed.map_or(none.clone(), |s| if s >= 1000 && s % 1000 == 0 { format!("{} Gb/s", s / 1000) } else { format!("{s} Mb/s") })),
    ];
    let lines = rows.into_iter().map(|(label, value)| {
        // The label right-aligned in its column, as form rows have it.
        div().flex().flex_row().child(div().w(px(DIVIDER)).flex_shrink_0().flex().justify_end().pr(px(m::LABEL_GAP)).child(StringView::new(label))).child(StringView::new(value))
    });
    ui.add(div().flex().flex_col().children(lines.collect::<Vec<_>>()));
}
