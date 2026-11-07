//! Bluetooth: turn it on or off, look for devices, pair, connect and
//! forget them. Talks to BlueZ natively (`olo_system::bluetooth`): one
//! thread reads its state every second and a thread per action does the
//! slow parts (pairing can wait on the user typing a passkey), so the
//! window never waits. A pairing agent registered for this process puts
//! BlueZ's questions (passkeys, PINs) in the panel.

use std::{
    collections::HashMap,
    sync::{
        Arc, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_sync::Mutex;
use olo_system::bluetooth::{self as bt, Agent, AgentEvent, Ask, Question, Status};
use olo_ui::{
    gpui::{AppContext as _, Context, Entity, ParentElement as _, Styled as _, div, px},
    metrics as m,
    widgets::{Button, CheckBox, Column, ColumnListView, EmptyState, ListModel, StringView, TextControl, TextField, TextFieldEvent},
};

use crate::{Msg, PanelView, form::Ui, panel};

/// How long a search for devices runs.
const SEARCH: Duration = Duration::from_secs(30);

enum Request {
    Power(String, bool),
    Search(String),
}

#[derive(Default)]
struct Shared {
    /// None until first read; Err when BlueZ isn't running.
    status: Option<Result<Status, String>>,
    /// What's being done to which device ("Pairing…").
    doing: HashMap<String, &'static str>,
    question: Option<Question>,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
    proxy: olo_ui::Proxy,
}

/// Test data: this computer's adapter, two paired devices and one nearby.
fn test_status() -> Status {
    let device = |name: &str, icon: &str, paired, connected, battery| bt::Device {
        path: format!("/org/bluez/hci0/{name}"),
        name: name.into(),
        address: "02:00:00:00:A1:02".into(),
        icon: icon.into(),
        named: true,
        paired,
        connected,
        trusted: paired,
        rssi: Some(-61),
        battery,
        modalias: None,
    };
    Status {
        adapter: Some(bt::Adapter { path: "/org/bluez/hci0".into(), name: "studio-laptop".into(), address: "02:00:00:00:A1:03".into(), powered: true, discoverable: false, discovering: true }),
        devices: vec![
            device("Sony WH-1000XM5 Wireless Noise Canceling Headphones", "audio-headset", true, true, Some(80)),
            device("Logitech MX Master 3S for Mac", "input-mouse", true, false, Some(45)),
            device("Keychron Q1 Pro QMK/VIA Wireless Custom Mechanical Keyboard", "input-keyboard", false, false, None),
            // The AirPods panel's test pair (their model and levels).
            bt::Device {
                address: "02:00:00:00:A1:01".into(),
                modalias: Some("bluetooth:v004Cp2024d215C".into()),
                ..device("Travel AirPods Pro (2nd generation) for the Office", "audio-headphones", true, true, None)
            },
        ],
    }
}

/// BlueZ's watcher and pairing agent (one for the app), telling the
/// windows through `proxy` when anything changes.
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): an adapter and devices named as long
        // as real ones, and never the real adapter (a search on it).
        if cfg!(test) {
            shared.lock().unwrap().status = Some(Ok(test_status()));
            return Link { shared, tx: channel().0, proxy };
        }
        let (tx, rx) = channel::<Request>();
        let (sh, out) = (shared.clone(), proxy.clone());
        std::thread::Builder::new()
            .name("bluetooth".into())
            .spawn(move || {
                let proxy = out;
                let agent = {
                    let (sh, proxy) = (sh.clone(), proxy.clone());
                    Agent::register(move |e| {
                        sh.lock().unwrap().question = match e {
                            AgentEvent::Ask(q) => Some(q),
                            AgentEvent::Cancel => None,
                        };
                        proxy.send(Msg::Redraw);
                    })
                };
                if let Err(e) = &agent {
                    tracing::warn!("bluetooth agent: {e}");
                }
                let mut searching: Option<(String, Instant)> = None;
                loop {
                    if let Some((adapter, at)) = &searching
                        && at.elapsed() > SEARCH
                    {
                        let _ = bt::stop_discovery(adapter);
                        searching = None;
                    }
                    let status = Some(bt::status().map_err(|e| e.to_string()));
                    let changed = {
                        let mut s = sh.lock().unwrap();
                        std::mem::replace(&mut s.status, status.clone()) != status
                    };
                    if changed {
                        proxy.send(Msg::Redraw);
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let r = match req {
                        Request::Power(adapter, on) => bt::set_powered(&adapter, on).map_err(|e| (e, "Bluetooth couldn't be turned on or off.")),
                        Request::Search(adapter) => {
                            let r = bt::start_discovery(&adapter);
                            searching = Some((adapter, Instant::now()));
                            // Already searching is fine.
                            r.or_else(|e| if e.to_string().contains("InProgress") { Ok(()) } else { Err(e) }).map_err(|e| (e, "Couldn't search for devices."))
                        }
                    };
                    sh.lock().unwrap().error = r.err().map(|(e, what)| {
                        tracing::warn!("bluez: {e}");
                        what.to_owned()
                    });
                    proxy.send(Msg::Redraw);
                }
                drop(agent);
            })
            .expect("thread");
        Link { shared, tx, proxy }
    })
}

/// Do something slow to a device on its own thread.
fn act(link: &'static Link, device: String, doing: &'static str, failed: &'static str, f: impl FnOnce(&str) -> olo_system::Result<()> + Send + 'static) {
    link.shared.lock().unwrap().doing.insert(device.clone(), doing);
    link.proxy.send(Msg::Redraw);
    std::thread::spawn(move || {
        let r = f(&device);
        let mut s = link.shared.lock().unwrap();
        s.doing.remove(&device);
        if s.question.as_ref().is_some_and(|q| q.path == device) {
            s.question = None;
        }
        s.error = r.err().map(|e| {
            tracing::warn!("bluez: {e}");
            failed.to_owned()
        });
        drop(s);
        link.proxy.send(Msg::Redraw);
    });
}

fn cancel_pairing(device: String) {
    std::thread::spawn(move || {
        let _ = bt::cancel_pairing(&device);
    });
}

/// Answers BlueZ's question (None: no), and clears the code typed.
fn answer(link: &'static Link, typed: &Entity<TextField>, a: Option<String>, cx: &mut olo_ui::gpui::App) {
    if let Some(q) = link.shared.lock().unwrap().question.take() {
        q.answer(a);
    }
    typed.update(cx, |f, cx| f.set_text("", cx));
}

pub struct State {
    list: Entity<ListModel>,
    /// The selected device's path (the list reorders).
    selected: Option<String>,
    /// A search was started when the panel first showed.
    searched: bool,
    /// What's typed for a PIN or passkey.
    typed: Entity<TextField>,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let link = link(olo_ui::proxy(cx));
        let typed = cx.new(TextField::new);
        // Return answers with the code.
        cx.subscribe(&typed, move |_, field, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                let code = field.read(cx).text().to_owned();
                if !code.is_empty() {
                    answer(link, &field, Some(code), cx);
                }
            }
        })
        .detach();
        State { list: ListModel::new(cx, false), selected: None, searched: false, typed, link }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Bluetooth(st) => Some(st),
        _ => None,
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (status, doing, error) = {
        let sh = link.shared.lock().unwrap();
        (sh.status.clone(), sh.doing.clone(), sh.error.clone())
    };
    let status = match status {
        None => return ui.add(EmptyState::new("Looking for Bluetooth…").icon("bluetooth")),
        Some(Err(_)) => return ui.add(EmptyState::new("The Bluetooth service (bluetoothd) isn't running.").icon("bluetooth")),
        Some(Ok(s)) => s,
    };
    let Some(adapter) = status.adapter.clone() else {
        ui.add(EmptyState::new("There's no Bluetooth adapter.").icon("bluetooth"));
        return;
    };
    if adapter.powered && !st.searched {
        st.searched = true;
        let _ = link.tx.send(Request::Search(adapter.path.clone()));
    }

    let text = if adapter.powered { format!("This computer is “{}”.", adapter.name) } else { "Off".to_owned() };
    let path = adapter.path.clone();
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING * 3.0))
            .child(CheckBox::new("Bluetooth", adapter.powered).on_toggle(move |on, _, _| {
                let _ = link.tx.send(Request::Power(path.clone(), on));
            }))
            .child(StringView::new(text).dim(true)),
    );
    if !adapter.powered {
        return;
    }

    question(ui, st);

    let devs: Vec<bt::Device> = status.devices.iter().filter(|d| d.named || d.paired).cloned().collect();
    // AirPods: their model as the kind, and each bud's level.
    let (pods, _) = crate::airpods::current(ui.cx);
    let pods_of = |d: &bt::Device| d.modalias.as_deref().is_some_and(olo_system::airpods::is_airpods).then(|| pods.by_address(&d.address).cloned()).flatten();
    let pods_rows: Vec<Option<olo_protocol::airpods::AirPods>> = devs.iter().map(pods_of).collect();
    let at = st.selected.as_ref().and_then(|s| devs.iter().position(|d| d.path == *s));
    st.list.update(ui.cx, |l, _| {
        l.state.selected.clear();
        if let Some(i) = at {
            l.state.select(i);
        }
    });
    let columns = vec![Column::new("Device", 200.0), Column::new("Type", 110.0), Column::new("Status", 90.0)];
    let (cells, doing2) = (devs.clone(), doing.clone());
    let paths: Vec<String> = devs.iter().map(|d| d.path.clone()).collect();
    let model = st.list.clone();
    let pick = ui.on_view(move |v, _: (), _, cx| {
        let row = model.read(cx).state.single();
        if let Some(st) = state_of(v) {
            st.selected = row.and_then(|i| paths.get(i).cloned());
        }
        cx.notify();
    });
    let picked = at.and_then(|i| devs.get(i)).cloned();
    // A double click (or Return) pairs or connects, as the button would.
    let open = {
        let picked = picked.clone();
        move |_: usize, _: &mut olo_ui::gpui::Window, _: &mut olo_ui::gpui::App| {
            if let Some(d) = &picked
                && !doing2.contains_key(&d.path)
                && !d.connected
            {
                if d.paired {
                    act(link, d.path.clone(), "Connecting…", "The device didn't connect.", bt::connect);
                } else {
                    act(link, d.path.clone(), "Pairing…", "Pairing didn't work.", bt::pair);
                }
            }
        }
    };
    let doing3 = doing.clone();
    let rows = pods_rows.clone();
    let list = ColumnListView::new("bt-devices", &st.list, columns, devs.len(), move |cell, window, cx| {
        let d = &cells[cell.row];
        let pod = rows[cell.row].as_ref();
        match cell.column {
            0 => cell.text(window, cx, d.name.clone()),
            1 => cell.text(window, cx, pod.map_or(d.kind().to_owned(), |p| p.model.split(" (").next().unwrap_or("AirPods").to_owned())),
            _ => {
                let text = match (doing3.get(&d.path), d.connected, d.paired) {
                    (Some(doing), ..) => (*doing).to_owned(),
                    (None, true, _) if pod.and_then(|p| p.lowest()).is_some() => format!("Connected, {}%", pod.and_then(|p| p.lowest()).unwrap_or(0)),
                    (None, true, _) => d.battery.map_or("Connected".to_owned(), |b| format!("Connected, {b}%")),
                    (None, false, true) => "Paired".to_owned(),
                    _ => String::new(),
                };
                cell.text(window, cx, text);
            }
        }
    })
    .on_select(move |window, cx| pick((), window, cx))
    .on_open(open);
    ui.add(div().flex_1().min_h(px(90.0)).child(list));

    let label = if adapter.discovering { "Searching…" } else { "Search" };
    let path = adapter.path.clone();
    let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(Button::new(label).enabled(!adapter.discovering).on_click(move |_, _| {
        let _ = link.tx.send(Request::Search(path.clone()));
    }));
    match picked {
        None => {
            if devs.is_empty() {
                row = row.child(StringView::new("Put a device in pairing mode to see it here.").dim(true));
            }
        }
        Some(d) => {
            let busy = doing.contains_key(&d.path);
            let p = d.path.clone();
            row = if !d.paired {
                row.child(Button::new("Pair").default(true).enabled(!busy).on_click(move |_, _| act(link, p.clone(), "Pairing…", "Pairing didn't work.", bt::pair)))
            } else if d.connected {
                row.child(Button::new("Disconnect").enabled(!busy).on_click(move |_, _| act(link, p.clone(), "Disconnecting…", "The device couldn't be disconnected.", bt::disconnect)))
            } else {
                row.child(Button::new("Connect").default(true).enabled(!busy).on_click(move |_, _| act(link, p.clone(), "Connecting…", "The device didn't connect.", bt::connect)))
            };
            if d.paired {
                let (adapter, p) = (adapter.path.clone(), d.path.clone());
                row = row.child(Button::new("Remove").enabled(!busy).on_click(move |_, _| {
                    let adapter = adapter.clone();
                    act(link, p.clone(), "Removing…", "The device couldn't be removed.", move |dev| bt::remove(&adapter, dev));
                }));
            }
            if d.paired && d.modalias.as_deref().is_some_and(olo_system::airpods::is_airpods) {
                let prefs = ui.prefs();
                row = row.child(Button::new("AirPods Settings…").on_click(move |_, cx| {
                    let _ = prefs.update(cx, |p, cx| p.open("airpods", cx));
                }));
            }
            if busy && doing.get(&d.path) == Some(&"Pairing…") {
                let p = d.path.clone();
                row = row.child(Button::new("Cancel").on_click(move |_, _| cancel_pairing(p.clone())));
            }
        }
    }
    ui.add(row);
    if let Some(e) = error {
        ui.label(e);
    }
}

/// BlueZ's question for a pairing in progress, with its answers.
fn question(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (q_text, typed, only_ok, path) = {
        let sh = link.shared.lock().unwrap();
        let Some(q) = &sh.question else { return };
        let (text, typed) = match &q.ask {
            Ask::Confirm(n) => (format!("Does “{}” show {n:06}?", q.device), false),
            Ask::ShowPasskey(n) => (format!("Type {n:06} on “{}”, then press Return.", q.device), false),
            Ask::ShowPin(p) => (format!("Type {p} on “{}”, then press Return.", q.device), false),
            Ask::EnterPin => (format!("Type the PIN for “{}” (often 0000).", q.device), true),
            Ask::EnterPasskey => (format!("Type the passkey shown by “{}”.", q.device), true),
            Ask::Authorize => (format!("“{}” wants to pair with this computer.", q.device), false),
        };
        (text, typed, matches!(q.ask, Ask::ShowPasskey(_) | Ask::ShowPin(_)), q.path.clone())
    };
    let field = st.typed.clone();
    let code = field.read(ui.cx).text().to_owned();
    ui.bbox("Pairing", |ui| {
        ui.label(q_text);
        if typed {
            ui.add(TextControl::new(field.clone()).label("Code:").divider(72.0));
        }
        let mut row = div().flex().flex_row().gap(px(m::SPACING));
        if only_ok {
            let f = field.clone();
            row = row.child(Button::new("Cancel").on_click(move |_, cx| {
                cancel_pairing(path.clone());
                answer(link, &f, None, cx);
            }));
        } else {
            let (no, yes) = (field.clone(), field.clone());
            row = row.child(Button::new(if typed { "Cancel" } else { "Reject" }).on_click(move |_, cx| answer(link, &no, None, cx))).child(
                Button::new(if typed { "Pair" } else { "Accept" }).default(true).enabled(!typed || !code.is_empty()).on_click(move |_, cx| {
                    let code = yes.read(cx).text().to_owned();
                    answer(link, &yes, Some(code), cx);
                }),
            );
        }
        ui.add(row);
    });
}
