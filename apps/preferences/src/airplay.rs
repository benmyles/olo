//! AirPlay: the receivers on the network (an Apple TV, a Mac, a
//! television with AirPlay built in), pairing with one (the code it shows
//! on its screen, typed here), checking and forgetting a pairing, and
//! showing this computer's display on one (Ben, 2026-11-12: "proper gui
//! integration with a preferences panel … cli alone is not enough").
//!
//! The network is asked (mDNS, `olo_airplay::receivers`) on a thread of
//! its own when the panel first shows and when Search is pressed, never
//! on a timer; pairing and checking each wait on the receiver on a thread
//! of their own, so the window never does. The pairings file and the
//! plays' folder are followed (inotify): a pairing made by `olo-airplay
//! pair`, or a showing started from a window's menu, shows here at once.
//! Windows are shown from their tab's menu and Deskbar's rows (Show on
//! AirPlay), films from Viewer and Tracker (Play On).

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use olo_airplay::{Kind, Pairing, Receiver, kept};
use olo_kit::{
    Child, Cx, ParentElement as _, Styled as _, ViewRef, WindowCx, div, metrics as m, px,
    widgets::{Button, Column, ColumnListView, EmptyState, ListModel, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_protocol::airplay::{self as ap, Paired, Playing, Shown};
use olo_sync::Mutex;

use crate::{Msg, PanelView, form::Ui, panel};

/// How long the network is listened to for receivers.
const LOOK: Duration = Duration::from_secs(2);
/// How long a receiver is waited for at each step of pairing.
const WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Shared {
    /// The receivers that answered (None until the network was first
    /// asked), and whether it's being asked now.
    found: Option<Vec<Receiver>>,
    searching: bool,
    /// Receivers paired with, and what plays or shows on them.
    paired: Vec<Paired>,
    plays: Vec<Playing>,
    /// What's being done to which receiver, by its key ("Pairing…").
    doing: HashMap<String, &'static str>,
    /// A receiver showing its code, waiting for it typed here (its key
    /// and name).
    asking: Option<(String, String)>,
    /// What last happened, said (true: it went wrong).
    said: Option<(bool, String)>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    /// The pairing begun (the receiver shows its code while it's held).
    pairing: Arc<Mutex<Option<Pairing>>>,
    proxy: olo_kit::Proxy,
}

/// The key a receiver is known by here (its id, or its name).
fn key_of(r: &Receiver) -> String {
    if r.device_id.is_empty() { r.name.clone() } else { r.device_id.clone() }
}

fn kind_said(r: &Receiver) -> &'static str {
    match r.kind {
        Kind::AppleTv => "Apple TV",
        Kind::Mac => "Mac",
        Kind::Speaker => "Speaker",
        Kind::Television => "Television",
    }
}

/// Test data (the layout lint): receivers named as long as real ones,
/// one paired and showing, one paired and away; never the network.
fn test_shared() -> Shared {
    let at = |port: u16| std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let front = Receiver::of_a_test("Front Room", at(7000));
    let tv = Receiver { kind: Kind::Television, model: "OLED65C3PUA".into(), ..Receiver::of_a_test("Living Room LG OLED65C3PUA Television", at(7001)) };
    let paired = vec![Paired { device_id: front.device_id.clone(), name: front.name.clone() }, Paired { device_id: "02:00:00:00:AA:01".into(), name: "Studio Mac mini (M4 Pro, 2026)".into() }];
    let plays = vec![Playing {
        pid: 1,
        receiver: front.name.clone(),
        title: "The screen".into(),
        path: None,
        state: "showing".into(),
        position: None,
        duration: None,
        rate: 1.0,
        as_of: 0,
        started: 0,
        live: true,
    }];
    Shared { found: Some(vec![front, tv]), paired, plays, ..Default::default() }
}

/// The panel's link to the network and the files (one for the app),
/// telling the windows through `proxy` when anything changes.
fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        if cfg!(test) {
            return Link { shared: Arc::new(Mutex::new(test_shared())), pairing: Arc::default(), proxy };
        }
        let shared: Arc<Mutex<Shared>> = Arc::new(Mutex::new(Shared { paired: ap::paired(), plays: ap::all(), ..Default::default() }));
        let (sh, out) = (shared.clone(), proxy.clone());
        let (pairings, plays) = (ap::pairings_path(), ap::dir());
        let watch = olo_kit::watch::watch([pairings.clone(), plays.clone()], move |changed| {
            let all = changed.is_empty();
            let mut s = sh.lock().unwrap();
            if all || changed.contains(&pairings) {
                s.paired = ap::paired();
            }
            if all || changed.iter().any(|p| p.starts_with(&plays)) {
                s.plays = ap::all();
            }
            drop(s);
            out.send(Msg::Redraw);
        });
        // (The app's for as long as it runs, as the link is.)
        std::mem::forget(watch);
        Link { shared, pairing: Arc::default(), proxy }
    })
}

/// Ask the network who's there, on a thread of its own.
fn search(link: &'static Link) {
    {
        let mut s = link.shared.lock().unwrap();
        if s.searching {
            return;
        }
        s.searching = true;
    }
    link.proxy.send(Msg::Redraw);
    let _ = std::thread::Builder::new().name("airplay-search".into()).spawn(move || {
        let found = olo_airplay::receivers(LOOK);
        let mut s = link.shared.lock().unwrap();
        s.searching = false;
        match found {
            Ok(found) => s.found = Some(found),
            Err(e) => {
                tracing::warn!("airplay: the network couldn't be asked: {e}");
                s.found.get_or_insert_with(Vec::new);
                s.said = Some((true, "The network couldn't be asked for receivers.".into()));
            }
        }
        drop(s);
        link.proxy.send(Msg::Redraw);
    });
}

/// Something slow done to a receiver on its own thread, said as it goes
/// (`doing`) and when it's done (an error, or `done`'s words).
fn act(link: &'static Link, key: String, doing: &'static str, f: impl FnOnce() -> Result<Option<String>, String> + Send + 'static) {
    link.shared.lock().unwrap().doing.insert(key.clone(), doing);
    link.proxy.send(Msg::Redraw);
    let _ = std::thread::Builder::new().name("airplay-act".into()).spawn(move || {
        let r = f();
        let mut s = link.shared.lock().unwrap();
        s.doing.remove(&key);
        match r {
            Ok(Some(said)) => s.said = Some((false, said)),
            Ok(None) => {}
            Err(e) => s.said = Some((true, e)),
        }
        drop(s);
        link.proxy.send(Msg::Redraw);
    });
}

/// Ask the receiver to show its code (it waits for it typed here).
fn begin_pairing(link: &'static Link, r: Receiver) {
    let Some(at) = r.at() else { return };
    let key = key_of(&r);
    link.shared.lock().unwrap().said = None;
    act(link, key.clone(), "Asking…", move || {
        let p = Pairing::begin(at, &r.device_id, &r.name, WAIT).map_err(|e| format!("{} didn't start pairing: {e}.", r.name))?;
        *link.pairing.lock().unwrap() = Some(p);
        link.shared.lock().unwrap().asking = Some((key, r.name.clone()));
        Ok(None)
    });
}

/// The code typed: paired (and kept), or why not.
fn finish_pairing(link: &'static Link, code: String) {
    let Some((key, name)) = link.shared.lock().unwrap().asking.take() else { return };
    let Some(p) = link.pairing.lock().unwrap().take() else { return };
    act(link, key, "Pairing…", move || match p.finish(code.trim()) {
        Ok(_) => Ok(Some(format!("Paired with {name}."))),
        Err(e) => Err(format!("Pairing with {name} didn't work: {e}.")),
    });
}

/// Stop pairing: the receiver takes its code down.
fn cancel_pairing(link: &'static Link) {
    link.pairing.lock().unwrap().take();
    link.shared.lock().unwrap().asking = None;
    link.proxy.send(Msg::Redraw);
}

pub struct State {
    list: ListModel,
    /// The selected receiver's key (the list reorders).
    selected: Option<String>,
    /// The network was asked when the panel first showed.
    searched: bool,
    /// The code the receiver shows, typed.
    code: Child<TextField>,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let link = link(olo_kit::proxy());
        let code = crate::form::text_field(cx, |f| f);
        // Return pairs with the code.
        cx.subscribe(&code, move |v: &mut PanelView, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered)
                && let panel::State::AirPlay(st) = &v.state
            {
                let typed = st.code.read(cx).text().to_owned();
                if !typed.trim().is_empty() {
                    clear(st.code.downgrade(), cx);
                    finish_pairing(link, typed);
                }
            }
        });
        State { list: ListModel::new(false), selected: None, searched: false, code, link }
    }
}

fn clear(field: ViewRef<TextField>, cx: &mut WindowCx) {
    field.update(cx, |f, cx| f.set_text("", cx));
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::AirPlay(st) => Some(st),
        _ => None,
    }
}

/// A row of the list: a receiver that answered, or one paired with that
/// didn't (it's off, or on another network).
#[derive(Clone)]
struct Row {
    key: String,
    name: String,
    found: Option<Receiver>,
    paired: bool,
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    if !st.searched {
        st.searched = true;
        if !cfg!(test) {
            search(link);
        }
    }
    let (found, searching, paired, plays, doing, asking, said) = {
        let s = link.shared.lock().unwrap();
        (s.found.clone(), s.searching, s.paired.clone(), s.plays.clone(), s.doing.clone(), s.asking.clone(), s.said.clone())
    };
    ui.add(StringView::new(format!("This computer is “{}” to AirPlay receivers.", olo_airplay::our_name())).dim(true));

    // Those that answered, then those paired with that didn't.
    let is_paired = |key: &str, name: &str| paired.iter().any(|p| (!p.device_id.is_empty() && p.device_id.eq_ignore_ascii_case(key)) || (p.device_id.is_empty() && p.name == name));
    let mut rows: Vec<Row> = found.iter().flatten().map(|r| Row { key: key_of(r), name: r.name.clone(), paired: is_paired(&key_of(r), &r.name), found: Some(r.clone()) }).collect();
    for p in &paired {
        if !rows.iter().any(|r| r.key.eq_ignore_ascii_case(p.key())) {
            rows.push(Row { key: p.key().to_owned(), name: p.name.clone(), found: None, paired: true });
        }
    }
    if rows.is_empty() {
        let said = if found.is_none() || searching { "Looking for AirPlay receivers…" } else { "No AirPlay receivers answered on this network." };
        ui.add(div().flex_1().min_h(px(120.0)).flex().child(EmptyState::new(said).icon("airplay")));
    } else {
        let at = st.selected.as_ref().and_then(|s| rows.iter().position(|r| r.key == *s));
        {
            let mut l = st.list.state_mut();
            l.selected.clear();
            if let Some(i) = at {
                l.select(i);
            }
        }
        let columns = vec![Column::new("Receiver", 220.0), Column::new("Type", 100.0), Column::new("Status", 120.0)];
        let keys: Vec<String> = rows.iter().map(|r| r.key.clone()).collect();
        let model = st.list.clone();
        let pick = ui.on_view(move |v, _: &(), cx| {
            let row = model.state().single();
            if let Some(st) = state_of(v) {
                st.selected = row.and_then(|i| keys.get(i).cloned());
            }
            cx.notify();
        });
        // A double click (or Return) pairs with one not paired with (the
        // code's field takes the keyboard as its box shows).
        let open = {
            let (rows, doing, field) = (rows.clone(), doing.clone(), st.code.downgrade());
            move |row: &usize, cx: &mut WindowCx| {
                if let Some(Row { found: Some(r), paired: false, key, .. }) = rows.get(*row)
                    && !doing.contains_key(key)
                {
                    field.focus(cx);
                    begin_pairing(link, r.clone());
                }
            }
        };
        let (cells, doing2, plays2, asked) = (rows.clone(), doing.clone(), plays.clone(), asking.as_ref().map(|a| a.0.clone()));
        let list = ColumnListView::new(ui.cx, "airplay-receivers", &st.list, columns, rows.len(), move |_: &PanelView, cell, g| {
            let r = &cells[cell.row];
            match cell.column {
                0 => cell.text(g, r.name.clone()),
                1 => cell.text(g, r.found.as_ref().map_or("", kind_said).to_owned()),
                _ => {
                    let on: Vec<&Playing> = plays2.iter().filter(|p| p.receiver == r.name).collect();
                    let text = match (doing2.get(&r.key), on.first(), &r.found, r.paired) {
                        (Some(doing), ..) => (*doing).to_owned(),
                        _ if asked.as_deref() == Some(r.key.as_str()) => "Showing its code".to_owned(),
                        (None, Some(p), ..) if p.paused() => "Paused".to_owned(),
                        (None, Some(p), ..) if p.live => "Showing".to_owned(),
                        (None, Some(_), ..) => "Playing".to_owned(),
                        (None, None, None, _) => "Not found".to_owned(),
                        (None, None, Some(_), true) => "Paired".to_owned(),
                        (None, None, Some(_), false) => "Not paired".to_owned(),
                    };
                    cell.text(g, text);
                }
            }
        })
        .on_select(move |_window, cx| pick(&(), cx))
        .on_open(open);
        ui.add(div().flex_1().min_h(px(120.0)).child(list));
    }

    // The receiver showing its code: the code, typed.
    if let Some((_, name)) = &asking {
        let typed = st.code.read(ui.cx).text().trim().to_owned();
        let field = st.code.downgrade();
        let code_field = &st.code;
        ui.bbox("Pairing", |ui| {
            ui.label(format!("Type the code “{name}” shows on its screen."));
            ui.add(TextControl::new(ui.cx, code_field).label("Code:").divider(72.0));
            let pair = ui.on_click_view(move |v, cx| {
                if let Some(st) = state_of(v) {
                    let typed = st.code.read(cx).text().to_owned();
                    clear(st.code.downgrade(), cx);
                    finish_pairing(link, typed);
                }
            });
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").on_click(move |_, cx| {
                        clear(field, cx);
                        cancel_pairing(link);
                    }))
                    .child(Button::new("Pair").default(true).enabled(!typed.is_empty()).on_click(pair)),
            );
        });
    }

    // What can be done with the one chosen.
    let picked = st.selected.as_ref().and_then(|k| rows.iter().find(|r| r.key == *k)).cloned();
    let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(Button::new(if searching { "Searching…" } else { "Search" }).enabled(!searching).on_click(move |_, _| search(link)));
    if let Some(r) = picked {
        let busy = doing.contains_key(&r.key) || asking.is_some();
        let on: Vec<Playing> = plays.iter().filter(|p| p.receiver == r.name).cloned().collect();
        match (&r.found, r.paired) {
            // (While one shows its code, its box has the buttons.)
            (Some(found), false) if asking.is_none() => {
                let (found, field) = (found.clone(), st.code.downgrade());
                row = row.child(Button::new("Pair…").default(true).enabled(!busy).on_click(move |_, cx| {
                    field.focus(cx);
                    begin_pairing(link, found.clone());
                }));
            }
            (found, true) => {
                if !on.is_empty() {
                    row = row.child(Button::new("Stop").on_click(move |_, _| {
                        for p in &on {
                            let _ = olo_xdg::spawn_detached(&format!("olo-airplay stop --pid {}", p.pid));
                        }
                    }));
                } else if let Some(f) = found.as_ref().filter(|f| f.shows_screens) {
                    let key = key_of(f);
                    row = row.child(Button::new("Show Display").default(true).enabled(!busy).on_click(move |_, _| {
                        if let Err(e) = ap::show_on(&key, &Shown::Display { name: None, title: String::new() }) {
                            tracing::warn!("airplay: couldn't show the display: {e}");
                        }
                    }));
                }
                if let Some(f) = found.clone() {
                    let key = r.key.clone();
                    row = row.child(Button::new("Check").enabled(!busy).on_click(move |_, _| {
                        let f = f.clone();
                        link.shared.lock().unwrap().said = None;
                        act(link, key.clone(), "Checking…", move || {
                            let at = f.at().ok_or_else(|| format!("{} didn't say where it is.", f.name))?;
                            let k = kept::of(&f.device_id, &f.name).ok_or_else(|| format!("{} isn't paired with.", f.name))?;
                            olo_airplay::check(at, &k, WAIT).map_err(|e| format!("{}: {e}.", f.name))?;
                            Ok(Some(format!("{} knows this computer.", f.name)))
                        });
                    }));
                }
                let (key, name) = (r.key.clone(), r.name.clone());
                row = row.child(Button::new("Forget").enabled(!busy).on_click(move |_, _| {
                    let (key, name) = (key.clone(), name.clone());
                    act(link, key.clone(), "Forgetting…", move || {
                        kept::forget(&key).map_err(|e| format!("The pairing with {name} couldn't be forgotten: {e}."))?;
                        Ok(Some(format!("Forgot {name}: pair with it again to use it.")))
                    });
                }));
            }
            _ => {}
        }
    }
    ui.add(row);
    if let Some((_, said)) = said {
        ui.label(said);
    }
    ui.note("Show a window from its tab's menu (right-click its title) or its row in Deskbar: Show on AirPlay. Play a film from Viewer or Tracker: Play On.");
}
