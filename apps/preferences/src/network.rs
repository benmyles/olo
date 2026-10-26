//! Network: Wi-Fi networks in range (join, leave, forget) and each
//! interface's addresses. Talks to NetworkManager natively
//! (`tabula_system::network`) on a worker thread that reads its state every
//! two seconds, scans now and then, and makes the changes, so the window
//! never waits on it.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use tabula_kit::{
    egui::{Color32, Context, Rect, Stroke, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    widgets::{self, Button, CheckBox, Column, ColumnListView, ListState, MenuField, StringView, TabView, TextControl},
};
use tabula_system::network::{self as nm, Device, Kind, Network, Status};

enum Request {
    Wireless(bool),
    Join(String, Network, Option<String>),
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

#[derive(Default)]
pub struct State {
    tab: usize,
    list: ListState,
    /// The selected network's name (the list reorders as signals change).
    selected: Option<String>,
    password: String,
    device: usize,
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
    let mut join = resp.activated.is_some() && !net.active && !needs_password;
    if needs_password {
        let r = ui.add(TextControl::new(&mut st.password).label("Password:").divider(72.0).password(true));
        if r.lost_focus() && ui.input(|i| i.key_pressed(tabula_kit::egui::Key::Enter)) && st.password.len() >= 8 {
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
                let ready = !net.enterprise && (!needs_password || (8..=63).contains(&st.password.len()));
                if ui.add_enabled_ui(ready, |ui| ui.add(Button::new("Join").default(true))).inner.clicked() {
                    join = true;
                }
            }
            if net.known && ui.add(Button::new("Forget")).clicked() {
                let _ = link().tx.send(Request::Forget(net.ssid.clone()));
            }
        });
        if net.enterprise {
            ui.add(StringView::new("Enterprise networks aren't supported yet.").dim(true));
        }
    });
    if join && !busy && !net.active {
        let pw = needs_password.then(|| std::mem::take(&mut st.password));
        let _ = link().tx.send(Request::Join(dev.path.clone(), net, pw));
        st.clear_password();
    }
}

/// The tick beside the network in use.
fn check(p: &tabula_kit::egui::Painter, r: Rect, color: Color32) {
    let c0 = r.center();
    let pts = [c0 + vec2(-4.5, 0.0), c0 + vec2(-1.5, 3.5), c0 + vec2(4.5, -4.0)];
    p.line_segment([pts[0], pts[1]], Stroke::new(2.0, color));
    p.line_segment([pts[1], pts[2]], Stroke::new(2.0, color));
}

/// Four rising bars, lit to the signal's strength.
fn bars(p: &tabula_kit::egui::Painter, r: Rect, strength: u8, color: Color32) {
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
