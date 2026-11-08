//! Network: Wi-Fi networks in range (join, leave, forget), wired links
//! (connect, DHCP or their own addresses), the services' order, DNS of
//! one's own, /etc/hosts, traffic shaping, and each interface's
//! addresses. Talks to NetworkManager natively (`olo_system::network`)
//! on a worker thread that reads its state every two seconds, scans now
//! and then, and makes the changes, so the window never waits on it;
//! what needs root goes through `olo-net-admin` (`admin.rs`), started
//! through polkit once.

mod admin;
mod dns;
mod ethernet;
mod hosts_tab;
mod order;
mod shaping_tab;
mod vpn;

use olo_sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_system::{
    hosts::{Host, HostsFile},
    netconf::NetConf,
    network::{self as nm, Device, Enterprise, Kind, Network, Status},
    shaping::{self, Shaping},
};
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
    /// A wired link connected (its saved connection, or a new one).
    Connect(String),
    /// A connection's IPv4 settings, and its link given them.
    Addressing {
        device: String,
        profile: String,
        ipv4: nm::Addressing,
    },
    /// The services' order and DNS of one's own (root).
    Conf(NetConf),
    /// /etc/hosts as `text`, if it's still `was` (root).
    Hosts {
        text: String,
        was: String,
    },
    /// A link shaped, or not (root).
    Shape {
        interface: String,
        shaping: Shaping,
    },
    /// The panel closed: the root helper let go.
    EndAdmin,
    /// A VPN brought up (its saved connection).
    VpnUp(String),
    /// An active connection taken down (a VPN, the hotspot).
    Deactivate(String),
    /// A saved connection deleted (a VPN).
    Delete(String),
    /// A WireGuard configuration file saved as a VPN.
    ImportWireguard(PathBuf),
    /// The Wi-Fi adapter made a hotspot.
    StartHotspot {
        device: String,
        ssid: String,
        password: String,
    },
}

/// What the tabs past Wi-Fi show, read at the start, after each change
/// and as the links change (not polled).
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Extra {
    /// Olo's conf.d file: the order and DNS of one's own.
    pub conf: NetConf,
    /// /etc/hosts: its entries and its text as read (Err: unreadable).
    pub hosts: Option<Result<(Vec<Host>, String), String>>,
    /// Each link's shaping (its interface).
    pub shaping: Vec<(String, Shaping)>,
    /// Each wired link's connection (its path, its saved connection) and
    /// that one's IPv4 settings.
    pub ipv4: Vec<(String, Option<String>, nm::Addressing)>,
    /// The VPNs saved, and whether each is up.
    pub vpns: Vec<nm::Vpn>,
    /// The hotspot this computer makes, if it's on.
    pub hotspot: Option<nm::Hotspot>,
}

fn read_extra(status: Option<&Status>) -> Extra {
    let devices: Vec<&Device> = status.map(|s| s.devices.iter().collect()).unwrap_or_default();
    Extra {
        conf: NetConf::read(),
        hosts: Some(HostsFile::read().map(|(f, text)| (f.hosts(), text)).map_err(|e| e.to_string())),
        shaping: devices.iter().map(|d| (d.interface.clone(), shaping::read(&d.interface).unwrap_or_default())).collect(),
        ipv4: devices
            .iter()
            .filter(|d| d.kind == Kind::Ethernet)
            .map(|d| {
                let profile = nm::profile_of(&d.path).ok().flatten();
                let ipv4 = profile.as_deref().and_then(|p| nm::profile_addressing(p).ok()).unwrap_or_default();
                (d.path.clone(), profile, ipv4)
            })
            .collect(),
        vpns: nm::vpns().unwrap_or_default(),
        hotspot: nm::hotspot().ok().flatten(),
    }
}

/// What the links are, as far as what [`Extra`] reads follows them: each
/// device's state and connection, and the connections up (VPNs too).
fn links_key(status: &Option<Result<Status, String>>) -> Vec<(String, u32, Option<String>)> {
    match status {
        Some(Ok(s)) => {
            let mut key: Vec<_> = s.devices.iter().map(|d| (d.path.clone(), d.state, d.connection.clone())).collect();
            key.extend(nm::active_connections().unwrap_or_default().into_iter().map(|a| (a, 0, None)));
            key
        }
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct Shared {
    /// None until first read; Err when NetworkManager isn't there.
    status: Option<Result<Status, String>>,
    extra: Extra,
    busy: bool,
    error: Option<String>,
    /// What the last change did, when there's something to say.
    done: Option<String>,
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
                let mut admin = admin::Admin::default();
                let mut links: Option<Vec<(String, u32, Option<String>)>> = None;
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
                    // What the other tabs show, as the links change.
                    let key = links_key(&status);
                    let extra = (links.as_ref() != Some(&key)).then(|| read_extra(status.as_ref().and_then(|s| s.as_ref().ok())));
                    links = Some(key);
                    let changed = {
                        let mut s = sh.lock().unwrap();
                        let extra_changed = extra.is_some_and(|e| std::mem::replace(&mut s.extra, e) != s.extra);
                        (std::mem::replace(&mut s.status, status.clone()) != status) | extra_changed
                    };
                    if changed {
                        wake();
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    if matches!(req, Request::EndAdmin) {
                        admin.end();
                        continue;
                    }
                    {
                        let mut s = sh.lock().unwrap();
                        s.busy = true;
                        s.done = None;
                    }
                    wake();
                    let mut done = None;
                    // The root helper's own words, when it said no.
                    let mut said: Option<String> = None;
                    // (The same error as the other requests', large or not.)
                    #[allow(clippy::result_large_err)]
                    let mut ask = |request: serde_json::Value| {
                        admin.ask(&request).map_err(|e| {
                            said = Some(e.clone());
                            (olo_system::Error::Failure(e), "")
                        })
                    };
                    let r = match req {
                        Request::EndAdmin => Ok(()),
                        Request::VpnUp(p) => nm::vpn_up(&p).map_err(|e| (e, "The VPN couldn't be connected.")),
                        Request::Deactivate(a) => nm::deactivate(&a).map_err(|e| (e, "It couldn't be disconnected.")),
                        Request::Delete(p) => nm::delete_profile(&p).map_err(|e| (e, "The VPN couldn't be removed.")),
                        Request::ImportWireguard(path) => {
                            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            done = Some(format!("“{name}” is imported."));
                            match std::fs::read_to_string(&path) {
                                Ok(text) => nm::import_wireguard(&name, &text).map_err(|e| {
                                    said = Some(match &e {
                                        olo_system::Error::Failure(why) => format!("It isn't a WireGuard configuration: {why}."),
                                        _ => "The VPN couldn't be saved.".to_owned(),
                                    });
                                    (e, "")
                                }),
                                Err(e) => Err((olo_system::Error::Failure(e.to_string()), "The file couldn't be read.")),
                            }
                        }
                        Request::StartHotspot { device, ssid, password } => nm::start_hotspot(&device, &ssid, &password).map_err(|e| (e, "The hotspot couldn't be started.")),
                        Request::Connect(dev) => nm::connect(&dev).map_err(|e| (e, "The link couldn't be connected.")),
                        Request::Addressing { device, profile, ipv4 } => {
                            // As this account, as NetworkManager lets it; else
                            // as root.
                            let own = nm::update_profile(&profile, |s| nm::set_addressing_in(s, &ipv4)).and_then(|_| nm::reapply(&device));
                            match own {
                                Err(olo_system::Error::MethodError(name, ..)) if name.as_str().ends_with("PermissionDenied") => {
                                    ask(serde_json::json!({ "profile": { "profile": profile, "ipv4": ipv4, "device": device } }))
                                }
                                r => r.map_err(|e| (e, "The connection couldn't be changed.")),
                            }
                        }
                        Request::Conf(c) => {
                            done = Some("Applied.".to_owned());
                            ask(serde_json::json!({ "conf": c }))
                        }
                        Request::Hosts { text, was } => {
                            done = Some("Saved to /etc/hosts.".to_owned());
                            ask(serde_json::json!({ "hosts": { "text": text, "was": was } }))
                        }
                        Request::Shape { interface, shaping } => {
                            done = Some(if shaping.is_on() { format!("Shaping {interface}.") } else { format!("{interface} isn't shaped.") });
                            ask(serde_json::json!({ "shape": { "interface": interface, "shaping": shaping } }))
                        }
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
                    let ok = r.is_ok();
                    let error = r.err().map(|(e, what)| {
                        tracing::warn!("NetworkManager: {e}");
                        said.take().unwrap_or_else(|| what.to_owned())
                    });
                    // What the change changed, read at once.
                    let status = nm::status().map_err(|e| e.to_string());
                    let extra = read_extra(status.as_ref().ok());
                    let mut s = sh.lock().unwrap();
                    s.busy = false;
                    s.error = error;
                    s.done = done.filter(|_| ok);
                    s.extra = extra;
                    s.status = Some(status);
                    links = Some(links_key(&s.status));
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
            _ => None,
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
    eth: ethernet::Edit,
    order: order::Edit,
    dns: dns::Edit,
    hosts: hosts_tab::Edit,
    shape: shaping_tab::Edit,
    vpn: vpn::Edit,
    hotspot_name: Entity<TextField>,
    hotspot_password: Entity<TextField>,
}

impl Drop for State {
    /// The panel closed: the root helper, if it was started, let go.
    fn drop(&mut self) {
        let _ = self.link.tx.send(Request::EndAdmin);
    }
}

/// The tabs, as `--tab` names them too.
const TABS: [&str; 8] = ["Wi-Fi", "Ethernet", "VPN", "Service Order", "DNS", "Hosts", "Shaping", "Details"];

/// A text field that redraws the panel as it's typed in.
fn field(cx: &mut Context<PanelView>, hint: &str) -> Entity<TextField> {
    let hint = hint.to_owned();
    let f = olo_ui::widgets::text_field(cx, |f| f.hint(hint));
    cx.subscribe(&f, |_, _, e: &TextFieldEvent, cx| {
        if matches!(e, TextFieldEvent::Changed) {
            cx.notify();
        }
    })
    .detach();
    f
}

/// Label and value rows, the labels in their column as the kit's fields
/// and menus have theirs (on the left, the values at the divider).
fn rows(ui: &mut Ui, divider: f32, rows: &[(&str, String)]) {
    let lines: Vec<_> = rows.iter().map(|(label, value)| labelled(divider, label, StringView::new(value.clone()))).collect();
    ui.add(div().flex().flex_col().children(lines));
}

/// `label` in the label column, `then` from the divider on.
fn labelled(divider: f32, label: &str, then: impl IntoElement) -> olo_ui::gpui::Div {
    div().flex().flex_row().items_center().child(div().w(px(divider)).flex_shrink_0().child(StringView::new(label.to_owned()))).child(div().flex_1().min_w(px(0.0)).child(then))
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
        State {
            tab: 0,
            list: ListModel::new(cx, false),
            selected: None,
            password,
            device: 0,
            link: link(olo_ui::proxy(cx)),
            eth: ethernet::Edit::new(cx),
            order: order::Edit::new(cx),
            dns: dns::Edit::new(cx),
            hosts: hosts_tab::Edit::new(cx),
            shape: shaping_tab::Edit::new(cx),
            vpn: vpn::Edit::new(cx),
            hotspot_name: {
                let f = field(cx, "Network name");
                let name = host_name();
                f.update(cx, |f, cx| f.set_text(name, cx));
                f
            },
            hotspot_password: {
                let f = field(cx, "8+ characters");
                f.update(cx, |f, cx| f.set_password(true, cx));
                f
            },
        }
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

/// Open at the tab named `name` (`olo-preferences network --tab
/// Ethernet`, Deskbar's Ethernet menu).
pub fn show_tab(st: &mut State, name: &str) {
    if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(name)) {
        st.tab = i;
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let (status, extra, busy, error, done) = {
        let sh = st.link.shared.lock().unwrap();
        (sh.status.clone(), sh.extra.clone(), sh.busy, sh.error.clone(), sh.done.clone())
    };
    let status = match status {
        None => return ui.add(EmptyState::new("Looking for network devices…").icon("network-prefs")),
        Some(Err(_)) => return ui.add(EmptyState::new("NetworkManager isn't running.").icon("network-prefs")),
        Some(Ok(s)) => s,
    };
    let items = ui.collect(|ui| {
        match TABS.get(st.tab).copied().unwrap_or_default() {
            "Wi-Fi" => {
                wifi(ui, st, &status, busy);
                hotspot(ui, st, &status, &extra, busy);
            }
            "Ethernet" => ethernet::ui(ui, st, &status, &extra, busy),
            "VPN" => vpn::ui(ui, st, &extra, busy),
            "Service Order" => order::ui(ui, st, &status, &extra, busy),
            "DNS" => dns::ui(ui, st, &status, &extra, busy),
            "Hosts" => hosts_tab::ui(ui, st, &extra, busy),
            "Shaping" => shaping_tab::ui(ui, st, &status, &extra, busy),
            _ => details(ui, st, &status),
        }
        if busy {
            ui.note("Working…");
        } else if let Some(e) = &error {
            ui.label(e.clone());
        } else if let Some(d) = &done {
            ui.note(d.clone());
        }
    });
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.tab = i;
            // (What the last change said was about the tab it was on.)
            let mut sh = st.link.shared.lock().unwrap();
            sh.done = None;
            sh.error = None;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
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
        if name == "Olo Hotspot" { "Making a hotspot (below)".to_owned() } else { format!("Connected to “{name}”") }
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
            // (As tall as the line beside it: their middles one, the two
            // level at 1.25×.)
            .child(CheckBox::new("Wi-Fi", status.wireless_enabled).height(m::CONTROL_HEIGHT).enabled(status.wireless_hardware && !busy).on_toggle(move |on, _, _| {
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

// ---- Hotspot ---------------------------------------------------------------------

/// This computer's name, for its hotspot's.
fn host_name() -> String {
    let mut b = [0u8; 64];
    // SAFETY: into a buffer of its length.
    let ok = unsafe { libc::gethostname(b.as_mut_ptr() as *mut libc::c_char, b.len()) } == 0;
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    if ok { String::from_utf8_lossy(&b[..n]).split('.').next().unwrap_or_default().to_owned() } else { String::new() }
}

/// The Wi-Fi tab's hotspot: this computer's connection shared as a Wi-Fi
/// network of its own (WPA2), or, while it is, what it's called and Stop.
fn hotspot(ui: &mut Ui, st: &mut State, status: &Status, extra: &Extra, busy: bool) {
    let Some(dev) = status.devices.iter().find(|d| d.kind == Kind::Wifi).cloned() else { return };
    if !status.wireless_enabled {
        return;
    }
    let link = st.link;
    let (name, password) = (st.hotspot_name.clone(), st.hotspot_password.clone());
    ui.bbox("Hotspot", |ui| match &extra.hotspot {
        Some(h) => {
            let active = h.active.clone();
            ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StringView::new(format!("Sharing this computer's connection as “{}”.", h.ssid)))).child(
                Button::new("Stop").enabled(!busy).on_click(move |_, _| {
                    let _ = link.tx.send(Request::Deactivate(active.clone()));
                }),
            ));
        }
        None => {
            let ready = !name.read(ui.cx).text().trim().is_empty() && (8..=63).contains(&password.read(ui.cx).text().len());
            let (n, p, device) = (name.clone(), password.clone(), dev.path.clone());
            let start = move |_: &mut Window, cx: &mut App| {
                let ssid = n.read(cx).text().trim().to_owned();
                let pw = p.read(cx).text().to_owned();
                let _ = link.tx.send(Request::StartHotspot { device: device.clone(), ssid, password: pw });
            };
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING * 2.0))
                    .child(div().flex_1().child(TextControl::new(name).label("Name:").divider(60.0)))
                    .child(div().flex_1().child(TextControl::new(password).label("Password:").divider(80.0)))
                    .child(Button::new("Start").enabled(ready && !busy).on_click(start)),
            );
            ui.note("Shares this computer's connection as a Wi-Fi network of its own; the adapter leaves the network it's on while it does.");
        }
    });
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
pub(super) fn check(window: &mut Window, r: Bounds<Pixels>, color: Hsla) {
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
    // (On the 4-point grid, as Ethernet's.)
    const DIVIDER: f32 = 132.0;
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
    let rows_of = [
        ("Status:", d.state_text().to_owned()),
        ("Connection:", d.connection.clone().unwrap_or_else(|| none.clone())),
        ("IP address:", if d.addresses.is_empty() { none.clone() } else { d.addresses.join(", ") }),
        ("Router:", d.gateway.clone().unwrap_or_else(|| none.clone())),
        ("DNS servers:", if d.dns.is_empty() { none.clone() } else { d.dns.join(", ") }),
        ("Hardware address:", if d.hw_address.is_empty() { none.clone() } else { d.hw_address.clone() }),
        ("Speed:", d.speed.map_or(none.clone(), nm::speed_text)),
    ];
    rows(ui, DIVIDER, &rows_of);
}
