//! Bluetooth: turn it on or off, look for devices, pair, connect and
//! forget them. Talks to BlueZ natively (`olo_system::bluetooth`): one
//! thread reads its state every second and a thread per action does the
//! slow parts (pairing can wait on the user typing a passkey), so the
//! window never waits. A pairing agent registered for this process puts
//! BlueZ's questions (passkeys, PINs) in the panel.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{Context, Key, Rect, Ui, UiBuilder, vec2},
    metrics as m,
    widgets::{BBox, Button, CheckBox, Column, ColumnListView, ListState, StringView, TextControl},
};
use olo_system::bluetooth::{self as bt, Agent, AgentEvent, Ask, Question, Status};

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
    contexts: Arc<Mutex<Vec<Context>>>,
}

fn wake(contexts: &Mutex<Vec<Context>>) {
    contexts.lock().unwrap().iter().for_each(|c| c.request_repaint());
}

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        let contexts: Arc<Mutex<Vec<Context>>> = Default::default();
        let (tx, rx) = channel::<Request>();
        let (sh, ctxs) = (shared.clone(), contexts.clone());
        std::thread::Builder::new()
            .name("bluetooth".into())
            .spawn(move || {
                let agent = {
                    let (sh, ctxs) = (sh.clone(), ctxs.clone());
                    Agent::register(move |e| {
                        sh.lock().unwrap().question = match e {
                            AgentEvent::Ask(q) => Some(q),
                            AgentEvent::Cancel => None,
                        };
                        wake(&ctxs);
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
                        wake(&ctxs);
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
                            r.or_else(|e| if e.to_string().contains("InProgress") { Ok(()) } else { Err(e) })
                                .map_err(|e| (e, "Couldn't search for devices."))
                        }
                    };
                    sh.lock().unwrap().error = r.err().map(|(e, what)| {
                        tracing::warn!("bluez: {e}");
                        what.to_owned()
                    });
                    wake(&ctxs);
                }
                drop(agent);
            })
            .expect("thread");
        Link { shared, tx, contexts }
    })
}

/// Do something slow to a device on its own thread.
fn act(device: String, doing: &'static str, failed: &'static str, f: impl FnOnce(&str) -> olo_system::Result<()> + Send + 'static) {
    let link = link();
    link.shared.lock().unwrap().doing.insert(device.clone(), doing);
    let (sh, ctxs) = (link.shared.clone(), link.contexts.clone());
    std::thread::spawn(move || {
        let r = f(&device);
        let mut s = sh.lock().unwrap();
        s.doing.remove(&device);
        if s.question.as_ref().is_some_and(|q| q.path == device) {
            s.question = None;
        }
        s.error = r.err().map(|e| {
            tracing::warn!("bluez: {e}");
            failed.to_owned()
        });
        drop(s);
        wake(&ctxs);
    });
}

fn cancel_pairing(device: String) {
    std::thread::spawn(move || {
        let _ = bt::cancel_pairing(&device);
    });
}

#[derive(Default)]
pub struct State {
    list: ListState,
    /// The selected device's path (the list reorders).
    selected: Option<String>,
    /// A search was started when the panel first showed.
    searched: bool,
    /// What's typed for a PIN or passkey.
    typed: String,
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = link();
    {
        let mut ctxs = link.contexts.lock().unwrap();
        if !ctxs.iter().any(|c| c == ui.ctx()) {
            ctxs.push(ui.ctx().clone());
        }
    }
    let (status, doing, error) = {
        let sh = link.shared.lock().unwrap();
        (sh.status.clone(), sh.doing.clone(), sh.error.clone())
    };
    let status = match status {
        None => {
            ui.add(StringView::new("Looking for Bluetooth…").dim(true));
            return;
        }
        Some(Err(_)) => {
            ui.add(StringView::new("The Bluetooth service (bluetoothd) isn't running.").dim(true));
            return;
        }
        Some(Ok(s)) => s,
    };
    let Some(adapter) = &status.adapter else {
        ui.add(StringView::new("There's no Bluetooth adapter.").dim(true));
        return;
    };
    if adapter.powered && !st.searched {
        st.searched = true;
        let _ = link.tx.send(Request::Search(adapter.path.clone()));
    }

    let mut on = adapter.powered;
    ui.horizontal(|ui| {
        ui.add(CheckBox::new(&mut on, "Bluetooth"));
        ui.add_space(m::SPACING * 2.0);
        let text = if adapter.powered { format!("This computer is “{}”.", adapter.name) } else { "Off".to_owned() };
        ui.add(StringView::new(&text).dim(true));
    });
    if on != adapter.powered {
        let _ = link.tx.send(Request::Power(adapter.path.clone(), on));
    }
    if !adapter.powered {
        return;
    }

    question(ui, st);

    let devs: Vec<_> = status.devices.iter().filter(|d| d.named || d.paired).collect();
    let devs = &devs;
    st.list.selected.clear();
    if let Some(i) = st.selected.as_ref().and_then(|s| devs.iter().position(|d| d.path == *s)) {
        st.list.select(i);
    }
    let columns = [Column::new("Device", 200.0), Column::new("Type", 110.0), Column::new("Status", 90.0)];
    let list = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), (ui.available_height() - 2.0 * (m::CONTROL_HEIGHT + m::SPACING)).max(90.0)));
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(list), |ui| {
            ColumnListView::new("bt-devices", &columns, devs.len(), &mut st.list).show(ui, |cell| {
                let d = &devs[cell.row];
                match cell.column {
                    0 => cell.text(&d.name),
                    1 => cell.text(d.kind()),
                    _ => {
                        let text = match (doing.get(&d.path), d.connected, d.paired) {
                            (Some(doing), ..) => (*doing).to_owned(),
                            (None, true, _) => d.battery.map_or("Connected".to_owned(), |b| format!("Connected, {b}%")),
                            (None, false, true) => "Paired".to_owned(),
                            _ => String::new(),
                        };
                        cell.text(&text);
                    }
                }
            })
        })
        .inner;
    ui.advance_cursor_after_rect(list);
    let picked = st.list.single().and_then(|i| devs.get(i)).map(|d| (*d).clone());
    if resp.selection_changed {
        st.selected = picked.as_ref().map(|d| d.path.clone());
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING;
        let label = if adapter.discovering { "Searching…" } else { "Search" };
        if ui.add_enabled_ui(!adapter.discovering, |ui| ui.add(Button::new(label))).inner.clicked() {
            let _ = link.tx.send(Request::Search(adapter.path.clone()));
        }
        let Some(d) = picked else {
            if devs.is_empty() {
                ui.add(StringView::new("Put a device in pairing mode to see it here.").dim(true));
            }
            return;
        };
        let busy = doing.contains_key(&d.path);
        ui.add_enabled_ui(!busy, |ui| {
            if !d.paired {
                if ui.add(Button::new("Pair").default(true)).clicked() || resp.activated.is_some() {
                    act(d.path.clone(), "Pairing…", "Pairing didn't work.", bt::pair);
                }
            } else if d.connected {
                if ui.add(Button::new("Disconnect")).clicked() {
                    act(d.path.clone(), "Disconnecting…", "The device couldn't be disconnected.", bt::disconnect);
                }
            } else if ui.add(Button::new("Connect").default(true)).clicked() || resp.activated.is_some() {
                act(d.path.clone(), "Connecting…", "The device didn't connect.", bt::connect);
            }
            if d.paired && ui.add(Button::new("Remove")).clicked() {
                let adapter = adapter.path.clone();
                act(d.path.clone(), "Removing…", "The device couldn't be removed.", move |dev| bt::remove(&adapter, dev));
            }
        });
        if busy && doing.get(&d.path) == Some(&"Pairing…") && ui.add(Button::new("Cancel")).clicked() {
            cancel_pairing(d.path.clone());
        }
    });
    if let Some(e) = &error {
        ui.add(StringView::new(e));
    }
}

/// BlueZ's question for a pairing in progress, with its answers.
fn question(ui: &mut Ui, st: &mut State) {
    let mut sh = link().shared.lock().unwrap();
    let Some(q) = &sh.question else { return };
    let (text, typed) = match &q.ask {
        Ask::Confirm(n) => (format!("Does “{}” show {n:06}?", q.device), false),
        Ask::ShowPasskey(n) => (format!("Type {n:06} on “{}”, then press Return.", q.device), false),
        Ask::ShowPin(p) => (format!("Type {p} on “{}”, then press Return.", q.device), false),
        Ask::EnterPin => (format!("Type the PIN for “{}” (often 0000).", q.device), true),
        Ask::EnterPasskey => (format!("Type the passkey shown by “{}”.", q.device), true),
        Ask::Authorize => (format!("“{}” wants to pair with this computer.", q.device), false),
    };
    let only_ok = matches!(q.ask, Ask::ShowPasskey(_) | Ask::ShowPin(_));
    let mut answer: Option<Option<String>> = None;
    BBox::new("Pairing").show(ui, |ui| {
        ui.add(StringView::new(&text));
        if typed {
            let r = ui.add(TextControl::new(&mut st.typed).label("Code:").divider(72.0).autofocus(true));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !st.typed.is_empty() {
                answer = Some(Some(std::mem::take(&mut st.typed)));
            }
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            if only_ok {
                if ui.add(Button::new("Cancel")).clicked() {
                    cancel_pairing(q.path.clone());
                    answer = Some(None);
                }
                return;
            }
            if ui.add(Button::new(if typed { "Cancel" } else { "Reject" })).clicked() {
                answer = Some(None);
            }
            let label = if typed { "Pair" } else { "Accept" };
            if ui.add_enabled_ui(!typed || !st.typed.is_empty(), |ui| ui.add(Button::new(label).default(true))).inner.clicked() {
                answer = Some(Some(std::mem::take(&mut st.typed)));
            }
        });
    });
    if let Some(a) = answer
        && let Some(q) = sh.question.take()
    {
        q.answer(a);
        st.typed.clear();
    }
}
