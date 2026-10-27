//! Network: Wi-Fi networks in range (join, leave, forget) and each
//! interface's addresses. Talks to NetworkManager natively
//! (`olo_system::network`) on a worker thread that reads its state every
//! two seconds, scans now and then, and makes the changes, so the window
//! never waits on it.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{Color32, Context, Rect, Stroke, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    widgets::{self, Button, CheckBox, Column, ColumnListView, ListState, MenuField, StringView, TabView, TextControl},
};
use olo_system::network::{self as nm, Device, Enterprise, Kind, Network, Status};

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
    contexts: Arc<Mutex<Vec<Context>>>,
}

/// How often to ask the Wi-Fi adapter for a fresh look around.
const SCAN_EVERY: Duration = Duration::from_secs(30);

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        let contexts: Arc<Mutex<Vec<Context>>> = Default::default();
        let (tx, rx) = channel::<Request>();
        let (sh, ctxs) = (shared.clone(), contexts.clone());
        std::thread::Builder::new()
            .name("network".into())
            .spawn(move || {
                let wake = || ctxs.lock().unwrap().iter().for_each(|c| c.request_repaint());
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
        Link { shared, tx, contexts }
    })
}

/// `OLO_FAKE_NETWORKS=1` (debug builds): made-up Wi-Fi networks, so
/// the panel and the enterprise sheet can be tried headless without
/// touching NetworkManager or showing the networks around the machine.
fn fake_networks() -> bool {
    cfg!(debug_assertions) && std::env::var_os("OLO_FAKE_NETWORKS").is_some()
}

fn fake_worker(sh: &Arc<Mutex<Shared>>, wake: &dyn Fn(), rx: &std::sync::mpsc::Receiver<Request>) {
    let net = |ssid: &str, strength, secure, enterprise| Network { ssid: ssid.into(), strength, secure, enterprise, ap: format!("/ap/{ssid}"), active: false, known: false };
    let mut status = Status {
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
    };
    loop {
        sh.lock().unwrap().status = Some(Ok(status.clone()));
        wake();
        let Ok(req) = rx.recv() else { return };
        let joined = match req {
            Request::Join(_, n, _) => Some(n.ssid),
            Request::JoinEnterprise(_, n, e) => {
                tracing::info!("fake join of {}: {:?}", n.ssid, nm::enterprise_settings(&Enterprise { password: "…".into(), key_password: String::new(), ..*e }).get("802-1x").map(|x| x.keys().collect::<Vec<_>>()));
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

#[derive(Default)]
pub struct State {
    tab: usize,
    list: ListState,
    /// The selected network's name (the list reorders as signals change).
    selected: Option<String>,
    password: String,
    device: usize,
    /// Join was clicked on a new enterprise network: main opens the sheet
    /// that asks for its credentials (device path, network).
    pub enterprise: Option<(String, Network)>,
}

impl State {
    fn clear_password(&mut self) {
        // SAFETY: zero bytes keep the string valid UTF-8.
        unsafe { self.password.as_bytes_mut().fill(0) };
        self.password.clear();
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = link();
    {
        let mut ctxs = link.contexts.lock().unwrap();
        if !ctxs.iter().any(|c| c == ui.ctx()) {
            ctxs.push(ui.ctx().clone());
        }
    }
    let (status, busy, error) = {
        let sh = link.shared.lock().unwrap();
        (sh.status.clone(), sh.busy, sh.error.clone())
    };
    let status = match status {
        None => {
            ui.add(StringView::new("Looking for network devices…").dim(true));
            return;
        }
        Some(Err(_)) => {
            ui.add(StringView::new("NetworkManager isn't running.").dim(true));
            return;
        }
        Some(Ok(s)) => s,
    };
    let mut tab = st.tab;
    TabView::new(&mut tab, &["Wi-Fi", "Details"]).show(ui, |ui, tab| {
        ui.spacing_mut().item_spacing.y = m::SPACING;
        match tab {
            0 => wifi(ui, st, &status, busy),
            _ => details(ui, st, &status),
        }
        if busy {
            ui.add(StringView::new("Working…").dim(true));
        } else if let Some(e) = &error {
            ui.add(StringView::new(e));
        }
    });
    st.tab = tab;
}

// ---- Wi-Fi ---------------------------------------------------------------------

fn wifi(ui: &mut Ui, st: &mut State, status: &Status, busy: bool) {
    let Some(dev) = status.devices.iter().find(|d| d.kind == Kind::Wifi) else {
        ui.add(StringView::new("There's no Wi-Fi adapter.").dim(true));
        return;
    };
    let mut on = status.wireless_enabled;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(status.wireless_hardware && !busy, |ui| ui.add(CheckBox::new(&mut on, "Wi-Fi")));
        ui.add_space(m::SPACING * 2.0);
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
        ui.add(StringView::new(&text).dim(true));
    });
    if on != status.wireless_enabled {
        let _ = link().tx.send(Request::Wireless(on));
    }
    if !status.wireless_enabled {
        return;
    }

    let nets = &status.networks;
    // Keep the selection on the same network as the list reorders.
    st.list.selected.clear();
    if let Some(i) = st.selected.as_ref().and_then(|s| nets.iter().position(|n| n.ssid == *s)) {
        st.list.select(i);
    }
    let columns = [Column::new("", 22.0), Column::new("Network", 210.0), Column::new("Security", 100.0), Column::new("Signal", 50.0)];
    let list = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), (ui.available_height() - 3.0 * (m::CONTROL_HEIGHT + m::SPACING)).max(90.0)));
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(list), |ui| {
            ColumnListView::new("networks", &columns, nets.len(), &mut st.list).show(ui, |cell| {
                let n = &nets[cell.row];
                match cell.column {
                    0 if n.active => check(cell.painter, cell.rect, cell.text_color),
                    1 => cell.text(&n.ssid),
                    2 => cell.text(match (n.enterprise, n.secure, n.known) {
                        (true, ..) => "Enterprise",
                        (_, true, true) => "Secured, saved",
                        (_, true, false) => "Secured",
                        (_, false, true) => "Open, saved",
                        (_, false, false) => "Open",
                    }),
                    3 => bars(cell.painter, cell.rect, n.strength, cell.text_color),
                    _ => {}
                }
            })
        })
        .inner;
    ui.advance_cursor_after_rect(list);
    let picked = st.list.single().and_then(|i| nets.get(i)).cloned();
    if resp.selection_changed {
        st.selected = picked.as_ref().map(|n| n.ssid.clone());
        st.clear_password();
    }
    let Some(net) = picked else {
        ui.add(StringView::new("Choose a network to join it.").dim(true));
        return;
    };
    let needs_password = net.secure && !net.known && !net.active && !net.enterprise;
    // A new enterprise network asks for more than fits here: a sheet.
    let needs_sheet = net.enterprise && !net.known && !net.active;
    let mut join = resp.activated.is_some() && !net.active && !needs_password;
    if needs_password {
        let r = ui.add(TextControl::new(&mut st.password).label("Password:").divider(72.0).password(true));
        if r.lost_focus() && ui.input(|i| i.key_pressed(olo_kit::egui::Key::Enter)) && st.password.len() >= 8 {
            join = true;
        }
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING;
        ui.add_enabled_ui(!busy, |ui| {
            if net.active {
                if ui.add(Button::new("Disconnect")).clicked() {
                    let _ = link().tx.send(Request::Disconnect(dev.path.clone()));
                }
            } else {
                // WPA passwords are 8 to 63 characters.
                let ready = !needs_password || (8..=63).contains(&st.password.len());
                let label = if needs_sheet { "Join…" } else { "Join" };
                if ui.add_enabled_ui(ready, |ui| ui.add(Button::new(label).default(true))).inner.clicked() {
                    join = true;
                }
            }
            if net.known && ui.add(Button::new("Forget")).clicked() {
                let _ = link().tx.send(Request::Forget(net.ssid.clone()));
            }
        });
    });
    if join && !busy && needs_sheet {
        st.enterprise = Some((dev.path.clone(), net));
    } else if join && !busy && !net.active {
        let pw = needs_password.then(|| std::mem::take(&mut st.password));
        let _ = link().tx.send(Request::Join(dev.path.clone(), net, pw));
        st.clear_password();
    }
}

// ---- Enterprise sheet ------------------------------------------------------------

/// A file the sheet needs: main opens a panel and hands back the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertSlot {
    Ca,
    Client,
    Key,
}

impl CertSlot {
    pub fn title(self) -> &'static str {
        match self {
            CertSlot::Ca => "Choose a CA Certificate",
            CertSlot::Client => "Choose Your Certificate",
            CertSlot::Key => "Choose Your Private Key",
        }
    }
}

/// What the sheet asks of main.
pub enum SheetResult {
    Close,
    Choose(CertSlot),
}

/// Joining a WPA-Enterprise (802.1X) network: method, credentials and how
/// to trust the server, as macOS asks.
pub struct EnterpriseForm {
    device: String,
    net: Network,
    e: nm::Enterprise,
    eap: usize,
    inner: usize,
    /// System certificates, a file, or don't verify.
    ca: usize,
    ca_file: Option<std::path::PathBuf>,
}

pub const SHEET_WIDTH: u32 = 440;

/// Certificates and keys, as PEM, DER or PKCS#12.
pub const CERT_TYPES: &[&str] = &["pem", "crt", "cer", "der", "key", "p12", "pfx"];
const SHEET_DIVIDER: f32 = 132.0;

impl EnterpriseForm {
    pub fn new(device: String, net: Network) -> EnterpriseForm {
        EnterpriseForm { device, net, e: nm::Enterprise::default(), eap: 0, inner: 0, ca: 0, ca_file: None }
    }

    /// The window height its rows need (they change with the method).
    pub fn height(&self) -> u32 {
        let rows = match (self.e.eap, self.ca) {
            (nm::Eap::Tls, 1) => 10,
            (nm::Eap::Tls, _) => 9,
            (_, 1) => 9,
            _ => 8,
        };
        (rows as f32 * (m::CONTROL_HEIGHT + m::SPACING) + m::SPACING * 2.0 + m::CONTROL_HEIGHT + 2.0 * m::WINDOW_INSET).round() as u32
    }

    pub fn ssid(&self) -> &str {
        &self.net.ssid
    }

    pub fn chosen(&mut self, slot: CertSlot, path: std::path::PathBuf) {
        match slot {
            CertSlot::Ca => self.ca_file = Some(path),
            CertSlot::Client => self.e.client_cert = Some(path),
            CertSlot::Key => self.e.private_key = Some(path),
        }
    }

    fn clear_secrets(&mut self) {
        for s in [&mut self.e.password, &mut self.e.key_password] {
            // SAFETY: zero bytes keep the string valid UTF-8.
            unsafe { s.as_bytes_mut().fill(0) };
            s.clear();
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<SheetResult> {
        let p = olo_kit::style::palette(ui.ctx());
        ui.painter().rect_filled(ui.max_rect(), 0.0, olo_kit::style::c(p.panel_background));
        let inner_rect = ui.max_rect().shrink(m::WINDOW_INSET);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner_rect));
        let ui = &mut ui;
        ui.spacing_mut().item_spacing.y = m::SPACING;
        let mut result = None;
        ui.add(StringView::new(&format!("“{}” is an enterprise network.", self.net.ssid)));
        let eaps: Vec<&str> = nm::Eap::ALL.iter().map(|e| e.name()).collect();
        ui.add(MenuField::new("ent-eap", &mut self.eap, &eaps).label("Security:").divider(SHEET_DIVIDER).width(ui.available_width()));
        self.e.eap = nm::Eap::ALL[self.eap.min(2)];
        let tls = self.e.eap == nm::Eap::Tls;
        if tls {
            ui.add(TextControl::new(&mut self.e.identity).label("Identity:").divider(SHEET_DIVIDER));
            if file_row(ui, "Certificate:", self.e.client_cert.as_deref()) {
                result = Some(SheetResult::Choose(CertSlot::Client));
            }
            if file_row(ui, "Private key:", self.e.private_key.as_deref()) {
                result = Some(SheetResult::Choose(CertSlot::Key));
            }
            ui.add(TextControl::new(&mut self.e.key_password).label("Key password:").divider(SHEET_DIVIDER).password(true));
        } else {
            let names: Vec<&str> = self.e.eap.inner().iter().map(|i| inner_name(i)).collect();
            self.inner = self.inner.min(names.len() - 1);
            ui.add(MenuField::new("ent-inner", &mut self.inner, &names).label("Authentication:").divider(SHEET_DIVIDER).width(ui.available_width()));
            self.e.inner = self.e.eap.inner()[self.inner].to_owned();
            ui.add(TextControl::new(&mut self.e.identity).label("Username:").divider(SHEET_DIVIDER).autofocus(true));
            ui.add(TextControl::new(&mut self.e.password).label("Password:").divider(SHEET_DIVIDER).password(true));
        }
        ui.add(TextControl::new(&mut self.e.anonymous_identity).label("Anonymous identity:").divider(SHEET_DIVIDER).hint("Optional"));
        let cas = ["System certificates", "A certificate file", "Don't verify"];
        ui.add(MenuField::new("ent-ca", &mut self.ca, &cas).label("CA certificate:").divider(SHEET_DIVIDER).width(ui.available_width()));
        if self.ca == 1 && file_row(ui, "", self.ca_file.as_deref()) {
            result = Some(SheetResult::Choose(CertSlot::Ca));
        }
        self.e.ca = match self.ca {
            0 => nm::CaCert::System,
            1 => nm::CaCert::File(self.ca_file.clone().unwrap_or_default()),
            _ => nm::CaCert::None,
        };
        ui.add_enabled_ui(self.ca != 2, |ui| ui.add(TextControl::new(&mut self.e.domain).label("Domain:").divider(SHEET_DIVIDER).hint("Optional, e.g. example.edu")));
        if self.ca == 2 {
            self.e.domain.clear();
        }
        // Buttons at the bottom right, Join the default.
        let ready = self.e.ready();
        let enter = ready && ui.input(|i| i.key_pressed(olo_kit::egui::Key::Enter));
        let bar = Rect::from_min_max(pos2(inner_rect.left(), inner_rect.bottom() - m::CONTROL_HEIGHT), inner_rect.max);
        let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(bar).layout(olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center)));
        bar_ui.spacing_mut().item_spacing.x = m::SPACING;
        let join = bar_ui.add_enabled_ui(ready, |ui| ui.add(Button::new("Join").default(true))).inner.clicked() || enter;
        let cancel = bar_ui.add(Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(olo_kit::egui::Key::Escape));
        if join {
            let e = Box::new(std::mem::take(&mut self.e));
            let _ = link().tx.send(Request::JoinEnterprise(self.device.clone(), self.net.clone(), e));
            return Some(SheetResult::Close);
        }
        if cancel {
            self.clear_secrets();
            return Some(SheetResult::Close);
        }
        result
    }
}

impl Drop for EnterpriseForm {
    fn drop(&mut self) {
        self.clear_secrets();
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

/// A label, the chosen file's name (or "None") and Choose…; true when
/// Choose… is clicked.
fn file_row(ui: &mut Ui, label: &str, path: Option<&std::path::Path>) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), m::CONTROL_HEIGHT), olo_kit::egui::Sense::hover());
    let mut row = ui.new_child(UiBuilder::new().max_rect(rect).layout(olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center)));
    let clicked = row.add(Button::new("Choose…")).clicked();
    row.add_space(m::SPACING);
    let rest = Rect::from_min_max(rect.min, pos2(row.cursor().max.x, rect.bottom()));
    let mut left = ui.new_child(UiBuilder::new().max_rect(rest).layout(olo_kit::egui::Layout::left_to_right(olo_kit::egui::Align::Center)));
    left.spacing_mut().item_spacing.x = 0.0;
    left.add(StringView::new(label));
    left.add_space((rect.left() + SHEET_DIVIDER - left.cursor().min.x).max(0.0));
    let name = path.and_then(|p| p.file_name()).map_or("None".into(), |n| n.to_string_lossy().into_owned());
    left.add(StringView::new(&name).dim(path.is_none()));
    clicked
}

/// The tick beside the network in use.
fn check(p: &olo_kit::egui::Painter, r: Rect, color: Color32) {
    let c0 = r.center();
    let pts = [c0 + vec2(-4.5, 0.0), c0 + vec2(-1.5, 3.5), c0 + vec2(4.5, -4.0)];
    p.line_segment([pts[0], pts[1]], Stroke::new(2.0, color));
    p.line_segment([pts[1], pts[2]], Stroke::new(2.0, color));
}

/// Four rising bars, lit to the signal's strength.
fn bars(p: &olo_kit::egui::Painter, r: Rect, strength: u8, color: Color32) {
    let lit = match strength {
        0..=20 => 1,
        21..=45 => 2,
        46..=70 => 3,
        _ => 4,
    };
    let x0 = r.left() + m::CELL_INSET;
    let bottom = r.center().y + 5.0;
    for i in 0..4 {
        let h = 3.0 + i as f32 * 2.5;
        let bar = Rect::from_min_max(pos2(x0 + i as f32 * 5.0, bottom - h), pos2(x0 + i as f32 * 5.0 + 3.0, bottom));
        let fill = if i < lit { color } else { color.gamma_multiply(0.25) };
        p.rect_filled(bar, 0.0, fill);
    }
}

// ---- Details -------------------------------------------------------------------

fn details(ui: &mut Ui, st: &mut State, status: &Status) {
    if status.devices.is_empty() {
        ui.add(StringView::new("There are no network interfaces.").dim(true));
        return;
    }
    let names: Vec<String> = status
        .devices
        .iter()
        .map(|d| format!("{} ({})", d.interface, if d.kind == Kind::Wifi { "Wi-Fi" } else { "Ethernet" }))
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    st.device = st.device.min(names.len() - 1);
    const DIVIDER: f32 = 130.0;
    ui.add(MenuField::new("network-device", &mut st.device, &names).label("Interface:").divider(DIVIDER).width(ui.available_width()));
    widgets::separator(ui);
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
    ui.spacing_mut().item_spacing.y = 0.0;
    for (label, value) in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            widgets::form_label(ui, label, DIVIDER);
            ui.add(StringView::new(&value));
        });
    }
}
