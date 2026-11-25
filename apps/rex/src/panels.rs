//! Rex's windows besides the workspace:
//! - Hosts: the hosts Rex knows, how each stands, and the ports
//!   forwarded from them;
//! - Remote Access: this computer's own Rex server reached over
//!   Tailscale, and a device paired with it by a code to scan;
//! - Diagnostics: the facts of the server and of this app, to copy;
//! - the question an SSH connection asks (a password, a host key to
//!   trust).
//!
//! Rex has no settings window of its own. What it does for whom is
//! Preferences ▸ Rex (the desktop's settings: `shared`); its keys are
//! Preferences ▸ Keyboard; a window's quick choices are its Settings
//! menu. (It came with a window of eight sections, most of whose
//! controls had nothing behind them.)

use std::sync::Arc;

use olo_kit::{
    Bounds, Child, Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, WindowCx, canvas, div, icons, metrics as m, point, px, size, style, theme,
    widgets::{BBox, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, StringView, TextControl, TextField, divider_for},
};
use olo_rex_client::{AgentStatus, SshAskpass};
use olo_sync::{Condvar, Mutex};
use zeroize::Zeroizing;

use crate::hub::{HostConnState, RexHub};

// (The status light of every window of Rex's is the kit's.)
pub use olo_kit::widgets::{LIGHT, Light, busy_of, light, light_of, ring_of};

/// How a host's connection stands, as a light. (One it was never asked
/// to connect to, or was disconnected from, is off, not down: red is
/// for what went wrong.)
pub fn light_for(conn: &HostConnState) -> Light {
    match conn {
        HostConnState::Connected => Light::On,
        HostConnState::Connecting | HostConnState::Reconnecting { .. } => Light::Waiting,
        HostConnState::Error(_) => Light::Down,
        HostConnState::Disconnected => Light::Off,
    }
}

/// A host in the Hosts window's list: its name and address, how it
/// stands (its light, and the same in words), and whether it is this
/// computer's own server (which isn't removed).
#[derive(Clone, Debug, PartialEq)]
pub struct HostRow {
    pub key: String,
    pub name: String,
    /// Its address; "Local" for this computer's own server; empty where
    /// its name *is* its address (a host given no name), not said twice.
    pub address: String,
    pub light: Light,
    pub state: String,
    /// What went wrong, where something did: said whole under the list
    /// for the host chosen (its row says "Failed").
    pub trouble: Option<String>,
    pub local: bool,
    /// Whether Connect has something to do: it isn't connected, nor on
    /// its way.
    pub connects: bool,
    /// Whether the same button is Disconnect: for a host connected, or
    /// on its way. (This computer's own server too, where it is grey:
    /// connected, and that isn't to be changed. A grey Connect there
    /// said the opposite: Opus's review, 2026-11-23.)
    pub disconnects: bool,
    /// Whether its server can be looked after from here (the Server
    /// window): a host reached over SSH as an account.
    pub managed: bool,
    /// What the line under the buttons says of it when it is the one
    /// chosen ([`host_line`]).
    pub line: String,
}

/// What the line under the Hosts list's buttons says of the chosen
/// host, where nothing went wrong with it (then its trouble is said
/// there, after a red light): something in every state, so the line
/// whose room is always kept is never an empty band (Opus's review,
/// 2026-11-23: "as if a control is missing"), and what the greyed
/// buttons and a waiting port don't say for themselves is said.
///
/// One pattern in every state (its third review: five states read as
/// five voices): the row's own word for how it stands, then what that
/// means for its sessions and its ports, in short sentences. Not the
/// address again, which the row has.
pub fn host_line(conn: &HostConnState, local: bool, waits_for_agent: bool, sessions: usize, forwards: usize) -> String {
    use olo_kit::words::{count, tr};
    let sentences = |parts: &[&str]| parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let waiting = match forwards {
        0 => String::new(),
        n => count(n as u64, "1 forwarded port is waiting.", "{n} forwarded ports are waiting."),
    };
    match conn {
        _ if local => tr("Rex’s own server. It can’t be disconnected or removed.").into(),
        HostConnState::Connected => {
            let forwarded = match forwards {
                0 => String::new(),
                n => count(n as u64, "1 port forwarded here.", "{n} ports forwarded here."),
            };
            sentences(&[tr("Connected."), &count(sessions as u64, "1 session there.", "{n} sessions there."), &forwarded])
        }
        HostConnState::Connecting if waits_for_agent => tr("Connecting… Your key agent is asking you about this sign-in.").into(),
        HostConnState::Connecting => sentences(&[tr("Connecting…"), &waiting]),
        HostConnState::Reconnecting { last_error, .. } => sentences(&[&olo_kit::tr!("Reconnecting. {}", sentence(last_error)), &waiting]),
        HostConnState::Disconnected => sentences(&[tr("Disconnected."), tr("Sessions there keep running."), &waiting]),
        // (Said as its trouble.)
        HostConnState::Error(_) => String::new(),
    }
}

/// An error's words as a sentence: begun with a capital, ended with a
/// stop.
fn sentence(words: &str) -> String {
    let words = words.trim();
    let mut out: String = words.chars().take(1).flat_map(char::to_uppercase).chain(words.chars().skip(1)).collect();
    if !out.ends_with(['.', '!', '?', '…']) {
        out.push('.');
    }
    out
}

/// How a host's connection stands, in a word or two (its row's Status:
/// a state, not a message; an error's own words are the row's
/// `trouble`).
pub fn host_state(conn: &HostConnState, waits_for_agent: bool) -> String {
    match conn {
        HostConnState::Connected => olo_kit::words::tr("Connected").into(),
        HostConnState::Connecting if waits_for_agent => olo_kit::words::tr("Waiting for your key agent…").into(),
        HostConnState::Connecting => olo_kit::words::tr("Connecting…").into(),
        HostConnState::Disconnected => olo_kit::words::tr("Disconnected").into(),
        HostConnState::Reconnecting { attempt, .. } => olo_kit::tr!("Reconnecting (try {})…", attempt).to_string(),
        HostConnState::Error(_) => olo_kit::words::tr("Failed").into(),
    }
}

/// A host's address as its row says it: without `ssh://`, which every
/// one would begin with ("you@build.example" beside "nas.ts.net");
/// another scheme is kept.
pub fn address_said(endpoint: &str) -> &str {
    endpoint.strip_prefix("ssh://").unwrap_or(endpoint)
}

/// The hosts Rex knows, in their order, as the Hosts window lists them.
/// Whether Connect has something to do for a host standing so.
fn connects(conn: &HostConnState) -> bool {
    matches!(conn, HostConnState::Disconnected | HostConnState::Error(_) | HostConnState::Reconnecting { .. })
}

/// Whether the button is Disconnect: connected, or on its way (to stop
/// it). (One being connected to again after a failure has Connect,
/// which tries now.)
fn disconnects(conn: &HostConnState) -> bool {
    matches!(conn, HostConnState::Connected | HostConnState::Connecting)
}

/// How a forward stands, as its row says it. One that isn't up goes by
/// its host, in the host's own light: "Waiting for host" while the host
/// isn't connected (grey for one let go of, never connected or failed;
/// amber for one being connected to), "Reconnecting…" only where the
/// host's row says so too, or the host is back and the forward is being
/// made again. Never one word on a host and another on its port (Opus's
/// fourth review, 2026-11-23: a host Connecting…, its port
/// Reconnecting…, the line under them "is waiting").
fn forward_said(state: &crate::hub::ForwardState, host: Option<&HostConnState>) -> (Light, &'static str, Option<String>) {
    match (state, host) {
        (crate::hub::ForwardState::Active, _) => (Light::On, "Forwarded", None),
        (crate::hub::ForwardState::Failed(why), _) => (Light::Down, "Failed", Some(why.clone())),
        (crate::hub::ForwardState::Waiting, Some(HostConnState::Connecting)) => (Light::Waiting, "Waiting for host", None),
        (crate::hub::ForwardState::Waiting, Some(HostConnState::Disconnected | HostConnState::Error(_)) | None) => (Light::Off, "Waiting for host", None),
        (crate::hub::ForwardState::Waiting, Some(HostConnState::Reconnecting { .. } | HostConnState::Connected)) => (Light::Waiting, "Reconnecting…", None),
    }
}

pub fn host_rows(hub: &RexHub, waits_for_agent: &dyn Fn(&str) -> bool) -> Vec<HostRow> {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    st.host_order
        .iter()
        .filter_map(|k| st.hosts.get(k))
        .map(|h| {
            let named = !h.label.is_empty() && h.label != h.endpoint_raw;
            let forwards = st.forwards.iter().filter(|f| f.host_key == h.key).count() + demo_forwards().iter().filter(|(.., host, _, _)| *host == h.key).count();
            HostRow {
                line: host_line(&h.conn_state, h.is_local, waits_for_agent(&h.endpoint_raw), h.session_summaries.len().max(h.sessions.len()), forwards),
                key: h.key.clone(),
                name: if h.label.is_empty() { h.endpoint_raw.clone() } else { h.label.clone() },
                // (This computer's own server has no address a person
                // gave: "Local", not the path of a socket.)
                address: if h.is_local {
                    olo_kit::words::tr("Local").into()
                } else if named {
                    address_said(&h.endpoint_raw).to_owned()
                } else {
                    String::new()
                },
                light: light_for(&h.conn_state),
                state: host_state(&h.conn_state, waits_for_agent(&h.endpoint_raw)),
                trouble: match &h.conn_state {
                    HostConnState::Error(why) => Some(why.clone()),
                    _ => None,
                },
                local: h.is_local,
                connects: connects(&h.conn_state),
                disconnects: disconnects(&h.conn_state),
                managed: !h.is_local && matches!(crate::setup::address(&h.endpoint_raw), crate::setup::Address::Ssh(_)),
            }
        })
        .collect()
}

/// What a row of the Hosts window's Ports list is of.
#[derive(Clone, Debug, PartialEq)]
pub enum PortKind {
    /// A port a program in a pane listens on: its host, a session of it
    /// to forward through, and whether it is this computer's (then it
    /// is here already).
    Listening { host_key: String, session_id: String, port: u16, here: bool },
    /// A port forwarded here: the forward, the port here, and whether
    /// it failed to come back (then it is removed, not stopped).
    Forwarded { id: String, local_port: u16, failed: bool },
}

/// A row of the Ports list: a port, the program on it, its host, and
/// how it stands (listening there; forwarded here, to which port).
#[derive(Clone, Debug, PartialEq)]
pub struct PortRow {
    pub kind: PortKind,
    /// The port, where it is.
    pub port: String,
    /// The port it is on this computer, which Open opens: a forwarded
    /// port's port here, this computer's own again; none for another
    /// host's that isn't forwarded.
    pub here: String,
    pub program: String,
    pub host: String,
    pub light: Light,
    /// How it stands, in a word, as a host's row says it.
    pub state: String,
    /// Why a forward couldn't be brought back, where one couldn't.
    pub trouble: Option<String>,
}

/// The ports as the Hosts window lists them: those forwarded here
/// first (they are the person's own doing), then those programs in
/// the panes listen on.
/// The demo's forwards, which it has no hosts to make
/// (`OLO_REX_DEMO_HOSTS=states`, for the window's pictures): a forward
/// that works, one of the host being connected to, one of the host
/// that isn't connected.
fn demo_forwards() -> &'static [(&'static str, &'static str, u16, &'static str, Light, &'static str)] {
    static DEMO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DEMO.get_or_init(|| std::env::var("OLO_REX_DEMO_HOSTS").ok().as_deref() == Some("states")) {
        &[
            ("demo-1", "5432", 49152, "dev-box", Light::On, "Forwarded"),
            // (On the host that is being connected to, and saying what
            // a real one says there: `forward_said`.)
            ("demo-2", "8080", 49153, "build", Light::Waiting, "Waiting for host"),
            ("demo-3", "9090", 49154, "nas", Light::Off, "Waiting for host"),
        ]
    } else {
        &[]
    }
}

pub fn port_rows(hub: &RexHub) -> Vec<PortRow> {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let label = |key: &str| st.hosts.get(key).map(|h| h.label.clone()).unwrap_or_else(|| key.to_owned());
    // (Each with where it goes: its host's place in the Hosts list over
    // it, then its number; a port forwarded before one only listened
    // on.)
    let place = |key: &str| st.host_order.iter().position(|k| k == key).unwrap_or(usize::MAX);
    let mut rows: Vec<((usize, u16, u8), PortRow)> = Vec::new();
    for f in &st.forwards {
        let local_port = f.local_addr.port();
        let (light, state, trouble) = forward_said(&f.state, st.hosts.get(&f.host_key).map(|h| &h.conn_state));
        rows.push((
            (place(&f.host_key), f.remote_port, 0),
            PortRow {
                kind: PortKind::Forwarded { id: f.id.clone(), local_port, failed: trouble.is_some() },
                port: f.remote_port.to_string(),
                here: local_port.to_string(),
                program: String::new(),
                host: label(&f.host_key),
                light,
                state: olo_kit::words::tr(state).into(),
                trouble,
            },
        ));
    }
    for (bk, br) in &st.blocks {
        let g = br.lock().unwrap_or_else(|e| e.into_inner());
        let program = g.process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.clone()).unwrap_or_default();
        let here = st.hosts.get(&bk.host_key).is_some_and(|h| h.is_local);
        for pe in &g.ports.ports {
            rows.push((
                (place(&bk.host_key), pe.port, 1),
                PortRow {
                    kind: PortKind::Listening { host_key: bk.host_key.clone(), session_id: bk.session_id.clone(), port: pe.port, here },
                    port: pe.port.to_string(),
                    here: if here { pe.port.to_string() } else { String::new() },
                    program: program.clone(),
                    host: label(&bk.host_key),
                    // (Working: green, as a connected host. Grey is for off.)
                    light: Light::On,
                    state: olo_kit::words::tr("Listening").into(),
                    trouble: None,
                },
            ));
        }
    }
    for &(id, port, local_port, host, light, state) in demo_forwards() {
        rows.push((
            (place(host), port.parse().unwrap_or(0), 0),
            PortRow {
                kind: PortKind::Forwarded { id: id.to_owned(), local_port, failed: false },
                port: port.to_owned(),
                here: local_port.to_string(),
                program: String::new(),
                host: host.to_owned(),
                light,
                state: olo_kit::words::tr(state).into(),
                trouble: None,
            },
        ));
    }
    // (An order that holds from one look to the next, the panes being
    // kept in a map, and that is the Hosts list's own: this computer's
    // ports first, then each host's as the hosts stand, by number. They
    // were those forwarded, then those listened on, and the list read
    // as unsorted: 5432, 8080, 9090, 3000. Opus's fourth review.)
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// How many rows a list of the Hosts window has room for: all of a few,
/// never fewer than `least` (a list of one row reads as a field), and
/// past eight it scrolls.
pub fn list_rows(rows: usize, least: usize) -> usize {
    rows.clamp(least, 8)
}

/// Where the Status column begins in both of the Hosts window's lists
/// (one over the other: the lights in one line), and how wide it is: a
/// state is a word or two.
pub const STATUS_AT: f32 = 384.0;
pub const STATUS: f32 = 146.0;
/// The Ports list's two columns of numbers: a port (five figures at
/// most), and the port it is here.
const PORT: f32 = 48.0;
const HERE: f32 = 56.0;

/// A light and the words beside it, in a list's cell: the light where
/// the cell's text begins, the words after it.
pub fn lit_cell(g: &mut olo_kit::Painter, cell: &mut olo_kit::widgets::Cell, light: Light, words: &str) {
    let b = cell.bounds;
    style::dot(g, point(b.left() + px(m::CELL_INSET + LIGHT / 2.0), b.center().y), LIGHT, light.color(&theme::palette()));
    let x = b.left() + px(m::CELL_INSET + LIGHT + m::TIGHT);
    style::label_fit(g, x, b.center().y, style::Align::Left, words.to_owned(), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL), cell.text_color, b.right() - px(m::CELL_INSET) - x);
    cell.say(words.to_owned());
}

/// [`lit_cell`] for a thing being done now: a ring in the waiting
/// light's colour where the light would be (the kit's `busy_of`, in a
/// list's cell), the words after it.
pub fn ring_cell(g: &mut olo_kit::Painter, cell: &mut olo_kit::widgets::Cell, words: &str) {
    let b = cell.bounds;
    style::ring(g, point(b.left() + px(m::CELL_INSET + LIGHT / 2.0), b.center().y), LIGHT, Light::Waiting.color(&theme::palette()));
    let x = b.left() + px(m::CELL_INSET + LIGHT + m::TIGHT);
    style::label_fit(g, x, b.center().y, style::Align::Left, words.to_owned(), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL), cell.text_color, b.right() - px(m::CELL_INSET) - x);
    cell.say(words.to_owned());
}

/// The Hosts window: the hosts as a list (its name, its address, how
/// it stands, with its light), Add…, Remove and Connect under it for
/// the one chosen, what the chosen one's trouble is in full; and the
/// ports as a list (those forwarded here, those programs in the panes
/// listen on), with Forward…, Stop and Open under it. No Close: it is
/// a window that stays, with its tab's close box. (It was a row of
/// buttons for each host and each port, and two forms set in the
/// window: the design reviews of 2026-11-21 and 22.)
#[allow(clippy::too_many_arguments)]
pub fn render_hosts_panel(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    hosts_list: &ListModel,
    hosts: &std::rc::Rc<Vec<HostRow>>,
    chosen_host: Option<usize>,
    ports_list: &ListModel,
    ports: &std::rc::Rc<Vec<PortRow>>,
    chosen_port: Option<usize>,
    can_forward: bool,
    on_host: impl Fn(&(), &mut WindowCx) + 'static,
    on_add_host: impl Fn(&(), &mut WindowCx) + 'static,
    on_connect_host: impl Fn(&(String, bool), &mut WindowCx) + 'static,
    on_remove_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_manage: impl Fn(&String, &mut WindowCx) + 'static,
    on_port: impl Fn(&(), &mut WindowCx) + 'static,
    on_forward: impl Fn(&(), &mut WindowCx) + 'static,
    on_stop: impl Fn(&String, &mut WindowCx) + 'static,
    on_open: impl Fn(&PortKind, &mut WindowCx) + 'static,
) -> Element {
    // (As tall as so many rows take, to the device pixel: the kit's
    // own sum. Added up here it came short at 1.25× and 1.75×, and
    // both lists had a live scroll bar with every row showing.)
    let tall = |rows: usize| px(olo_kit::widgets::list_height(rows, m::LIST_ROW, true));

    // The hosts. (Each cell from the window's own rows, as the list
    // draws them when it wants them.)
    let names = hosts.clone();
    let host_list = ColumnListView::new(
        cx,
        "rex-hosts",
        hosts_list,
        vec![Column::new("Host", 124.0), Column::new("Address", STATUS_AT - 124.0), Column::new("Status", STATUS)],
        hosts.len(),
        |v: &crate::ui::RexWindow, cell, g| {
            let Some(h) = v.hosts_rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.text(g, h.name.clone()),
                // (This computer's "Local" isn't an address: quieter.)
                1 if h.local && !cell.selected => cell.text_with(g, h.address.clone(), style::ui_font(olo_kit::FontWeight::NORMAL), style::secondary_text(&theme::palette())),
                1 => cell.text(g, h.address.clone()),
                _ => lit_cell(g, cell, h.light, &h.state),
            }
        },
    )
    .row_key(|v: &crate::ui::RexWindow, row| v.hosts_rows.get(row).map_or(0, |h| style::key((&h.key, &h.name, &h.address, h.light, &h.state))))
    .type_select(move |_, i| names.get(i).map(|h| h.name.clone().into()).unwrap_or_default())
    .on_select(on_host);
    let host_list = div().w_full().h(tall(list_rows(hosts.len(), 5))).child(host_list);
    // Every button under the two lists as wide as the widest of them:
    // one column of left edges, one of right. (Four of them under the
    // hosts: what they come to is to be less than the window. As wide
    // as "Manage Rex Server…", which was here until that became the
    // Host window's, they were more, and Connect was cut at the
    // window's edge, 2026-11-23.)
    let wide = ["Add…", "Remove", "Manage Host…", "Connect", "Disconnect", "Forward…", "Stop", "Open"].iter().map(|l| olo_kit::widgets::button_width(l, false)).fold(0.0, f32::max);

    // Under it: what is done to the one chosen. This computer's own
    // server isn't removed; Connect is for one that isn't connected,
    // and is Disconnect for one that is, in the same place (a host that
    // was connected had a button that only greyed: no way back, Opus's
    // review of the window, 2026-11-23).
    let picked = chosen_host.and_then(|i| hosts.get(i));
    let (pick_remove, pick_connect, pick_server) = (picked.map(|h| h.key.clone()), picked.map(|h| h.key.clone()), picked.map(|h| h.key.clone()));
    let host_actions = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(Button::new("Add…").min_width(wide).on_click(move |_, cx| on_add_host(&(), cx)))
        .child(Button::new("Remove").min_width(wide).enabled(picked.is_some_and(|h| !h.local)).on_click(move |_, cx| {
            if let Some(key) = &pick_remove {
                on_remove_host(key, cx);
            }
        }))
        .child(div().flex_1())
        // (Everything of Olo's on the chosen host, and its Rex server:
        // the Host window. For any host but this computer.)
        .child(Button::new("Manage Host…").min_width(wide).enabled(picked.is_some_and(|h| !h.local)).on_click(move |_, cx| {
            if let Some(key) = &pick_server {
                on_manage(key, cx);
            }
        }))
        .child({
            let disconnects = picked.is_some_and(|h| h.disconnects);
            let enabled = picked.is_some_and(|h| if h.disconnects { !h.local } else { h.connects });
            Button::new(if disconnects { "Disconnect" } else { "Connect" }).min_width(wide).enabled(enabled).on_click(move |_, cx| {
                if let Some(key) = &pick_connect {
                    on_connect_host(&(key.clone(), disconnects), cx);
                }
            })
        });
    // What went wrong with the chosen one, in its own words, under the
    // buttons: a line's room always, so choosing a host moves nothing
    // (its row says "Failed"; the reason here, after its red light,
    // in the window's own ink: a whole line of red was the loudest
    // thing in the window).
    let trouble = picked.and_then(|h| h.trouble.clone());
    // (Where nothing went wrong, how the chosen host stands, quieter:
    // `host_line`. The line is never empty.)
    let said: String = match picked {
        Some(h) => h.line.clone(),
        None => olo_kit::words::tr("Choose a host to connect it, disconnect it or manage it.").into(),
    };
    // One line, always, in a row of its own height: choosing a host
    // moves nothing under it. A reason too long for the line is cut
    // with "…", its whole in the line's tip, to agents, and in the Host
    // window (it wrapped, and the Ports under it moved down a line for
    // that host alone: Opus's reviews, 2026-11-23). The light and the
    // words are centred in the same row (the light in a row of its own
    // beside a paragraph stood a point low against the capitals:
    // Gemini's fourth look).
    let row = style::line_row(style::PLAIN_SIZE);
    let trouble = div().w_full().h(px(row)).child(match trouble {
        Some(why) => div()
            .w_full()
            .h(px(row))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .child(light(Light::Down))
            .child(div().flex_1().min_w_0().child(StringView::new(why).cut_words(true).height(row))),
        None => div().w_full().child(StringView::new(said).dim(true).cut_words(true).height(row)),
    });

    // The ports.
    let port_list = ColumnListView::new(
        cx,
        "rex-ports",
        ports_list,
        // (The port, and the port it is here: two columns of numbers,
        // set right. They were one, "5432 → 49152" under "Port", which
        // didn't say which was which; and "Port → Here" over it was a
        // title with an arrow in it, over rows that had none. Opus's
        // reviews, 2026-11-23.)
        vec![Column::new("Port", PORT).right(), Column::new("Here", HERE).right(), Column::new("Program", 116.0), Column::new("Host", STATUS_AT - 220.0), Column::new("Status", STATUS)],
        ports.len(),
        |v: &crate::ui::RexWindow, cell, g| {
            let Some(p) = v.ports_rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.text(g, p.port.clone()),
                // (Not here, nor a program known: a dash, quiet, not a
                // blank that reads as something not loaded.)
                1 | 2 if (if cell.column == 1 { &p.here } else { &p.program }).is_empty() => {
                    let ink = if cell.selected { cell.text_color } else { style::secondary_text(&theme::palette()) };
                    cell.text_with(g, "–", style::ui_font(olo_kit::FontWeight::NORMAL), ink)
                }
                1 => cell.text(g, p.here.clone()),
                2 => cell.text(g, p.program.clone()),
                3 => cell.text(g, p.host.clone()),
                _ => lit_cell(g, cell, p.light, &p.state),
            }
        },
    )
    .row_key(|v: &crate::ui::RexWindow, row| v.ports_rows.get(row).map_or(0, |p| style::key((&p.port, &p.here, &p.program, &p.host, p.light, &p.state))))
    .empty_text("No programs in the panes are listening on ports, and none is forwarded here.")
    .on_select(on_port);
    let port_list = div().w_full().h(tall(list_rows(ports.len(), 4))).child(port_list);
    let port = chosen_port.and_then(|i| ports.get(i));
    let stop = port.and_then(|p| match &p.kind {
        PortKind::Forwarded { id, failed, .. } => Some((id.clone(), *failed)),
        PortKind::Listening { .. } => None,
    });
    // (A forward that couldn't come back isn't running: removed, not stopped.)
    let stop_words = if stop.as_ref().is_some_and(|(_, failed)| *failed) { "Remove" } else { "Stop" };
    let stop_id = stop.map(|(id, _)| id);
    let stops = stop_id.is_some();
    // (Open: a port here is opened; another host's is forwarded here
    // first, and one already forwarded is opened where it is here.
    // One word for the three: its row then says where it is.)
    let opens = port.is_some_and(|p| !matches!(p.kind, PortKind::Forwarded { failed: true, .. }));
    let open_kind = port.map(|p| p.kind.clone());
    let port_actions = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(Button::new("Forward…").min_width(wide).enabled(can_forward).on_click(move |_, cx| on_forward(&(), cx)))
        .child(div().flex_1())
        .child(Button::new(stop_words).min_width(wide).enabled(stops).on_click(move |_, cx| {
            if let Some(id) = &stop_id {
                on_stop(id, cx);
            }
        }))
        .child(Button::new("Open").min_width(wide).enabled(opens).on_click(move |_, cx| {
            if let Some(kind) = &open_kind {
                on_open(kind, cx);
            }
        }));

    // (A forward that couldn't be brought back: why, as a host's reason
    // is said. At the window's foot, nothing under it to move.)
    let port_trouble = port.and_then(|p| p.trouble.clone()).map(|why| {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::TIGHT))
            .child(div().h(px(style::line_row(style::PLAIN_SIZE))).flex().items_center().child(light(Light::Down)))
            .child(div().flex_1().min_w_0().child(StringView::new(why).wrap(true)))
    });
    div()
        .flex()
        .flex_col()
        // (The reason's line has its room whether or not there is one:
        // with a section's whole step after it besides, an empty band
        // stood between the two lists.)
        .gap(px(m::SPACING))
        .p(px(m::WINDOW_INSET))
        .w_full()
        // (Each list under its heading, close; its buttons a step under
        // it; the chosen host's trouble close under those.)
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(StringView::new("Hosts").bold(true)).child(host_list))
                .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(host_actions).child(trouble)),
        )
        // (Its heading close over its list; the list's buttons a step under it, as the hosts'.)
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(StringView::new("Ports").bold(true)).child(port_list))
                .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(port_actions).children(port_trouble)),
        )
}

/// Add Host's width: room for an address, and for a command a host is
/// asked to run, on a line.
pub const ADD_HOST_WIDTH: f32 = 480.0;

/// The size of what a host is asked to run, and of what it says.
pub const COMMAND_SIZE: f32 = style::SMALL_SIZE;
/// A well's room round what it holds.
pub const WELL_INSET: f32 = 8.0;

/// The Add Host window: a host's address and the name it goes by; for a
/// host reached over SSH, how Rex runs there and whether to set it up;
/// Cancel and Add. (`kind`: what the address typed is.)
#[allow(clippy::too_many_arguments)]
pub fn render_add_host_panel(
    cx: &WindowCx,
    name_field: &Child<TextField>,
    address_field: &Child<TextField>,
    kind: &crate::setup::Address,
    mode: olo_rex_client::ServerMode,
    set_up: bool,
    on_mode: impl Fn(&usize, &mut WindowCx) + 'static,
    on_set_up: impl Fn(&bool, &mut WindowCx) + 'static,
    on_done: impl Fn(&bool, &mut WindowCx) + 'static,
) -> Element {
    use crate::setup::Address;
    let p = theme::palette();
    let can_add = matches!(kind, Address::Ssh(_) | Address::Other);
    // (How Rex runs there is said of a host reached over SSH; any other
    // address is of a server that runs already.)
    let over_ssh = !matches!(kind, Address::Other);
    let on_done = std::rc::Rc::new(on_done);
    let (cancel, add) = (on_done.clone(), on_done);
    let divider = divider_for(&["Address:", "Name:", "Server:"]);
    let width = ADD_HOST_WIDTH - 2.0 * m::WINDOW_INSET;
    let under = |text: StringView| div().pl(px(divider)).child(text.size(style::SMALL_SIZE).wrap(true));
    let modes: Vec<SharedString> = olo_rex_client::ServerMode::ALL.iter().map(|m| olo_kit::words::tr(m.name()).into()).collect();
    let chosen = olo_rex_client::ServerMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    let says = if over_ssh { mode.says() } else { "Only a host reached over SSH (ssh://you@host) is set up from here." };
    // (More follows Add where the host is looked at first: "Add…".)
    let add_label = if over_ssh && set_up { "Add…" } else { "Add" };
    div()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(div().flex().flex_col().gap(px(m::TIGHT)).w_full().child(TextControl::new(cx, address_field).label("Address:").divider(divider)).children(match kind {
            Address::Unfit(why) => Some(under(StringView::new(*why).color(style::c(p.failure)))),
            _ => None,
        }))
        .child(TextControl::new(cx, name_field).label("Name:").divider(divider))
        // (What the menu's choice means, close under it: its own.)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .w_full()
                .child(MenuField::new("add-host-server", modes, chosen).label("Server:").divider(divider).width(width).enabled(over_ssh).on_change(on_mode))
                .child(under(StringView::new(says).dim(true))),
        )
        .child(div().pl(px(divider)).child(CheckBox::new("Set Rex up there if it needs it", set_up && over_ssh).id("add-host-set-up").enabled(over_ssh).on_toggle(on_set_up)))
        .child(
            olo_kit::widgets::button_row()
                .pt(px(m::TIGHT))
                .child(Button::new("Cancel").on_click(move |_, cx| cancel(&false, cx)))
                .child(Button::new(add_label).id("add-host-add").default(true).enabled(can_add).on_click(move |_, cx| add(&true, cx))),
        )
}

/// What a person may do about a host being set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupDo {
    /// Back to the form (nothing more is done there).
    Back,
    /// Carry the steps out (or try root again, with the key chosen).
    Go,
    /// Look at the host again.
    Again,
    /// Stop after the step it is at.
    Stop,
    /// Olo Passwords, opened: it has the keys.
    OpenPasswords,
}

/// A key as the menu of them has it: its name, its kind, and enough of
/// its fingerprint to tell it by.
pub fn key_called(key: &olo_rex_client::AgentKey) -> SharedString {
    let print = key.fingerprint.strip_prefix("SHA256:").unwrap_or(&key.fingerprint);
    let short =
        if print.chars().count() > 16 { format!("{}…{}", print.chars().take(8).collect::<String>(), print.chars().skip(print.chars().count() - 6).collect::<String>()) } else { print.to_owned() };
    let name = if key.name.trim().is_empty() { olo_kit::words::tr("A key with no name").to_string() } else { key.name.clone() };
    format!("{name}  ·  {}  ·  {short}", key.kind).into()
}

/// A command as it is set on lines no longer than `most` characters,
/// the way one is written out by hand: broken after `|` and `&&`, a
/// part still too long broken at a space with a `\` to say it goes on,
/// and every line after the first set in. (Left to wrap, a command's
/// last words came back to the margin and read as another command.)
pub fn command_lines(command: &str, most: usize) -> Vec<String> {
    const IN: &str = "  ";
    let mut parts: Vec<String> = Vec::new();
    let mut rest = command.trim();
    while let Some((at, op)) = [" | ", " && "].iter().filter_map(|op| rest.find(op).map(|at| (at, *op))).min() {
        parts.push(format!("{}{}", &rest[..at], op.trim_end()));
        rest = rest[at + op.len()..].trim_start();
    }
    parts.push(rest.to_owned());
    let mut lines: Vec<String> = Vec::new();
    for part in parts {
        let mut line = String::new();
        for word in part.split(' ') {
            let lead = if lines.is_empty() { 0 } else { IN.len() };
            // (Room kept for the mark that says a line goes on.)
            if !line.is_empty() && lead + line.chars().count() + 1 + word.chars().count() + 2 > most {
                lines.push(format!("{}{line} \\", if lines.is_empty() { "" } else { IN }));
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(format!("{}{line}", if lines.is_empty() { "" } else { IN }));
    }
    lines
}

/// A sunken well, as a field's, round `inner`: what is read as it
/// stands and not typed (a command to be run, what a host said).
pub fn well(inner: Element) -> Element {
    let back = canvas(|g, b| {
        let p = theme::palette();
        // (The panel's own ground, a little shaded: not a field's
        // white, which says "type here".)
        style::fill_rect(g, b, style::shade(&p, p.panel_background, 1.06));
        style::sunken(g, b, &p, false);
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div().relative().w_full().child(back).child(div().relative().w_full().px(px(WELL_INSET)).py(px(5.0)).child(inner))
}

/// A single-line sunken command well matching a button's height (`m::CONTROL_H`),
/// used beside Copy in empty states so the field and button sit flush.
pub fn command_well(inner: Element) -> Element {
    let back = canvas(|g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::shade(&p, p.panel_background, 1.06));
        style::sunken(g, b, &p, false);
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div().relative().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().child(back).child(div().relative().w_full().px(px(WELL_INSET)).child(inner))
}

/// `lines`, each on a line of its own, in the fixed-width font.
pub fn mono_lines(lines: Vec<String>) -> Element {
    div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(COMMAND_SIZE).wrap(true)))
}

/// How many characters of a command fit a well `width` points wide.
pub fn command_room(width: f32) -> usize {
    let ten = f32::from(style::text_width("0000000000", COMMAND_SIZE, style::mono_font(olo_kit::FontWeight::NORMAL)));
    (((width - 2.0 * WELL_INSET - 2.0) / (ten / 10.0)).floor() as usize).max(20)
}

/// The Add Host window while a host is set up: looked at, what it needs
/// put to the person, root signed in, the steps carried out with what
/// they write, or why it didn't go. Waiting and working have one
/// layout (a heading, a bar with what is being done over it, a line
/// under it, the buttons), so that going from one to the next changes
/// the words and the bar and nothing else; what needs the person (a key
/// to choose, something that didn't go) is an alert, its icon beside it.
/// (`phase`: where a busy bar's block is.)
pub fn render_setup_panel(setup: &crate::setup::Setup, phase: f32, on_do: impl Fn(&SetupDo, &mut WindowCx) + 'static, on_key: impl Fn(&usize, &mut WindowCx) + 'static) -> Element {
    use crate::setup::Stage;
    use olo_kit::widgets::{AlertFrame, StatusBar, alert_title};
    let p = theme::palette();
    let on_do = std::rc::Rc::new(on_do);
    let button = |label: &'static str, what: SetupDo| {
        let on_do = on_do.clone();
        Button::new(label).on_click(move |_, cx| on_do(&what, cx))
    };
    let width = ADD_HOST_WIDTH - 2.0 * m::WINDOW_INSET;
    let head = |text: String| StringView::new(text).bold(true).wrap(true);
    let body = |text: String| StringView::new(text).wrap(true);
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    let account = setup.target.user.clone().unwrap_or_default();
    let col = || div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).w_full();
    let buttons = || olo_kit::widgets::button_row().pt(px(m::TIGHT));
    let gap = |h: f32| div().h(px(h)).flex_shrink_0();
    // Waiting, and working: a heading, the bar with its words, a line.
    let waiting = |heading: String, bar: StatusBar, under: StringView, foot: Element| {
        col().child(head(heading)).child(bar).child(under.size(style::SMALL_SIZE).dim(true).cut(true).height(m::LABEL_ROW)).child(foot)
    };
    match &setup.stage {
        Stage::Looking => waiting(
            olo_kit::tr!("Looking at {}…", setup.host()).into(),
            StatusBar::busy(phase).label("Asking how Rex is set up there"),
            StringView::new(olo_kit::tr!("Nothing on {} is changed yet.", setup.host())),
            buttons().child(button("Cancel", SetupDo::Back)),
        ),
        // (Root reached before anything is changed: with sudo, as the
        // account, where it may use it; else signed in as root.)
        Stage::Root { .. } if setup.root_by == olo_rex_client::provision::RootBy::Sudo => waiting(
            olo_kit::tr!("Checking sudo on {}…", setup.host()).into(),
            StatusBar::busy(phase).label(olo_kit::tr!("Running sudo as {}", setup.account()).to_string()),
            StringView::new(olo_kit::tr!("Setting up needs root. Nothing on {} is changed yet.", setup.host())),
            buttons().child(button("Cancel", SetupDo::Back)),
        ),
        Stage::Root { .. } => waiting(
            olo_kit::tr!("Signing in as {}…", setup.root()).into(),
            StatusBar::busy(phase).label("Waiting for a key from Olo Passwords"),
            StringView::new(olo_kit::tr!("Setting up needs root. Nothing on {} is changed yet.", setup.host())),
            buttons().child(button("Cancel", SetupDo::Back)),
        ),
        Stage::Working { steps, at, said, stopping } => {
            let title = steps.get(*at).map(|s| olo_kit::words::of(s.title.clone()).to_string()).unwrap_or_default();
            // (A sliver for the step under way: the bar is never dead.)
            let bar = StatusBar::new((*at as f32 + 0.15) / steps.len().max(1) as f32).label(title).trailing(olo_kit::tr!("{} of {}", at + 1, steps.len()));
            let under = if *stopping { StringView::new("Stopping after this step…") } else { StringView::new(if said.is_empty() { " ".to_owned() } else { said.clone() }).mono(true) };
            waiting(olo_kit::tr!("Setting up {}…", setup.host()).into(), bar, under, buttons().child(button("Stop After This Step", SetupDo::Stop).enabled(!*stopping)))
        }
        Stage::Confirm { steps, ends_sessions } => {
            let heading = match setup.mode {
                olo_rex_client::ServerMode::Personal => olo_kit::tr!("Set up your own Rex server on {}", setup.host()),
                olo_rex_client::ServerMode::Shared => olo_kit::tr!("Set up {} as a shared Rex server", setup.host()),
                olo_rex_client::ServerMode::Login => olo_kit::tr!("Set up {} as a Rex login server", setup.host()),
            };
            // Each step: its number, then its title, what it is for, and
            // what is run, as it is run, in a well; grouped by who runs
            // them (root's first, as they are carried out), each group
            // under its heading, set apart from the one before.
            let number = f32::from(style::text_width(format!("{}.", steps.len()), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL))).ceil() + m::TIGHT;
            let room = command_room(width - number);
            let mut list = div().flex().flex_col().w_full();
            for (n, step) in steps.iter().enumerate() {
                if n == 0 || steps[n - 1].root != step.root {
                    // (How root's steps reach root, said: with sudo where
                    // the account may use it without a password, else a
                    // root login, and why: Ben, 2026-11-24.)
                    // (The two ways in sentences that mirror each other:
                    // which account can or can't, who Rex signs in as, and
                    // what must work before anything is changed: both
                    // looks, 2026-11-24.)
                    let (who, how) = if step.root && setup.root_by == olo_rex_client::provision::RootBy::Sudo {
                        (
                            olo_kit::words::tr("As root, with sudo").to_string(),
                            olo_kit::tr!("{} can use sudo there without a password, so Rex signs in as {}@{} and runs these with sudo. Nothing is changed until sudo works.", account, account, setup.host()).to_string(),
                        )
                    } else if step.root {
                        (
                            olo_kit::words::tr("As root, with an SSH key").to_string(),
                            olo_kit::tr!("{} can’t use sudo there without a password, so Rex signs in as {} with an SSH key from Olo Passwords. Nothing is changed until that sign-in works.", account, setup.root()).to_string(),
                        )
                    } else {
                        (olo_kit::tr!("As {}, over SSH", account).to_string(), olo_kit::tr!("Rex signs in as {}@{}, as it will to connect.", account, setup.host()).to_string())
                    };
                    // (A rule under each group's heading, as a preference
                    // panel's sections have: the window's title is over
                    // both groups, and no closer to the first than they
                    // are to each other.)
                    list = list
                        .children((n > 0).then(|| gap(m::SECTION)))
                        .child(StringView::new(who).bold(true).height(m::LABEL_ROW))
                        .child(gap(2.0))
                        .child(olo_kit::widgets::separator())
                        .child(gap(4.0))
                        .child(small(how))
                        .child(gap(m::SPACING));
                } else {
                    list = list.child(gap(m::SPACING + 2.0));
                }
                // (What is run, as it is run: root's command through sudo
                // shown with it, Opus's look.)
                let shown = match step.adds_to() {
                    Some(_) => step.shown().lines().map(str::to_owned).collect(),
                    None if step.root && setup.root_by == olo_rex_client::provision::RootBy::Sudo => command_lines(&olo_rex_client::provision::by_sudo(&step.shown()), room),
                    None => command_lines(&step.shown(), room),
                };
                let mut said = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT - 2.0)).child(body(olo_kit::words::of(step.title.clone()).to_string()));
                if !step.what.is_empty() {
                    said = said.child(small(olo_kit::words::of(step.what.clone()).to_string()));
                }
                // (What to be afraid of, in the warning's colour and no
                // other words in it, before what is run.)
                if let Some(warning) = step.warning() {
                    said = said.child(StringView::new(warning).size(style::SMALL_SIZE).color(style::c(p.warning)).wrap(true));
                }
                said = said.child(well(mono_lines(shown)));
                list = list.child(div().flex().flex_row().items_start().w_full().child(div().w(px(number)).flex_shrink_0().child(body(format!("{}.", n + 1)))).child(said));
            }
            let go = if *ends_sessions { "Stop Server and Set Up" } else { "Set Up" };
            col().child(head(heading.into())).child(list.pt(px(m::SPACING))).child(buttons().child(button("Back", SetupDo::Back)).child(button(go, SetupDo::Go).default(true)))
        }
        // What needs the person: an alert.
        Stage::RootKey { keys, .. } if keys.is_empty() => AlertFrame::new("key")
            .child(alert_title("Olo Passwords has no SSH keys to offer"))
            .child(gap(m::TIGHT))
            .child(body(olo_kit::tr!("{} can’t use sudo on {} without a password, so setting up needs root’s own login, or an account that can. Unlock Olo Passwords, or add a key to it that root there accepts.", setup.account(), setup.host()).into()))
            .child(gap(m::SPACING))
            // (Open Passwords is a side trip, not a third answer: at
            // the left, under the words; the alert's two at the right.)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .pt(px(m::TIGHT))
                    .child(button("Open Passwords", SetupDo::OpenPasswords))
                    .child(div().flex_1())
                    .child(olo_kit::widgets::button_row().child(button("Back", SetupDo::Back)).child(button("Try Again", SetupDo::Go).default(true))),
            )
            .into_element(),
        Stage::RootKey { keys, chosen, .. } => AlertFrame::new("key")
            .child(alert_title(olo_kit::tr!("{} didn’t accept your keys for root", setup.host())))
            .child(gap(m::TIGHT))
            // (The fact first, then what follows from it, and the way
            // to act on it: both looks.)
            .child(body(olo_kit::tr!("{} can’t use sudo on {} without a password, so setting up needs root’s own login, or an account that can. Choose a key that root accepts there. If none does, add your public key to /root/.ssh/authorized_keys on that machine, then try again.", setup.account(), setup.host()).into()))
            .child(gap(m::SPACING))
            // (As wide as the words over it: the alert's column.)
            .child(
                MenuField::new("setup-root-key", keys.iter().map(key_called).collect(), *chosen)
                    .label("Key:")
                    .width(ADD_HOST_WIDTH - olo_kit::widgets::ALERT_TEXT_X - m::WINDOW_INSET)
                    .on_change(on_key),
            )
            .child(gap(m::SPACING))
            .child(buttons().child(button("Back", SetupDo::Back)).child(button("Try Again", SetupDo::Go).default(true)))
            .into_element(),
        Stage::Failed { what, why, said, icon } => {
            let mut frame = AlertFrame::new(*icon).child(alert_title(what.clone()));
            if !why.is_empty() {
                frame = frame.child(gap(m::TIGHT)).child(body(why.clone()));
            }
            if !said.is_empty() {
                // (What the host said, as it said it: told from Rex's own words.)
                frame = frame
                    .child(gap(m::SPACING))
                    .child(small(olo_kit::tr!("{} said:", setup.host()).into()))
                    .child(gap(m::TIGHT - 2.0))
                    .child(well(StringView::new(said.clone()).mono(true).size(COMMAND_SIZE).wrap(true).into_element()));
            }
            frame.child(gap(m::SPACING)).child(buttons().child(button("Back", SetupDo::Back)).child(button("Try Again", SetupDo::Again).default(true))).into_element()
        }
        // (What the host is, said, with the one way on: back.)
        Stage::Cant { what, why } => AlertFrame::new("info")
            .child(alert_title(what.clone()))
            .child(gap(m::TIGHT))
            .child(body(why.clone()))
            .child(gap(m::SPACING))
            .child(buttons().child(button("Back", SetupDo::Back).default(true)))
            .into_element(),
        Stage::Done => col(),
    }
}

/// The Move window's width: Add Host's.
pub const MOVE_WIDTH: f32 = ADD_HOST_WIDTH;

/// What the Move window asks of the move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveDo {
    /// The destination chosen in its menu (by its place).
    Pick(usize),
    /// Go on with the destination chosen.
    Choose,
    /// Its folders in `/var/tmp` brought too, or not.
    VarTmp(bool),
    /// `~/.cache` brought too, or not.
    Cache(bool),
    /// The plan agreed to (or Try Again): move.
    Go,
    /// The source started again as it was.
    GoBack,
    /// No other host to move to: Add Host.
    AddHost,
    /// Refused: back to choosing the host.
    Rechoose,
    /// Stop where it stands, and close.
    Cancel,
    /// Close the window (a move done, or one that can't be made).
    Close,
}

/// The Move window: where a move of an account to another host stands
/// (`crate::moving`), each stage as Add Host's setting up is drawn.
pub fn render_move_panel(mv: &crate::moving::Move, phase: f32, on_do: impl Fn(&MoveDo, &mut WindowCx) + 'static) -> Element {
    use crate::moving::Stage;
    use olo_kit::FontWeight;
    use olo_kit::widgets::{AlertFrame, MenuField, StatusBar, alert_title};
    use olo_rex_client::migrate::size_words;
    let on_do = std::rc::Rc::new(on_do);
    let button = |label: &'static str, what: MoveDo| {
        let on_do = on_do.clone();
        Button::new(label).on_click(move |_, cx| on_do(&what, cx))
    };
    // (Its titles as the other alerts' are; the plan's heads a step under
    // them, quiet: the parts of one question, not its peers.)
    let title = |text: String| StringView::new(text).bold(true).wrap(true);
    let body = |text: String| StringView::new(text).wrap(true);
    let secondary = |text: String| StringView::new(text).dim(true).wrap(true);
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    // (A list's line: one row a label's height, its rows 20 points apart.)
    let cell = |text: String| StringView::new(text).height(m::LABEL_ROW);
    let col = || div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).w_full();
    let buttons = || olo_kit::widgets::button_row().pt(px(m::TIGHT));
    let gap = |h: f32| div().h(px(h)).flex_shrink_0();
    let width = |t: &str, weight| f32::from(style::text_width(t.to_owned(), style::PLAIN_SIZE, style::ui_font(weight))).ceil();
    // (A full stop at a line's end; what a host or a program said, its
    // first letter capital too. Never a line of the move's own, which may
    // name a host: as the person spelled it.)
    let stop = |text: &str| -> String {
        let mut out = text.trim().to_owned();
        if !out.is_empty() && !out.ends_with(['.', '!', '?', ':']) {
            out.push('.');
        }
        out
    };
    let sentence = |text: &str| -> String {
        let t = text.trim();
        stop(&t.chars().take(1).flat_map(char::to_uppercase).chain(t.chars().skip(1)).collect::<String>())
    };
    // A group of the plan: its quiet head, then its rows; a step above
    // each but the first.
    let group = |first: bool, head: String, rows: Vec<Element>| {
        div()
            .flex()
            .flex_col()
            .w_full()
            .pt(px(if first { 0.0 } else { 14.0 - m::SPACING }))
            .child(StringView::new(head).size(style::SMALL_SIZE).bold(true).dim(true).height(m::LABEL_ROW))
            .child(gap(4.0))
            .child(div().flex().flex_col().gap(px(4.0)).w_full().children(rows))
    };
    // Every stage under way one height: its title, the bar with its words
    // above it and a line under it (never empty), then the button row with
    // how the old host stands at its left and Cancel (dimmed where it
    // can't be).
    let progress = |heading: String, bar: StatusBar, detail: String, state: String, can_cancel: bool| {
        col()
            .child(title(heading))
            // (The detail 8 points under the bar: the three lines one unit.)
            .child(div().flex().flex_col().gap(px(4.0)).w_full().child(bar).child(StringView::new(detail).size(style::SMALL_SIZE).dim(true).cut(true).height(m::LABEL_ROW)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .pt(px(m::TIGHT))
                    // (On the button's baseline, smaller as it is.)
                    .child(div().flex_1().min_w_0().child(StringView::new(state).size(style::SMALL_SIZE).dim(true).beside(style::PLAIN_SIZE).cut(true)))
                    .child(button("Cancel", MoveDo::Cancel).enabled(can_cancel)),
            )
            .into_element()
    };
    let (from, to) = (mv.source_label.clone(), mv.dest_label.clone());
    match &mv.stage {
        Stage::Choosing { choices, .. } if choices.is_empty() => AlertFrame::new("info")
            .child(alert_title(olo_kit::words::tr("No other host to move to").to_string()))
            .child(gap(m::TIGHT))
            .child(body(olo_kit::words::tr("Add the host you want to move to and connect to it, then come back.").into()))
            .child(gap(m::SPACING))
            .child(buttons().child(button("Close", MoveDo::Close)).child(button("Add Host…", MoveDo::AddHost).default(true)))
            .into_element(),
        Stage::Choosing { choices, chosen } => {
            let names: Vec<olo_kit::SharedString> = choices.iter().map(|(_, label)| label.clone().into()).collect();
            let on_pick = on_do.clone();
            let menu = MenuField::new("move-to", names, *chosen).label("To:").width(160.0).on_change(move |i: &usize, cx| on_pick(&MoveDo::Pick(*i), cx));
            // (What goes, quiet; what becomes of the old host, a line of
            // its own in the body's colour: the one that matters.)
            col()
                .child(title(olo_kit::tr!("Move {} to another host", from).into()))
                .child(menu)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .w_full()
                        .child(small(
                            olo_kit::tr!("Your home, your agents’ settings, sign-ins and Rex sessions go straight from {} to the host you choose, not through this computer.", from).to_string(),
                        ))
                        .child(body(olo_kit::tr!("Afterwards, {} is stopped; its files stay where they are.", from).to_string())),
                )
                .child(buttons().child(button("Cancel", MoveDo::Cancel)).child(button("Continue", MoveDo::Choose).default(true)))
                .into_element()
        }
        Stage::Looking { what, step } => progress(
            olo_kit::tr!("Looking at {} and {}…", from, to).into(),
            StatusBar::busy(phase).label(what.clone()),
            olo_kit::tr!("Step {} of 4", step).to_string(),
            olo_kit::words::tr("Nothing has changed yet.").to_string(),
            true,
        ),
        Stage::Refused { why, more } => {
            let mut frame = AlertFrame::new("warning").child(alert_title(olo_kit::tr!("Can’t move {} to {}", from, to).to_string()));
            for line in why {
                frame = frame.child(gap(m::TIGHT)).child(body(sentence(line)));
            }
            let mut row = buttons();
            if *more {
                row = row.child(button("Choose Another Host…", MoveDo::Rechoose));
            }
            frame.child(gap(m::SPACING)).child(row.child(button("Close", MoveDo::Close).default(true))).into_element()
        }
        Stage::Confirm { plan, choices } => {
            // The panes: each one's name, and what it comes back as (an
            // agent resumed; a command typed or run again, in the
            // terminal's face, as quiet as the rest of its column).
            let agent_name = |a: &str| {
                match a {
                    "claude" => "Claude Code",
                    "codex" => "Codex",
                    "agy" => "Antigravity",
                    other => other,
                }
                .to_owned()
            };
            let total: usize = plan.captured.iter().map(|c| c.panes.len()).sum();
            // (Six at most, then how many more: an account of many panes
            // still fits the smallest screen.)
            let shown = if total > 7 { 6 } else { total };
            let mut entries: Vec<(String, Element)> = Vec::new();
            for c in &plan.captured {
                let (runs, _, run_it) = olo_rex_client::migrate::pane_runs(c, &plan.panes, &|p| p.to_owned());
                for p in &c.panes {
                    if entries.len() == shown {
                        break;
                    }
                    let name = if p.label.is_empty() { olo_kit::words::tr("A pane").to_string() } else { p.label.clone() };
                    let agent = p.pid.and_then(|pid| plan.panes.iter().find(|x| x.pid == pid)).and_then(|x| x.agent.clone());
                    let command = runs.get(&p.block_id).and_then(|r| r.command.clone()).map(|c| olo_rex_client::migrate::command_text(&c));
                    let what: Element = match (agent, command) {
                        (Some(a), _) => cell(match a.how.as_str() {
                            "guess" => olo_kit::tr!("{}, resumed (a guess at which conversation)", agent_name(&a.agent)).to_string(),
                            "none" => olo_kit::tr!("{}, started again", agent_name(&a.agent)).to_string(),
                            _ => olo_kit::tr!("{}, resumed", agent_name(&a.agent)).to_string(),
                        })
                        .dim(true)
                        .cut(true)
                        .into_element(),
                        (None, Some(command)) => {
                            // (What becomes of it first, the command after in
                            // the rest of the row: a cut text has no width of
                            // its own to keep. The terminal's face a point
                            // smaller: its x-height is the larger.)
                            let how = if run_it.get(&p.block_id).copied().unwrap_or(false) { olo_kit::words::tr("Runs again:") } else { olo_kit::words::tr("Typed, not run:") };
                            div()
                                .w_full()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(div().flex_shrink_0().child(cell(how.to_string()).dim(true)))
                                .child(div().flex_1().min_w_0().child(cell(command).mono(true).size(style::MONO_SIZE - 1.0).height(m::LABEL_ROW).beside(style::PLAIN_SIZE).dim(true).cut(true)))
                                .into_element()
                        }
                        (None, None) => cell(olo_kit::words::tr("A shell, in its folder").to_string()).dim(true).cut(true).into_element(),
                    };
                    entries.push((name, what));
                }
            }
            // One column for the values of both lists (the hosts' systems,
            // what each pane comes back as), as far right as the further of
            // the two needs.
            let (from_word, to_word) = (olo_kit::words::tr("From").to_string(), olo_kit::words::tr("To").to_string());
            let labels_w = width(&from_word, FontWeight::NORMAL).max(width(&to_word, FontWeight::NORMAL));
            let names_w = width(&from, FontWeight::BOLD).max(width(&to, FontWeight::BOLD)).min(160.0);
            let panes_w = entries.iter().map(|(n, _)| width(n, FontWeight::NORMAL)).fold(0.0, f32::max).min(120.0);
            let value_x = (labels_w + m::TIGHT + names_w + m::SPACING).max(panes_w + m::SPACING);
            // The hosts: From and To right-aligned and flush with the head,
            // the names bold, what each runs; the machine only where the
            // two differ.
            let host_row = |label: String, name: &str, f: &olo_rex_client::migrate::Facts| {
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(div().w(px(labels_w)).flex_shrink_0().flex().justify_end().child(cell(label).dim(true)))
                    .child(div().w(px(value_x - labels_w)).flex_shrink_0().pl(px(m::TIGHT)).pr(px(m::SPACING)).child(cell(name.to_owned()).bold(true).cut(true)))
                    .child(div().flex_1().min_w_0().child(cell(f.system_name.clone()).dim(true).cut(true)))
                    .into_element()
            };
            let mut hosts = vec![host_row(from_word, &from, &plan.from), host_row(to_word, &to, &plan.to)];
            if !plan.from.machine.is_empty() && !plan.to.machine.is_empty() && plan.from.machine != plan.to.machine {
                hosts.push(
                    StringView::new(olo_kit::tr!("{} is {}, {} is {}: programs built for one don’t run on the other.", from, plan.from.machine, to, plan.to.machine).to_string())
                        .color(style::c(theme::palette().warning))
                        .wrap(true)
                        .into_element(),
                );
            }
            if plan.from.home != plan.to.home {
                hosts
                    .push(small(olo_kit::tr!("Your home moves from {} to {}: the paths your agents and tools keep are made the new home’s.", plan.from.home, plan.to.home).to_string()).into_element());
            }
            // Your home: what it holds, its largest folders, the options,
            // and the room it needs (the options and that line nearer one
            // another than the groups are).
            let mut home = vec![body(olo_kit::tr!("{} in {} files", size_words(plan.from.home_bytes), style::thousands(plan.from.home_files as usize)).to_string()).into_element()];
            let big: Vec<String> = plan.from.big.iter().take(3).map(|b| format!("{} {}", b.path, size_words(b.bytes))).collect();
            if !big.is_empty() {
                home.push(small(olo_kit::tr!("Largest: {}", big.join(" · ")).to_string()).into_element());
            }
            let tmp_bytes = plan.from.var_tmp_bytes();
            let tmp_count = plan.from.var_tmp_count();
            let tmp_label = match tmp_count {
                0 => olo_kit::words::tr("Also copy your folders in /var/tmp (you have none)").to_string(),
                1 => olo_kit::tr!("Also copy your folder in /var/tmp ({})", size_words(tmp_bytes)).to_string(),
                n => olo_kit::tr!("Also copy your {} folders in /var/tmp ({})", n, size_words(tmp_bytes)).to_string(),
            };
            let (on_tmp, on_cache) = (on_do.clone(), on_do.clone());
            let tmp = CheckBox::new(tmp_label, choices.var_tmp).id("move-var-tmp").enabled(tmp_count > 0).on_toggle(move |on: &bool, cx| on_tmp(&MoveDo::VarTmp(*on), cx));
            let cache = CheckBox::new(olo_kit::words::tr("Also copy ~/.cache (rebuilt as it’s used)").to_string(), choices.cache)
                .id("move-cache")
                .on_toggle(move |on: &bool, cx| on_cache(&MoveDo::Cache(*on), cx));
            let fits = olo_rex_client::migrate::refusals(&plan.from, &plan.to, choices.var_tmp, &to).is_empty();
            let copied = plan.from.home_bytes + if choices.var_tmp { tmp_bytes } else { 0 };
            let need = |b: u64| b + b / 10 + (1 << 30);
            // (One line whether it fits or not, so the window keeps its
            // height as /var/tmp is ticked: its words where the checkboxes'
            // labels are, the warning's triangle in their marks' column.)
            let label_x = m::MARK_SIZE + m::MARK_GAP;
            let room: Element = if fits {
                div()
                    .w_full()
                    .pl(px(label_x))
                    .child(
                        StringView::new(olo_kit::tr!("Needs {} with room to spare · {} free on {}", size_words(need(copied)), size_words(plan.to.free), to).to_string())
                            .size(style::SMALL_SIZE)
                            .dim(true)
                            .height(m::LABEL_ROW)
                            .cut_words(true),
                    )
                    .into_element()
            } else {
                let said = if olo_rex_client::migrate::refusals(&plan.from, &plan.to, false, &to).is_empty() {
                    olo_kit::tr!("Needs {} with room to spare; {} free. Without /var/tmp it fits.", size_words(need(copied)), size_words(plan.to.free)).to_string()
                } else {
                    olo_kit::tr!("Needs {} with room to spare; only {} free on {}.", size_words(need(copied)), size_words(plan.to.free), to).to_string()
                };
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div().w(px(label_x)).h(px(m::LABEL_ROW)).flex_shrink_0().child(
                            canvas(|g, b| {
                                icons::paint(g, b, "warning");
                            })
                            .paint_key(1)
                            .w(px(m::MARK_SIZE))
                            .h(px(m::MARK_SIZE))
                            .mt(px(1.0)),
                        ),
                    )
                    .child(div().flex_1().min_w_0().child(cell(said).cut_words(true)))
                    .into_element()
            };
            // (The checkboxes a control's room apart, as far below
            // "Largest" as from each other; the room line close under
            // them.)
            home.push(
                div()
                    .pt(px(5.0))
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(div().flex().flex_col().gap(px(m::TIGHT)).w_full().child(tmp).child(cache))
                    .child(div().pt(px(2.0)).w_full().child(room))
                    .into_element(),
            );
            // On the new host: the panes, then the keys and the Token Pool.
            let mut there: Vec<Element> = entries
                .into_iter()
                .map(|(name, what)| {
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(div().w(px(value_x)).flex_shrink_0().pr(px(m::SPACING)).child(cell(name).cut(true)))
                        .child(div().flex_1().min_w_0().child(what))
                        .into_element()
                })
                .collect();
            if total > shown {
                there.push(small(olo_kit::words::count((total - shown) as u64, "and 1 more pane", "and {n} more panes")).into_element());
            }
            if total == 0 {
                there.push(secondary(olo_kit::words::tr("No Rex sessions.").to_string()).into_element());
            }
            there.push(div().pt(px(4.0)).w_full().child(secondary(olo_kit::tr!("The SSH keys that sign in to {} will sign in to {} too.", from, to).to_string())).into_element());
            if plan.had_pool {
                there.push(secondary(olo_kit::tr!("Your sign-ins in the Token Pool move to {}.", to).to_string()).into_element());
            }
            // On the old host: when it stops, and what stops with it.
            let after = vec![
                body(olo_kit::tr!("You can keep using {} while your files are copied. Then it stops for good, after a last copy of what changed; its files are kept.", from).to_string())
                    .into_element(),
                secondary(olo_kit::words::tr("Stopped with it: Rex, agents, services, timers and crontab (SSH stays up).").to_string()).into_element(),
            ];
            col()
                .child(title(olo_kit::tr!("Move {} to {}", from, to).into()))
                .child(group(true, olo_kit::words::tr("Hosts").to_string(), hosts))
                .child(group(false, olo_kit::words::tr("Home folder").to_string(), home))
                .child(group(false, olo_kit::tr!("On {}", to).to_string(), there))
                .child(group(false, olo_kit::tr!("On {}", from).to_string(), after))
                .child(buttons().child(button("Cancel", MoveDo::Cancel)).child(button("Move", MoveDo::Go).default(fits).enabled(fits)))
                .into_element()
        }
        Stage::Copying { status } => {
            let (bytes, total) = (status["bytes"].as_u64().unwrap_or(0), status["total_bytes"].as_u64().unwrap_or(0));
            let rate = status["rate"].as_u64().unwrap_or(0);
            // (How long is left, at the speed it goes: minutes, then hours.)
            let left = match (rate > 0 && total > bytes).then(|| (total - bytes) / rate) {
                Some(secs) if secs < 90 => olo_kit::words::tr("About a minute left").to_string(),
                Some(secs) if secs < 90 * 60 => olo_kit::words::count(secs.div_ceil(60), "About 1 minute left", "About {n} minutes left"),
                Some(secs) => olo_kit::words::count(secs.div_ceil(3600), "About 1 hour left", "About {n} hours left"),
                None => String::new(),
            };
            let (bar, detail) = if total == 0 {
                (StatusBar::busy(phase).label(olo_kit::words::tr("Counting the files to copy").to_string()), olo_kit::words::tr("They’re sent in parallel streams once counted.").to_string())
            } else {
                let files = olo_kit::tr!("{} of {} files", style::thousands(status["files"].as_u64().unwrap_or(0) as usize), style::thousands(status["total_files"].as_u64().unwrap_or(0) as usize))
                    .to_string();
                let detail = if rate > 0 { format!("{files} · {}/s", size_words(rate)) } else { files };
                (StatusBar::new((bytes as f32 / total as f32).min(1.0)).label(olo_kit::tr!("{} of {}", size_words(bytes), size_words(total)).to_string()).trailing(left), detail)
            };
            progress(olo_kit::tr!("Copying your files to {}…", to).into(), bar, detail, olo_kit::tr!("You can keep using {} meanwhile.", from).to_string(), true)
        }
        Stage::Switching { at, status } => {
            let steps = crate::moving::SWITCHING;
            let bar = StatusBar::new((*at as f32 + 0.15) / steps as f32).label(crate::moving::switching_step(*at, &from, &to));
            let detail = match status["files"].as_u64() {
                Some(files) if *at == 1 => olo_kit::tr!(
                    "Step {} of {} · {} of {} changed files copied",
                    at + 1,
                    steps,
                    style::thousands(files as usize),
                    style::thousands(status["total_files"].as_u64().unwrap_or(0) as usize)
                )
                .to_string(),
                _ => olo_kit::tr!("Step {} of {}", at + 1, steps).to_string(),
            };
            progress(olo_kit::tr!("Moving {} to {}…", from, to).into(), bar, detail, olo_kit::tr!("Until this finishes, {} stays stopped.", from).to_string(), false)
        }
        Stage::Done { said } => {
            let mut frame = AlertFrame::new("info").child(alert_title(olo_kit::tr!("Moved to {}", to).to_string())).child(gap(m::TIGHT));
            for (i, line) in said.iter().enumerate() {
                if i > 0 {
                    frame = frame.child(gap(m::TIGHT - 2.0));
                }
                frame = frame.child(body(stop(line)));
            }
            frame.child(gap(m::SPACING)).child(buttons().child(button("Close", MoveDo::Close).default(true))).into_element()
        }
        Stage::Failed { what, why, stopped } => {
            // (The title says what went wrong and, where it is, that the
            // old host is stopped; the cause as the host said it, quiet
            // and close under it; then what each button does.)
            let what = olo_kit::words::of(what.clone()).to_string();
            let heading = match (*stopped, what.is_empty()) {
                (true, true) => olo_kit::tr!("The move didn’t finish, and {} is stopped", from).to_string(),
                (true, false) => olo_kit::tr!("{}, and {} is stopped", what, from).to_string(),
                (false, _) => what,
            };
            let mut frame = AlertFrame::new("warning").child(alert_title(heading));
            if !why.is_empty() {
                frame = frame.child(gap(4.0)).child(small(sentence(why))).child(gap(m::SPACING));
            } else {
                frame = frame.child(gap(m::TIGHT));
            }
            let state = if *stopped {
                olo_kit::tr!("Try Again goes on from where it left off. Undo Move starts {} again and puts {} back as it was.", from, to).to_string()
            } else {
                olo_kit::tr!("Nothing on {} was stopped; it keeps working. Close leaves the part already copied on {}; nothing else there changes.", from, to).to_string()
            };
            frame = frame.child(body(state));
            let row = if *stopped {
                buttons().child(button("Undo Move", MoveDo::GoBack)).child(button("Try Again", MoveDo::Go).default(true))
            } else {
                buttons().child(button("Close", MoveDo::Cancel)).child(button("Try Again", MoveDo::Go).default(true))
            };
            frame.child(gap(m::SPACING)).child(row).into_element()
        }
        Stage::GoingBack => progress(
            olo_kit::words::tr("Undoing the move…").to_string(),
            StatusBar::busy(phase).label(olo_kit::tr!("Starting {} again and putting {} back", from, to).to_string()),
            olo_kit::tr!("Nothing of yours on {} was deleted.", from).to_string(),
            olo_kit::tr!("Until this finishes, {} stays stopped.", from).to_string(),
            false,
        ),
        Stage::WentBack { said } => {
            let mut frame = AlertFrame::new("info").child(alert_title(olo_kit::words::tr("Move undone").to_string())).child(gap(m::TIGHT));
            for (i, line) in said.iter().enumerate() {
                if i > 0 {
                    frame = frame.child(gap(m::TIGHT - 2.0));
                }
                frame = frame.child(body(stop(line)));
            }
            frame.child(gap(m::SPACING)).child(buttons().child(button("Close", MoveDo::Close).default(true))).into_element()
        }
    }
}

/// The Forward Port window: a port of another host, and which host, to
/// have here; Cancel and Forward. (It is forwarded, not opened: a port
/// typed here may be a database's.)
pub fn render_forward_panel(
    cx: &WindowCx,
    port_field: &Child<TextField>,
    from: Vec<SharedString>,
    chosen: usize,
    can_forward: bool,
    on_from: impl Fn(&usize, &mut WindowCx) + 'static,
    on_done: impl Fn(&bool, &mut WindowCx) + 'static,
) -> Element {
    let on_done = std::rc::Rc::new(on_done);
    let (cancel, forward) = (on_done.clone(), on_done);
    let divider = divider_for(&["Port:", "From:"]);
    div()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(TextControl::new(cx, port_field).label("Port:").divider(divider).width(divider + 96.0))
        // (The menu to the window's right edge, as Add Host's fields
        // are; a number's field is short.)
        .child(MenuField::new("fwd-host", from, chosen).label("From:").divider(divider).width(olo_kit::widgets::ALERT_WIDTH - 2.0 * m::WINDOW_INSET).on_change(on_from))
        .child(div().pl(px(divider)).child(StringView::new("Rex picks a free port on this computer; the Ports list shows it.").size(style::SMALL_SIZE).dim(true).wrap(true)))
        .child(
            olo_kit::widgets::button_row()
                .pt(px(m::TIGHT))
                .child(Button::new("Cancel").on_click(move |_, cx| cancel(&false, cx)))
                .child(Button::new("Forward").default(true).enabled(can_forward).on_click(move |_, cx| forward(&true, cx))),
        )
}

fn qr_code(url: &str, qr: Arc<qrcode::QrCode>, side: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c([255, 255, 255, 255]));
        style::sunken(g, b, &p, false);

        let w = qr.width();
        let avail = f32::from(b.size.width).min(f32::from(b.size.height)) - 4.0;
        let scale = g.scale_factor();
        let cell = ((avail / (w + 8) as f32) * scale).floor().max(1.0) / scale;
        let total = cell * w as f32;
        let ox = f32::from(b.left()) + ((f32::from(b.size.width) - total) / 2.0 * scale).round() / scale;
        let oy = f32::from(b.top()) + ((f32::from(b.size.height) - total) / 2.0 * scale).round() / scale;
        let dark = style::c([0, 0, 0, 255]);
        for y in 0..w {
            for x in 0..w {
                if qr[(x, y)] == qrcode::Color::Dark {
                    g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x as f32 * cell), px(oy + y as f32 * cell)), size(px(cell), px(cell))), dark));
                }
            }
        }
    })
    .paint_key(style::key(url))
    .w(px(side))
    .h(px(side))
}

/// The side of the code to scan, in points.
const CODE: f32 = 144.0;

/// Remote Access: this computer's Rex server reached over Tailscale,
/// and what that takes at each step, in one group that keeps its place
/// and size whatever the state: a sign-in (the page opened here, or its
/// code scanned), then a device paired (the server's address as a code
/// for the phone, or copied for another computer). It was two places:
/// a section of the Settings window, and a window with the code alone
/// that said to go and look for the other.
pub fn render_access_panel(
    hub: &Arc<RexHub>,
    feedback: Option<Element>,
    code: Option<Arc<qrcode::QrCode>>,
    on_tailscale: impl Fn(&crate::tailscale::Action, &mut WindowCx) + 'static,
    on_copy: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    use crate::tailscale::Action;
    let (status, connected) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let h = st.hosts.get("local");
        (h.and_then(|x| x.tailscale.clone()), h.is_some_and(|x| x.conn_state == HostConnState::Connected))
    };
    let status = status.as_ref().filter(|_| connected);
    let step = Step::of(status);
    let on = status.is_some_and(|s| matches!(s.state.as_str(), "starting" | "needs_login" | "running" | "connected" | "needs_machine_auth" | "stopping"));
    let on_action = std::rc::Rc::new(on_tailscale);
    let toggle = on_action.clone();
    let check = CheckBox::new("Let your other devices reach this computer’s sessions over Tailscale", on)
        .enabled(status.is_some())
        .on_toggle(move |on, cx| toggle(&if *on { Action::Enable } else { Action::Disable }, cx));
    // Under the check, in line with its words: how it stands, or what
    // just went wrong with it. One line's room always, so nothing below
    // moves as it comes and goes.
    let (lit, stands): (Light, String) = match &step {
        // (The address is in the group below: not said twice.)
        Step::Pair(_) => (Light::On, olo_kit::words::tr("Your other devices on Tailscale can reach these sessions.").into()),
        Step::SignIn(_) => (Light::Waiting, olo_kit::words::tr("Waiting for you to sign in…").into()),
        Step::Off if !connected => (Light::Down, crate::tailscale::state_line(status, connected)),
        Step::Off if status.is_some_and(|s| matches!(s.state.as_str(), "starting" | "running" | "connected" | "stopping")) => (Light::Waiting, crate::tailscale::state_line(status, connected)),
        Step::Off => (Light::Off, crate::tailscale::state_line(status, connected)),
    };
    // (What just went wrong takes the line's place, without a light: it has its own colour.)
    let line = match feedback {
        Some(problem) => problem,
        None => div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::MARK_GAP))
            .child(light(lit))
            .child(div().flex_1().min_w_0().child(StringView::new(stands).dim(true).wrap(true)))
            .into_any_element(),
    };
    let stands = div().w_full().pl(px(m::MARK_SIZE + m::MARK_GAP)).min_h(px(olo_kit::widgets::TextView::line_height())).child(line);

    let (title, says, address, button): (&str, &str, Option<String>, Option<Element>) = match &step {
        Step::Off => ("Pair a Device", "Turn on remote access, then scan the code with Rex on your phone.", None, None),
        Step::SignIn(link) => {
            let open = on_action.clone();
            let button = match link {
                Some(page) => {
                    let page = page.clone();
                    Button::new("Open Sign-In Page").on_click(move |_, cx| open(&Action::OpenLink(page.clone()), cx))
                }
                None => Button::new("Sign In to Tailscale…").on_click(move |_, cx| open(&Action::Login, cx)),
            };
            (
                "Sign In to Tailscale",
                "Open the sign-in page on this computer, or scan the code with your phone. After you sign in, this window shows the pairing code.",
                None,
                Some(button.into_any_element()),
            )
        }
        Step::Pair(url) => {
            let copy = url.clone();
            (
                "Pair a Device",
                "Scan the code with Rex on your phone, or use this address in Rex on another computer:",
                Some(url.clone()),
                Some(Button::new("Copy Address").on_click(move |_, cx| on_copy(&copy, cx)).into_any_element()),
            )
        }
    };
    let shown = step.code().map(str::to_owned);
    let well = match (shown, code) {
        (Some(url), Some(code)) => qr_code(&url, code, CODE),
        // No code to show (or it is still being made): its place, with a
        // code's three corner marks faintly, so that it reads as where
        // one will be and not as something that failed to draw.
        _ => canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::shade(&p, p.panel_background, 1.04));
            style::sunken(g, b, &p, false);
            let scale = g.scale_factor();
            // 25 cells a side with 4 of quiet around, each a whole number of device pixels.
            let cell = ((CODE - 4.0) / 33.0 * scale).floor().max(1.0) / scale;
            let (ox, oy) = (f32::from(b.left()) + ((CODE - cell * 25.0) / 2.0 * scale).round() / scale, f32::from(b.top()) + ((CODE - cell * 25.0) / 2.0 * scale).round() / scale);
            let ink = style::c(p.panel_text).opacity(0.16);
            let ground = style::shade(&p, p.panel_background, 1.04);
            let square = |g: &mut olo_kit::Painter, x: f32, y: f32, cells: f32, color| {
                g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x * cell), px(oy + y * cell)), size(px(cells * cell), px(cells * cell))), color))
            };
            for (x, y) in [(0.0, 0.0), (18.0, 0.0), (0.0, 18.0)] {
                square(g, x, y, 7.0, ink);
                square(g, x + 1.0, y + 1.0, 5.0, ground);
                square(g, x + 2.0, y + 2.0, 3.0, ink);
            }
        })
        .paint_key(style::key("no-code"))
        .w(px(CODE))
        .h(px(CODE)),
    };
    // Beside the code, from its top: what to do, the address, the button.
    let beside = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(m::SPACING))
        .child(StringView::new(says).wrap(true))
        // (The address is a value, not more of the sentence: bold, on a
        // line the height of the text's own, so the column keeps one
        // rhythm in every state.)
        .children(address.map(|a| div().w_full().child(StringView::new(a).bold(true).cut(true).height(m::LABEL_ROW))))
        .children(button);
    let group = div().w_full().flex().flex_row().items_start().gap(px(m::SECTION)).child(div().flex_shrink_0().child(well)).child(beside);

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(check).child(stands))
        .child(BBox::new(title).child(group))
        .child(div().w_full().flex().flex_row().justify_end().child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))))
}

/// Where reaching this server from elsewhere stands, as the Remote
/// Access window's group shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Tailscale off, starting, or the server not connected: nothing to
    /// scan yet.
    Off,
    /// The server is to be signed in to Tailscale: its page, once the
    /// server has given the link.
    SignIn(Option<String>),
    /// On the tailnet: the address a device pairs with.
    Pair(String),
}

impl Step {
    pub fn of(status: Option<&olo_rex_client::TailscaleStatus>) -> Step {
        let Some(s) = status else { return Step::Off };
        match s.state.as_str() {
            "needs_login" => Step::SignIn(crate::tailscale::login_link(s).map(str::to_owned)),
            "running" | "connected" => s
                .https
                .as_ref()
                .and_then(|h| h.url.clone())
                .or_else(|| s.dns_name.as_ref().filter(|d| !d.is_empty()).map(|d| format!("https://{}", d.trim_end_matches('.'))))
                .map_or(Step::Off, Step::Pair),
            _ => Step::Off,
        }
    }

    /// What the code to scan says, if there is one to show.
    pub fn code(&self) -> Option<&str> {
        match self {
            Step::SignIn(link) => link.as_deref(),
            Step::Pair(url) => Some(url),
            Step::Off => None,
        }
    }
}

fn build_info(version: &olo_rex_client::ServerVersion) -> String {
    let mut parts = Vec::new();
    if !version.platform.is_empty() {
        parts.push(version.platform.as_str());
    }
    if !version.build_date.is_empty() {
        parts.push(version.build_date.as_str());
    }
    if version.dirty {
        parts.push("modified source");
    }
    parts.join(" · ")
}

/// How the SSH agent stands, in a line (Diagnostics).
fn agent_line(agent: &AgentStatus) -> String {
    if agent.reachable {
        olo_kit::tr!("{} keys, at {}", agent.key_count, agent.socket_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
    } else {
        // (With no socket at all, the client's own words only say the label again.)
        agent.error.clone().filter(|_| agent.socket_path.is_some()).unwrap_or_else(|| olo_kit::words::tr("None found").into())
    }
}

pub fn render_diagnostics_panel(
    hub: &Arc<RexHub>,
    agent: &AgentStatus,
    on_copy_version: impl Fn(&String, &mut WindowCx) + 'static,
    on_copy_debug: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let (blocks_n, res_bytes, img_bytes) = hub.total_vt_memory();
    let (version_info, debug_info, rows): (String, String, Vec<(&'static str, String)>) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let local = st.hosts.get("local");
        let ver = local.and_then(|h| h.server_version.as_ref()).map(|v| format!("{} (commit {}, API rev {})", v.version, v.commit, v.api_revision)).unwrap_or_else(|| "Disconnected".to_owned());
        let status = local.and_then(|h| h.server_status.as_ref());
        let pid_val = status.map(|s| format!("PID {}", s.pid)).unwrap_or_else(|| "—".to_owned());
        let inst_val = status.map(|s| s.instance_id.clone()).unwrap_or_else(|| "—".to_owned());
        let sess_val = status.map(|s| format!("{} on this server", s.session_count)).unwrap_or_else(|| "0 on this server".to_owned());
        let hosts_val = format!("{} configured", st.hosts.len());
        let listeners = status.and_then(|s| s.listeners.as_ref()).map(|ls| ls.iter().map(|l| l.endpoint.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "None".to_owned());
        let vt_val = format!("libghostty-vt ({blocks_n} live panes)");
        let mem_val = format!("{:.1} MiB of resident grid pages", res_bytes as f64 / (1024.0 * 1024.0));
        let img_val = if img_bytes == 0 { "None".to_owned() } else { format!("{:.1} MiB", img_bytes as f64 / (1024.0 * 1024.0)) };
        let mut ver_copy = format!("Olo Rex {}\nRex server {ver}", env!("CARGO_PKG_VERSION"));
        if let Some(version) = local.and_then(|h| h.server_version.as_ref()) {
            let build = build_info(version);
            if !build.is_empty() {
                ver_copy.push('\n');
                ver_copy.push_str(&build);
            }
        }
        let agent_val = agent_line(agent);
        let dbg_copy = format!(
            "{ver_copy}\nProcess: {pid_val}\nInstance: {inst_val}\nSessions: {sess_val}\nHosts: {hosts_val}\nListeners: {listeners}\nSSH agent: {agent_val}\nVT engine: {vt_val}\nMemory: {mem_val}\nKitty images: {img_val}"
        );
        (
            ver_copy,
            dbg_copy,
            vec![
                ("Server:", ver),
                ("Process:", pid_val),
                ("Instance:", inst_val),
                ("Sessions:", sess_val),
                ("Hosts:", hosts_val),
                ("Listeners:", listeners),
                ("SSH agent:", agent_val),
                ("VT engine:", vt_val),
                ("Memory:", mem_val),
                ("Kitty images:", img_val),
            ],
        )
    };

    let label_col_w = (divider_for(&["Server:", "Process:", "Instance:", "Sessions:", "Hosts:", "Listeners:", "SSH agent:", "VT engine:", "Memory:", "Kitty images:"]) - m::LABEL_GAP).max(64.0);
    let mut grid = div().flex().flex_col().gap(px(4.0)).w_full();
    for (label, value) in rows {
        grid = grid.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().w(px(label_col_w)).flex_shrink_0().flex().flex_row().justify_end().child(StringView::new(label).dim(true)))
                .child(div().flex_1().min_w_0().child(StringView::new(value).cut(true))),
        );
    }

    div().flex().flex_col().gap(px(m::WINDOW_INSET)).p(px(m::WINDOW_INSET)).w_full().child(grid).child(
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Copy Version Info").on_click(move |_, cx| on_copy_version(&version_info, cx)))
                    .child(Button::new("Copy Debug Info").on_click(move |_, cx| on_copy_debug(&debug_info, cx))),
            )
            .child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))),
    )
}

#[derive(Debug, Clone)]
pub enum AskpassRequest {
    ConfirmHostKey { host: String, port: u16, fingerprint: String, key_type: String, changed: bool },
    Secret { title: String, prompt: String, echo: bool },
}

#[derive(Debug, Clone)]
pub enum AskpassReply {
    Confirm(bool),
    Secret(Option<String>),
}

pub struct OloAskpass {
    /// One question at a time: whoever asks holds this until answered.
    turn: Mutex<()>,
    pending: Mutex<Option<(AskpassRequest, Option<AskpassReply>)>>,
    cv: Condvar,
    on_request: Arc<dyn Fn() + Send + Sync>,
    /// The hosts whose connection waits for the key agent to sign
    /// (`waiting_for_agent`), and what to do when that changes.
    agent_waits: Mutex<Vec<String>>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for OloAskpass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OloAskpass").finish_non_exhaustive()
    }
}

impl OloAskpass {
    /// `on_request`: a question waits to be shown. `on_change`: which
    /// hosts wait for the key agent changed.
    pub fn new(on_request: impl Fn() + Send + Sync + 'static, on_change: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { turn: Mutex::new(()), pending: Mutex::new(None), cv: Condvar::new(), on_request: Arc::new(on_request), agent_waits: Mutex::new(Vec::new()), on_change: Arc::new(on_change) })
    }

    /// Whether the connection to `host` (a name or an address, as its
    /// endpoint has it) is waiting for the key agent to sign.
    pub fn waits_for_agent(&self, endpoint: &str) -> bool {
        self.agent_waits.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|host| endpoint.contains(host.as_str()))
    }

    pub fn current_request(&self) -> Option<AskpassRequest> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(_, rep)| rep.is_none()).map(|(req, _)| req.clone())
    }

    pub fn answer(&self, reply: AskpassReply) {
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, slot)) = g.as_mut() {
            *slot = Some(reply);
            self.cv.notify_all();
        }
    }

    /// One question, and its own answer. Two hosts connecting at once
    /// each ask about their key: the second waits for the first to be
    /// answered. (There was one slot and no turn: the second question
    /// replaced the first, and the answer to it went to whichever of
    /// the two woke first, so "Trust" said of one host's key could
    /// trust the other's, which the person never saw.)
    fn ask(&self, req: AskpassRequest) -> AskpassReply {
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some((req, None));
        drop(g);
        (self.on_request)();
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((_, Some(rep))) = g.as_ref() {
                let out = rep.clone();
                *g = None;
                return out;
            }
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl SshAskpass for OloAskpass {
    fn confirm_host_key(&self, host: &str, port: u16, fingerprint: &str, key_type: &str, changed: bool) -> bool {
        match self.ask(AskpassRequest::ConfirmHostKey { host: host.to_owned(), port, fingerprint: fingerprint.to_owned(), key_type: key_type.to_owned(), changed }) {
            AskpassReply::Confirm(ok) => ok,
            _ => false,
        }
    }

    fn prompt_secret(&self, title: &str, prompt: &str, echo: bool) -> Option<Zeroizing<String>> {
        match self.ask(AskpassRequest::Secret { title: title.to_owned(), prompt: prompt.to_owned(), echo }) {
            AskpassReply::Secret(Some(s)) => Some(Zeroizing::new(s)),
            _ => None,
        }
    }

    fn waiting_for_agent(&self, host: &str, waiting: bool) {
        {
            let mut waits = self.agent_waits.lock().unwrap_or_else(|e| e.into_inner());
            waits.retain(|h| h != host);
            if waiting {
                waits.push(host.to_owned());
            }
        }
        (self.on_change)();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_hosts_row_says_how_it_stands() {
        use super::{host_state, list_rows};
        use crate::hub::HostConnState;
        assert_eq!(host_state(&HostConnState::Connected, false), "Connected");
        assert_eq!(host_state(&HostConnState::Connecting, false), "Connecting…");
        // Waiting on the person's key agent is said, not "Connecting…" for ever.
        assert_eq!(host_state(&HostConnState::Connecting, true), "Waiting for your key agent…");
        assert_eq!(host_state(&HostConnState::Reconnecting { attempt: 3, last_error: "x".into() }, false), "Reconnecting (try 3)…");
        // An error is a state in a word: its own words are the row's trouble.
        assert_eq!(host_state(&HostConnState::Error("No route to host".into()), false), "Failed");
        // An address without the scheme every one would begin with.
        assert_eq!(super::address_said("ssh://you@build.example:2222"), "you@build.example:2222");
        assert_eq!(super::address_said("nas.ts.net"), "nas.ts.net");
        assert_eq!(super::address_said("unix:///run/rex.sock"), "unix:///run/rex.sock");
        // All of a few hosts, never fewer rows than its least, eight at most.
        assert_eq!((list_rows(1, 5), list_rows(6, 5), list_rows(20, 5), list_rows(0, 4)), (5, 6, 8, 4));
    }

    #[test]
    fn the_one_button_is_connect_or_disconnect_never_both() {
        use super::{connects, disconnects};
        use crate::hub::HostConnState;
        let all = [
            HostConnState::Connected,
            HostConnState::Connecting,
            HostConnState::Disconnected,
            HostConnState::Error("no route".into()),
            HostConnState::Reconnecting { attempt: 2, last_error: "closed".into() },
        ];
        for conn in &all {
            assert!(connects(conn) != disconnects(conn), "{conn:?}: one of the two");
        }
        assert!(disconnects(&HostConnState::Connected) && disconnects(&HostConnState::Connecting));
        assert!(connects(&HostConnState::Disconnected) && connects(&all[4]));
    }

    #[test]
    fn the_line_under_the_hosts_says_something_of_every_host_but_one_that_failed() {
        use super::host_line;
        use crate::hub::HostConnState;
        let far = |conn: &HostConnState, sessions, forwards| host_line(conn, false, false, sessions, forwards);
        // (The row's own word first, then its sessions and its ports.)
        assert_eq!(far(&HostConnState::Connected, 1, 0), "Connected. 1 session there.");
        assert_eq!(far(&HostConnState::Connected, 3, 2), "Connected. 3 sessions there. 2 ports forwarded here.");
        assert_eq!(far(&HostConnState::Connecting, 0, 0), "Connecting…");
        assert_eq!(far(&HostConnState::Connecting, 0, 1), "Connecting… 1 forwarded port is waiting.");
        assert_eq!(host_line(&HostConnState::Connecting, false, true, 0, 0), "Connecting… Your key agent is asking you about this sign-in.");
        assert_eq!(far(&HostConnState::Reconnecting { attempt: 2, last_error: "connection closed".into() }, 0, 1), "Reconnecting. Connection closed. 1 forwarded port is waiting.");
        assert_eq!(far(&HostConnState::Disconnected, 0, 2), "Disconnected. Sessions there keep running. 2 forwarded ports are waiting.");
        assert_eq!(far(&HostConnState::Disconnected, 0, 0), "Disconnected. Sessions there keep running.");
        // (This computer's own, however it stands; a failure's own words are said in the line's place.)
        assert_eq!(host_line(&HostConnState::Connected, true, false, 2, 0), "Rex’s own server. It can’t be disconnected or removed.");
        assert_eq!(far(&HostConnState::Error("no route".into()), 0, 0), "");
    }

    #[test]
    fn a_forward_of_a_host_let_go_of_waits_and_is_not_said_to_reconnect() {
        use super::{Light, forward_said};
        use crate::hub::{ForwardState, HostConnState};
        assert_eq!(forward_said(&ForwardState::Waiting, Some(&HostConnState::Disconnected)), (Light::Off, "Waiting for host", None));
        assert_eq!(forward_said(&ForwardState::Waiting, Some(&HostConnState::Error("no route".into()))), (Light::Off, "Waiting for host", None));
        // (Its host's word and light: amber with a host being connected to.)
        assert_eq!(forward_said(&ForwardState::Waiting, Some(&HostConnState::Connecting)), (Light::Waiting, "Waiting for host", None));
        let lost = HostConnState::Reconnecting { attempt: 1, last_error: "closed".into() };
        assert_eq!(forward_said(&ForwardState::Waiting, Some(&lost)), (Light::Waiting, "Reconnecting…", None));
        assert_eq!(forward_said(&ForwardState::Waiting, Some(&HostConnState::Connected)).1, "Reconnecting…");
        assert_eq!(forward_said(&ForwardState::Active, Some(&HostConnState::Connected)), (Light::On, "Forwarded", None));
    }

    #[test]
    fn red_is_for_what_went_wrong() {
        use super::{Light, light_for};
        use crate::hub::HostConnState;
        // A host never connected to, or disconnected from, is off: the
        // red light is an error's alone.
        assert_eq!(light_for(&HostConnState::Connected), Light::On);
        assert_eq!(light_for(&HostConnState::Connecting), Light::Waiting);
        assert_eq!(light_for(&HostConnState::Disconnected), Light::Off);
        assert_eq!(light_for(&HostConnState::Error("no route".into())), Light::Down);
    }

    #[test]
    fn diagnostics_use_the_real_build_fields() {
        let v = serde_json::from_value(serde_json::json!({"rex_version":"1.2","platform":"linux_amd64","built_at":"fixture-date","dirty":true})).unwrap();
        assert_eq!(super::build_info(&v), "linux_amd64 · fixture-date · modified source");
    }

    use olo_rex_client::{TailscaleHttps, TailscaleStatus};

    use super::Step;

    /// Two hosts ask about their keys at once: each question is shown
    /// in turn and gets the answer given to it, never the other's.
    #[test]
    fn each_question_gets_its_own_answer() {
        use super::{AskpassReply, AskpassRequest, OloAskpass};
        use olo_rex_client::SshAskpass as _;
        let ui = OloAskpass::new(|| {}, || {});
        let ask = |host: &'static str| {
            let ui = ui.clone();
            std::thread::spawn(move || ui.confirm_host_key(host, 22, "SHA256:fixture", "ssh-ed25519", false))
        };
        let (a, b) = (ask("trusted.example"), ask("refused.example"));
        let mut answered = Vec::new();
        for _ in 0..2 {
            // Wait for a question to be shown, then answer by what it asks.
            let host = loop {
                match ui.current_request() {
                    Some(AskpassRequest::ConfirmHostKey { host, .. }) if !answered.contains(&host) => break host,
                    _ => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            };
            ui.answer(AskpassReply::Confirm(host == "trusted.example"));
            answered.push(host);
        }
        assert!(a.join().unwrap(), "the host the person trusted");
        assert!(!b.join().unwrap(), "the host the person refused is not trusted by the other's answer");
        // The key agent's waits, by host.
        ui.waiting_for_agent("build.example", true);
        assert!(ui.waits_for_agent("ssh://dev@build.example") && !ui.waits_for_agent("ssh://dev@other.example"));
        ui.waiting_for_agent("build.example", false);
        assert!(!ui.waits_for_agent("ssh://dev@build.example"));
    }

    /// What the Remote Access window offers at each state of the
    /// server's Tailscale: nothing to scan while it is off, starting or
    /// unknown; the sign-in page once the server has given its link
    /// (and a way to ask for it before); the address to pair with once
    /// it is on, the HTTPS one where the server has it.
    #[test]
    fn the_step_follows_the_servers_tailscale() {
        let state = |state: &str| TailscaleStatus { state: state.into(), ..Default::default() };
        assert_eq!(Step::of(None), Step::Off);
        for off in ["disabled", "stopped", "starting", "stopping", "something-new"] {
            assert_eq!(Step::of(Some(&state(off))), Step::Off, "{off}");
        }
        assert_eq!(Step::of(Some(&state("needs_login"))), Step::SignIn(None));
        let asked = TailscaleStatus { auth_url: Some("https://login.example/a/1".into()), ..state("needs_login") };
        assert_eq!(Step::of(Some(&asked)), Step::SignIn(Some("https://login.example/a/1".into())));
        assert_eq!(Step::of(Some(&asked)).code(), Some("https://login.example/a/1"));
        // (A link that isn't one is not offered, nor drawn as a code.)
        let odd = TailscaleStatus { auth_url: Some("javascript:alert(1)".into()), ..state("needs_login") };
        assert_eq!(Step::of(Some(&odd)), Step::SignIn(None));
        let named = TailscaleStatus { dns_name: Some("studio.example.ts.net.".into()), ..state("running") };
        assert_eq!(Step::of(Some(&named)), Step::Pair("https://studio.example.ts.net".into()));
        let served = TailscaleStatus { https: Some(TailscaleHttps { url: Some("https://studio.example.ts.net:8443".into()), ..Default::default() }), ..named.clone() };
        assert_eq!(Step::of(Some(&served)).code(), Some("https://studio.example.ts.net:8443"));
        // On, but the server hasn't said where yet: nothing to scan.
        assert_eq!(Step::of(Some(&state("running"))), Step::Off);
        assert_eq!(Step::Off.code(), None);
    }
}

#[cfg(test)]
mod command_tests {
    use super::command_lines;

    /// A command is set on lines as one is written out by hand: broken
    /// after a pipe or an `&&`, a long part at a space with a backslash,
    /// what follows the first line set in; a short one is left alone.
    #[test]
    fn a_command_is_broken_where_one_would_break_it() {
        assert_eq!(command_lines("rm -f /usr/local/bin/rex", 60), ["rm -f /usr/local/bin/rex"]);
        assert_eq!(
            command_lines("curl -fsSL https://pkgs.superlogical.com/rex/install.sh | sh -s -- --channel snapshot", 60),
            ["curl -fsSL https://pkgs.superlogical.com/rex/install.sh |", "  sh -s -- --channel snapshot"]
        );
        assert_eq!(
            command_lines("systemctl disable --now rex-login-server.socket rex-login-server.service && rm -rf /run/rex", 60),
            ["systemctl disable --now rex-login-server.socket \\", "  rex-login-server.service &&", "  rm -rf /run/rex"]
        );
        // No line is longer than the room, where there is a space to break at; put back together it is the command.
        for most in [30, 44, 60, 90] {
            for command in [
                "curl -fsSL https://pkgs.superlogical.com/rex/install.sh | sh -s -- --channel snapshot",
                "systemctl disable --now rex-login-server.socket rex-login-server.service && rm -rf /run/rex",
                "usermod --append --groups rex alice",
                "a | b && c | d",
            ] {
                let lines = command_lines(command, most);
                assert!(lines.iter().all(|l| l.chars().count() <= most || !l.trim().trim_end_matches(" \\").contains(' ')), "{most}: {lines:?}");
                let joined = lines.iter().map(|l| l.trim().trim_end_matches('\\').trim_end()).collect::<Vec<_>>().join(" ");
                assert_eq!(joined, command, "{most}");
                assert!(lines.iter().skip(1).all(|l| l.starts_with("  ")) && !lines[0].starts_with(' '));
            }
        }
    }
}
