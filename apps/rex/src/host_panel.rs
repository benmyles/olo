//! The Host window's drawing: everything of Olo's on a far host, in
//! one place (`host` is which host and what is being done to it; the
//! hub knows what the host has, `HubState::host_stands`).
//!
//! At its head, the host: its icon, its name, where it is and how its
//! connection stands, and Check Again (something may have been changed
//! there by hand). Then one section for each thing, as the Server
//! window has one for each service, in the order one rests on another:
//! the host's Rex server (which has a window of its own for the rest:
//! Manage…), Olo's bridge, the skill for agents, Olo's typing helper;
//! and, in a tab of its own, Agent Screens of the host's own (Olo
//! itself put there, and where an agent's screen opens when it doesn't
//! say: `host_screens`).
//! A section is its heading and rule; a line that says how the thing
//! stands (a light, a word or two, and what more there is to say of it
//! dimmer) with what can be done to it at the line's end; and under
//! those, set in under the words, what the thing is for and where it
//! is on the host, in a note's small dim words.
//!
//! One set of words for every thing: Installed, Not installed, Being
//! installed…, Being removed…, Couldn’t install, Couldn’t remove; and
//! Install and Remove. What is done is done at the click, over SSH,
//! and is as easily undone: only Remove All… asks first. A thing that
//! couldn't be done has the red light, its button is Try Again, and
//! what the host said stands under its line in a well, as the host
//! said it. (The first design reviews, of a first build: the failure
//! was a line in the warning's colour under the note beside a grey
//! light, the states were of three grammars with the host's name in
//! each, the notes ran under the buttons, nothing said which host.)

use std::rc::Rc;

use olo_kit::{
    Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, WindowCx, canvas, div, icons, metrics as m, px, style,
    widgets::{Button, RadioButton, StringView, TabView, button_width, separator},
};

use crate::host::{ScreenDo, Shown};
use crate::hub::{HostConnState, HostHelper, HostOp, HostStands, HostThing};
use crate::panels::{self, Light};

/// The window's width: Hosts and Ports', which it is opened from.
pub const HOST_WIDTH: f32 = 600.0;

/// The host's icon at the window's head.
const ICON: f32 = 32.0;

/// The host's name there: three points over a section's heading, in a
/// line of its own height; and the line under it. The two are a block
/// a little taller than the icon and centred on it (as tall as it, a
/// name's descenders stood on the state under them: "staging" over
/// "Failed").
const NAME_SIZE: f32 = style::PLAIN_SIZE + 3.0;
const NAME_ROW: f32 = 20.0;
const STATE_ROW: f32 = 14.0;

/// What a host said in a well, and where a thing is on it: one size of
/// the fixed-width face in the window, a point under a note's.
const MONO_SIZE: f32 = style::SMALL_SIZE - 1.0;
/// The widest the kept folders' names' column is: a longer name is cut
/// short (its whole is in its path, under it), and the places beside the
/// labels keep their room.
const NAME_MOST: f32 = 120.0;

thread_local! {
    /// The Files tab's list's scroller (one Host window at a time).
    static FILES_SCROLL: olo_kit::ScrollHandle = olo_kit::ScrollHandle::new();
}

/// How many of a host's screens the Screens tab's list has room for:
/// never fewer (a list of one row reads as a field), and past them it
/// scrolls; the window keeps its height.
const SCREENS_SHOWN: usize = 3;

/// One of a host's screens as the Screens tab's list has it: what the
/// host said of it, and what is being done to it from here.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenLine {
    pub row: crate::host_screens::ScreenRow,
    pub doing: Option<ScreenDo>,
}

/// The list's rows: the host's screens as last heard (running first),
/// each with what this window is doing to it.
pub fn screen_lines(host: &HostStands, shown: &Shown) -> Vec<ScreenLine> {
    host.screens.as_ref().filter(|s| s.installed.is_some()).map_or(Vec::new(), |s| s.rows.iter().map(|row| ScreenLine { row: row.clone(), doing: shown.screen_doing(&row.name) }).collect())
}

/// "Screens on this host": the host's screens as a list in a well
/// (its name, the title it was given, when it was started, how it
/// stands with its light), Look at Screen and Stop Screen under it for
/// the one chosen (a running one; opening a row looks at it), and
/// under them one line's room for what couldn't be done last (kept
/// when nothing failed: the window doesn't move when something does).
/// There in every state, empty where Olo isn't on the host yet: a
/// finished install fills a list in, it doesn't add a section, and the
/// tab isn't half empty on a new host. Opus 5.5's look.
pub fn screens_section(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    list: &olo_kit::widgets::ListModel,
    lines: &[ScreenLine],
    host: &HostStands,
    shown: &Shown,
    can_live_stream: bool,
    on_do: impl Fn(&HostDo, &mut WindowCx) + 'static,
) -> Option<Element> {
    use olo_kit::widgets::{Column, ColumnListView};
    // (An empty list says how screens come to be, where they can: not
    // where Olo isn't there yet, nor where none can run.)
    let can_run = host.screens.as_ref().is_some_and(|s| s.installed.is_some() && s.lacking().is_none());
    let none = if can_run { "No screens yet. Agents on the host start them." } else { "No screens." };
    let on_do: Rc<dyn Fn(&HostDo, &mut WindowCx)> = Rc::new(on_do);
    let reached = host.conn == HostConnState::Connected && host.ssh;
    // (The last column fills what is left of the rows' width: 494
    // points beside the scroll bar, as the row's own node says it,
    // read on the rig. The four declared come to 492: more
    // than its room and the list scrolls sideways, its bar over the
    // third row, which the lint doesn't see; 496 did, at every scale.
    // The first column's width keeps the second's words well clear of
    // the second button's under the list (34 points): at 132 they
    // stood 2 points apart, which read as a column that missed. The
    // lint's, at 1.5×.)
    // (The title is the one column of prose, and the widest: it was
    // the narrowest but one, "Preview of the docs" 8 points from the
    // time after it beside a column of short names half empty. Gemini 4
    // Neon's look.)
    // (Opus 5.5 measured the same: a name is 50 points, a time 52, and
    // Title "gets the rest … and any width added later"; a screen
    // started before today says its day, "Yesterday", "Nov 24", which
    // fits where its time did.)
    // (Status is wider than its words need, so its heading stands
    // clear of where the count over it begins, "3 screens  ·  2
    // running" at the heading's right: 4 points apart read as an edge
    // that missed. The lint's, at 1.5×.)
    // (And a name's column is 100 points, not 110: at 110 "Title"
    // began where an empty list's centred words do, to a point, at
    // four scales. The lint's.)
    let columns = vec![Column::new("Screen", 100.0), Column::new("Title", 208.0), Column::new("Started", 80.0), Column::new("Status", 104.0)];
    let opened = {
        let (on_do, lines) = (on_do.clone(), lines.to_vec());
        let host_key = shown.key.clone();
        let host_instance_id = shown.host_instance_id;
        let host_label = shown.name.clone();
        move |row: &usize, cx: &mut WindowCx| {
            if can_live_stream && let Some(line) = lines.get(*row).filter(|l| l.row.running && l.doing.is_none()) {
                on_do(&HostDo::ScreenLook { host_key: host_key.clone(), host_instance_id, host_label: host_label.clone(), name: line.row.name.clone(), expected_started: line.row.started }, cx);
            }
        }
    };
    let view = ColumnListView::new(cx, "rex-host-screens", list, columns, lines.len(), |v: &crate::ui::RexWindow, cell, g| {
        let Some(line) = v.host_screen_lines.get(cell.row) else { return };
        match cell.column {
            0 => cell.text(g, line.row.name.clone()),
            1 => cell.text(g, line.row.title.clone().unwrap_or_default()),
            2 => cell.text(g, line.row.started_said()),
            _ => match (line.doing, line.row.running) {
                (Some(ScreenDo::Stop), _) => panels::ring_cell(g, cell, &olo_kit::words::tr("Stopping…")),
                (_, true) => panels::lit_cell(g, cell, Light::On, &olo_kit::words::tr(line.row.said())),
                (_, false) => panels::lit_cell(g, cell, Light::Off, &olo_kit::words::tr(line.row.said())),
            },
        }
    })
    .row_key(|v: &crate::ui::RexWindow, row| {
        v.host_screen_lines.get(row).map_or(0, |l| style::key((&l.row.name, &l.row.title, l.row.size, l.row.running, l.row.archived, l.row.started, l.doing.map(|d| d as u8))))
    })
    .empty_text(none)
    .on_open(opened)
    .on_select(cx.msg_with(|_: &()| crate::ui::WinMsg::HostChosen));
    let tall = olo_kit::widgets::list_height(lines.len().clamp(SCREENS_SHOWN, SCREENS_SHOWN), m::LIST_ROW, true);
    let view = div().w_full().h(px(tall)).child(view);

    // (The one chosen, where it runs and nothing is being done to it.)
    let chosen = list.state().single().and_then(|i| lines.get(i));
    let free = chosen.filter(|l| l.row.running && l.doing.is_none() && reached);
    let stop_action = free.map(|l| HostDo::ScreenStop { host_key: shown.key.clone(), host_instance_id: shown.host_instance_id, name: l.row.name.clone(), expected_started: l.row.started });
    let look_action = free.filter(|_| can_live_stream).map(|l| HostDo::ScreenLook {
        host_key: shown.key.clone(),
        host_instance_id: shown.host_instance_id,
        host_label: shown.name.clone(),
        name: l.row.name.clone(),
        expected_started: l.row.started,
    });
    let look_tip = if !can_live_stream && reached { "Update Olo’s helper on this host to view screens live in Rex." } else { "Opens the screen live in Rex." };
    let wide = ["Look at Screen", "Stop Screen"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
    let button = |label: &'static str, id: &'static str, tip: &'static str, what: Option<HostDo>| {
        let on_do = on_do.clone();
        Button::new(label).id(id).min_width(wide).tip(tip).enabled(what.is_some()).on_click(move |_, cx| {
            if let Some(what) = &what {
                on_do(what, cx);
            }
        })
    };
    // (How many there are and how many run: "6 screens  ·  3 running",
    // since three rows show of six and the count alone said "3". A row
    // being stopped isn't one that runs: the count is the green lights'.)
    let running = lines.iter().filter(|l| l.row.running && l.doing != Some(ScreenDo::Stop)).count();
    let all = olo_kit::words::count(lines.len() as u64, "1 screen", "{n} screens").to_string();
    let count: String = match (lines.len(), running) {
        (0, _) => String::new(),
        (_, 0) => all,
        (_, n) => format!("{all}  ·  {}", olo_kit::words::count(n as u64, "1 running", "{n} running")),
    };
    let actions = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(button("Look at Screen", "host-screen-look", look_tip, look_action))
        .child(button("Stop Screen", "host-screen-stop", "Quits everything running on the screen. The screen stays in the list, stopped.", stop_action))
        .child(div().flex_1());
    // Under them, what couldn't be done last: one line with the down
    // light, its first half ("Couldn’t stop agent-2") in the ink the
    // section above gives "Couldn’t remove", the reason in the words'
    // own. Its room is there when nothing failed too, so the window and
    // the buttons at its foot don't move by a few points when something
    // does (they did, by 4 to 5: Opus 5.5's look).
    let row = style::line_row(style::PLAIN_SIZE);
    let failed: Element = match &shown.screen_failed {
        Some((_, what, why)) => {
            let p = olo_kit::theme::palette();
            let err_red = style::c(olo_theme::readable(p.failure, p.panel_background, 5.5));
            div()
                .w_full()
                .min_h(px(row))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(m::TIGHT))
                .child(div().h(px(row)).flex().items_center().child(panels::light(Light::Down)))
                // (On the line's own height: at its default, a control's,
                // the line stood 7 points taller than the room kept for it.)
                .child(StringView::new(format!("{what}:")).color(err_red).height(row))
                .child(div().flex_1().min_w_0().child(StringView::new(why.clone()).wrap(true)))
                .into_element()
        }
        None => div().w_full().h(px(row)).flex_shrink_0().into_element(),
    };
    // (The section's heading as the others': its words, a rule; how
    // many run at its right, in the heading's own row.)
    let head = div()
        .w_full()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .child(div().flex_1().min_w_0().child(StringView::new("Screens on this host").bold(true).height(m::LABEL_ROW)))
                .child(StringView::new(count).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW)),
        )
        .child(div().h(px(2.0)).flex_shrink_0())
        .child(separator());
    let section = div()
        .w_full()
        .flex()
        .flex_col()
        .child(head)
        .child(div().h(px(m::SPACING - 2.0)).flex_shrink_0())
        .child(view)
        .child(div().h(px(m::SPACING)).flex_shrink_0())
        .child(actions)
        .child(div().h(px(m::TIGHT)).flex_shrink_0())
        .child(failed);
    Some(section.into_element())
}

/// How far under the head its note begins: so that the state's words
/// are as far over the note's as a section's line is over its note
/// (twenty-seven points, as the pictures measure; four and a half
/// less, the note crowded the icon).
const HEAD_NOTE_UNDER: f32 = m::TIGHT + 2.0 + 4.5;

/// How far under a state's line the well with the host's words begins.
const WELL_OVER: f32 = 10.0;

/// What a button of the window asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum HostDo {
    /// `thing` installed, updated, or removed on the host.
    Change(HostThing, HostOp),
    /// Every outdated Olo component on the host updated at once.
    UpdateAll(Vec<HostThing>),
    /// The host asked again what it has.
    Check,
    /// A host that isn't connected, connected to (Check Again, there).
    Connect,
    /// The host's Rex server, in its own window.
    Server,
    /// The host's Rex server window opened directly to Update.
    UpdateServer,
    /// What the bridge may do here: Preferences ▸ Rex.
    Permissions,
    /// The Token Pool's Hosts window, where a host's pool is managed.
    OpenPool,
    /// The Account window: the host's account made ready for work.
    Account,
    /// The Move window: the account moved to another host.
    Move,
    /// Remove All… pressed (`true`), or its question answered no.
    AskAll(bool),
    /// Its question answered yes: these taken off.
    RemoveAll(Vec<HostThing>),
    /// A tab chosen: Rex, Olo Integration, Files, Screens, Tweaks.
    Tab(usize),
    /// On Tweaks: what the Server window's buttons ask, of the Host
    /// window's own record of the host (`server::for_tweaks`): a tweak
    /// put to the person, agreed to, left; the host looked at again.
    Tweak(crate::server_panel::ServerDo),
    /// On Tweaks: a key chosen for root in the menu.
    TweakKey(usize),
    /// On Tweaks, in brush's question: the accounts there are too.
    TweakEveryone(bool),
    /// Where an agent's screen opens there when it doesn't say: on the
    /// host itself (`true`), or on this computer.
    ScreensDefault(bool),
    /// Olo being put on the host for its screens, stopped where it is.
    ScreensCancel,
    /// One of the host's screens stopped, carrying the displayed host identity
    /// and observed `started` generation so a queued click can never retarget
    /// another host or stop a replacement run.
    ScreenStop { host_key: String, host_instance_id: u64, name: String, expected_started: Option<u64> },
    /// Look at one of the host's screens live in Rex, carrying the rendered
    /// `(host_key, host_instance_id, expected_started)` so queued clicks never
    /// retarget another host or a replacement session (Astra #4).
    ScreenLook { host_key: String, host_instance_id: u64, host_label: String, name: String, expected_started: Option<u64> },
    /// A kept folder (by its path there) let go of: `true` asks to confirm
    /// first (`Forget…`), `false` forgets immediately (gone folders).
    StopKept(String, bool),
    /// A kept folder synced immediately on demand.
    SyncKept(String),
    /// A kept folder that failed, kept in step again from the start.
    RetryKept(String),
    /// A kept folder's copy here shown in Tracker (by its path there); with
    /// none, the host's own folder there (Downloads ▸ Rex ▸ NAME).
    ShowKept(Option<String>),
    /// These words put on the clipboard (the command that keeps a folder).
    Copy(String),
}

/// How a thing stands, on its line: its light, a word or two, and what
/// more there is to say of it (set dimmer, after a dot).
#[derive(Clone, Debug, PartialEq)]
pub struct Stands {
    pub light: Light,
    pub said: String,
    pub more: Option<String>,
    /// What the host said, where it couldn't do what was asked.
    pub host_said: Option<String>,
    /// Olo's own word on what isn't right, in the window's face under
    /// the line (not the host's output, which is set in its well).
    pub why: Option<String>,
    /// Which, in a sentence under the line, dim (the SSH keys a host
    /// was offered, by name): what the line counts, where a line that
    /// named them was cut short of the rest.
    pub which: Option<String>,
    /// It is being done now (its light a ring, not a light at rest).
    pub busy: bool,
    /// Whether it is on the host, to be taken off.
    pub there: bool,
    /// Whether an installed component is behind this Olo and can be
    /// updated from here.
    pub behind: bool,
    /// What its button does, if it can be pressed now: install, update,
    /// or remove, and whether that is a second try (`Try Again`).
    pub can: Option<(HostOp, bool)>,
    /// What it is stays said under `why`, red light or not (a host's
    /// own screens: one structure in every state, what is wrong in one
    /// place over a description that never goes; a reviewer's, of a
    /// section that jumped as its lines were swapped in and out).
    pub keeps_note: bool,
}

/// How a kept folder stands, on its line: in step (and how long ago it
/// was last brought in step), syncing, waiting for its host, gone there,
/// or failed and why.
pub fn kept_stands(k: &crate::hub::Kept) -> Stands {
    use crate::hub::KeptState;
    // (Said again as it changes: `olo_kit::ago`.)
    let ago = k.last.map(|(at, _, _)| olo_kit::ago::ago(at, olo_kit::ago::long));
    match &k.state {
        // One amber, one meaning (Opus's review): amber is ONLY for conflicts
        // needing a look; green dot is in step; green ring is syncing; grey dot
        // is waiting or gone; red dot is failed.
        KeptState::InStep if !k.conflicts.is_empty() => {
            let conf = olo_kit::words::count(k.conflicts.len() as u64, "1 conflict copy", "{n} conflict copies").to_string();
            let more = match ago {
                Some(a) => format!("{conf}  ·  {a}"),
                None => conf,
            };
            Stands { more: Some(more), ..stands(Light::Waiting, "In step") }
        }
        KeptState::InStep => Stands { more: ago, ..stands(Light::On, "In step") },
        KeptState::Syncing => Stands { busy: true, ..stands(Light::On, "Syncing…") },
        KeptState::Waiting => stands(Light::Off, "Waiting for the host"),
        KeptState::Gone => stands(Light::Off, "Gone from the host"),
        KeptState::Failed(why) => {
            let more = match ago {
                Some(a) => format!("{why}  ·  {a}"),
                None => why.clone(),
            };
            Stands { more: Some(more), ..stands(Light::Down, "Failed") }
        }
    }
}

/// Primary direction phrase naming the host (Opus's review: primary ink so
/// direction stands out from modifiers), plus dim modifier words and monospace
/// exclude patterns.
pub fn kept_mode_parts(k: &crate::hub::Kept, host_name: &str) -> (String, Vec<String>, Vec<String>) {
    use olo_protocol::RexSyncDirection as Dir;
    let dir = match k.direction {
        Dir::FromHost => olo_kit::tr!("From {}", host_name).to_string(),
        Dir::ToHost => olo_kit::tr!("To {}", host_name).to_string(),
        Dir::Both => olo_kit::words::tr("Both ways").to_string(),
    };
    let mut mods = Vec::new();
    if k.mirror && k.direction != Dir::Both {
        mods.push(olo_kit::words::tr("mirrored").to_string());
    }
    if k.shallow {
        mods.push(olo_kit::words::tr("top level only").to_string());
    }
    (dir, mods, k.exclude.clone())
}

fn stands(light: Light, said: &'static str) -> Stands {
    Stands { light, said: olo_kit::words::tr(said).into(), more: None, host_said: None, why: None, which: None, busy: false, there: false, behind: false, can: None, keeps_note: false }
}

/// How `thing` stands on `host`, `reached` whether anything can be
/// asked of it now: what is being done to it first, then what couldn't
/// be, then what the host last said it has.
pub fn thing_stands(host: &HostStands, shown: &Shown, thing: HostThing) -> Stands {
    let reached = host.conn == HostConnState::Connected && host.ssh;
    let more = |s: Stands, more: &'static str| Stands { more: Some(olo_kit::words::tr(more).into()), ..s };
    // (Nothing of the bridge is on the host: the app offers its actions
    // through the host's Rex server, to whatever the host is granted.
    // On or off is the permissions', known whether it is reached or not.)
    if thing == HostThing::Bridge {
        if host.granted.is_empty() {
            return more(stands(Light::Off, "Off"), "nothing granted");
        }
        // (What is granted, by name: the one thing asked of this line.
        // Its light is on while the host is connected: granted to a
        // host that isn't reached, nothing is being used.)
        // The SSH keys by how many the host was offered, once it has
        // asked; and where it was offered none, why, under the line, its
        // light amber: what is granted isn't all there. (Said "SSH
        // keys" whatever came of it, a host offered none looked as one
        // with them: 2026-11-23, Rex on an agent screen.)
        // (None offered: "no SSH keys", as "2 SSH keys" says how many;
        // left "SSH keys", the line said they were there while the
        // sentence under it said they weren't: Opus's look.)
        // (And which, by their items' names, in a sentence under it, so
        // a key a host was refused for is seen missing: in the line, "2
        // SSH keys (GitHub, Work laptop)" was cut at the third name, and
        // what was granted after the keys went with it.)
        let keys = |name: &&'static str| match (*name, &host.agent_keys) {
            ("SSH keys", Some(Err(_))) => olo_kit::words::tr("no SSH keys").to_string(),
            ("SSH keys", Some(Ok(names))) if names.is_empty() => olo_kit::words::tr("no SSH keys").to_string(),
            ("SSH keys", Some(Ok(names))) => olo_kit::words::count(names.len() as u64, "1 SSH key", "{n} SSH keys").to_string(),
            _ => olo_kit::words::tr(name).to_string(),
        };
        let granted = host.granted.iter().map(keys).collect::<Vec<_>>().join(", ");
        let connected = host.conn == HostConnState::Connected;
        // (Under it, the reason alone, as a sentence.)
        let none = match &host.agent_keys {
            // (On an agent screen its Passwords has only what was shared
            // with the screen: said so, or it reads as the person's own
            // having none. 2026-11-23, Rex moved to an agent screen.)
            Some(Ok(names)) if names.is_empty() && olo_xdg::screen().is_some() => Some(olo_kit::words::tr("Passwords on this screen has no SSH keys shared with it.").to_string()),
            Some(Ok(names)) if names.is_empty() => Some(olo_kit::words::tr("Passwords has no SSH keys to offer.").to_string()),
            Some(Err(why)) => Some(format!("{}.", why.trim_end_matches('.'))),
            _ => None,
        }
        .filter(|_| connected && host.granted.contains(&"SSH keys"));
        let light = match (connected, &none) {
            (false, _) => Light::Off,
            (true, Some(_)) => Light::Waiting,
            (true, None) => Light::On,
        };
        let which = match &host.agent_keys {
            Some(Ok(names)) if !names.is_empty() => Some(olo_kit::tr!("SSH keys offered: {}.", names.join(", ")).to_string()),
            _ => None,
        }
        .filter(|_| connected && host.granted.contains(&"SSH keys"));
        return Stands { more: Some(granted), why: none, which, ..stands(light, "On") };
    }
    // (Olo being put on a host for its screens says how far it is, and
    // is said by the hub, which does it: a window closed and opened
    // again meanwhile shows it still.)
    if thing == HostThing::Screens && host.screens_doing.is_some() && shown.doing(thing) != Some(HostOp::Remove) {
        let word = if shown.doing(thing) == Some(HostOp::Update) || host.screens.as_ref().is_some_and(|s| s.installed.is_some()) { "Being updated…" } else { "Being installed…" };
        return Stands { busy: true, more: host.screens_doing.as_ref().map(|(words, _)| words.clone()), keeps_note: true, ..stands(Light::Waiting, word) };
    }
    if let Some(op) = shown.doing(thing) {
        let word = match op {
            HostOp::Install => "Being installed…",
            HostOp::Update => "Being updated…",
            HostOp::Remove => "Being removed…",
        };
        return Stands { busy: true, ..stands(Light::Waiting, word) };
    }
    if !reached {
        return stands(Light::Off, "Not known");
    }
    if let Some((op, said)) = shown.failed(thing) {
        let word = match op {
            HostOp::Install => "Couldn’t install",
            HostOp::Update => "Couldn’t update",
            HostOp::Remove => "Couldn’t remove",
        };
        let there = matches!(op, HostOp::Update | HostOp::Remove) && thing != HostThing::Pool && thing != HostThing::Screens;
        // (What the host's helper says of Olo there is a sentence of
        // Olo's own, set as one: not a command's last words in a well.)
        if thing == HostThing::Screens {
            let said = said.trim();
            let sentence = if said.ends_with(['.', '!', '?', '…']) { said.to_owned() } else { format!("{said}.") };
            return Stands { why: Some(sentence), can: Some((op, true)), keeps_note: true, ..stands(Light::Down, word) };
        }
        return Stands { host_said: Some(said.to_owned()), there, behind: op == HostOp::Update, can: Some((op, true)), ..stands(Light::Down, word) };
    }
    let installed = |s: Stands| Stands { there: true, can: Some((HostOp::Remove, false)), ..s };
    let absent = |s: Stands| Stands { can: Some((HostOp::Install, false)), ..s };
    match thing {
        HostThing::Helper => match &host.helper {
            HostHelper::Installing => Stands { busy: true, ..stands(Light::Waiting, "Being installed…") },
            // (Taken off while a connection goes through it: that one
            // goes on through it, a program running.)
            HostHelper::Used if host.helper_kept_off => absent(more(stands(Light::Waiting, "Not installed"), "this connection still uses it")),
            HostHelper::Used if host.helper_current == Some(false) && host.helper_sync_err.is_some() => {
                Stands { host_said: host.helper_sync_err.clone(), behind: true, ..installed(more(stands(Light::Down, "Update failed"), "older build in use")) }
            }
            HostHelper::Used if host.helper_current == Some(false) => Stands { behind: true, ..installed(more(stands(Light::Waiting, "Update available"), "older build in use")) },
            // (A host's own screens are run by it: said where it would be taken off.)
            HostHelper::Used if host.screens.as_ref().is_some_and(|s| s.installed.is_some()) => {
                Stands { which: Some(olo_kit::words::tr("Agent Screens on this host run through it.").into()), ..installed(more(stands(Light::On, "Up to date"), "in use")) }
            }
            HostHelper::Used => installed(more(stands(Light::On, "Up to date"), "in use")),
            HostHelper::Ready if host.helper_current == Some(false) && host.helper_sync_err.is_some() => {
                Stands { host_said: host.helper_sync_err.clone(), behind: true, ..installed(more(stands(Light::Down, "Update failed"), "older build on host")) }
            }
            HostHelper::Ready if host.helper_current == Some(false) => Stands { behind: true, ..installed(more(stands(Light::Waiting, "Update available"), "older build on host")) },
            HostHelper::Ready => installed(more(stands(Light::On, "Up to date"), "used the next time you connect")),
            HostHelper::Without(why) => Stands { host_said: Some(why.clone()), can: Some((HostOp::Install, true)), ..stands(Light::Down, "Couldn’t install") },
            HostHelper::None if !host.helper_setting => more(stands(Light::Off, "Not installed"), "turned off for every host"),
            HostHelper::None if !crate::link::carried() => stands(Light::Off, "Not installed"),
            HostHelper::None => absent(stands(Light::Off, "Not installed")),
        },
        HostThing::Bridge => unreachable!("answered above"),
        // (As this computer's Token Pool says, which puts it there and
        // keeps its accounts in step: `pool`.)
        // (Never "there" for Remove All…, and with no Install or Remove
        // of this window's: its accounts are the Token Pool's to move;
        // when behind or failed, Update / Try Again triggers a sync right here.)
        HostThing::Pool => {
            let accounts = |pool: &crate::pool::PoolStands| -> Option<String> {
                match pool.accounts {
                    Some(1) => Some(olo_kit::words::tr("1 account").to_string()),
                    Some(n) => Some(olo_kit::tr!("{} accounts", n).to_string()),
                    None => None,
                }
            };
            match &host.pool {
                None => more(stands(Light::Off, "Not known"), "Token Pool isn’t running on this computer"),
                Some(pool) if pool.doing.is_some() => Stands { busy: true, ..stands(Light::Waiting, if pool.wanted { "Being updated…" } else { "Being installed…" }) },
                Some(pool) if pool.wanted && pool.in_step => Stands { more: accounts(pool), ..stands(Light::On, "Up to date") },
                Some(pool) if pool.wanted && pool.error.is_some() => Stands { more: accounts(pool), why: pool.error.clone(), behind: true, ..stands(Light::Down, "Can’t sync") },
                Some(pool) if pool.wanted => Stands { more: accounts(pool), behind: true, ..stands(Light::Waiting, "Update available") },
                Some(pool) if pool.error.is_some() => Stands { why: pool.error.clone(), ..stands(Light::Down, "Couldn’t install") },
                Some(_) => stands(Light::Off, "Not installed"),
            }
        }
        HostThing::Skill => match (host.skill, host.skill_current) {
            (Some(true), Some(false)) if host.skill_sync_err.is_some() => Stands { host_said: host.skill_sync_err.clone(), behind: true, ..installed(stands(Light::Down, "Update failed")) },
            (Some(true), Some(false)) => Stands { behind: true, ..installed(stands(Light::Waiting, "Update available")) },
            (Some(true), _) => installed(stands(Light::On, "Up to date")),
            (Some(false), _) => absent(stands(Light::Off, "Not installed")),
            (None, _) => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
        },
        HostThing::Commands => match (host.commands, host.commands_current) {
            (Some(true), Some(false)) if host.commands_sync_err.is_some() => Stands { host_said: host.commands_sync_err.clone(), behind: true, ..installed(stands(Light::Down, "Update failed")) },
            (Some(true), Some(false)) => Stands { behind: true, ..installed(stands(Light::Waiting, "Update available")) },
            (Some(true), _) => installed(stands(Light::On, "Up to date")),
            (Some(false), _) => absent(stands(Light::Off, "Not installed")),
            (None, _) => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
        },
        // (As the host's helper says: `host_screens`. Never "there" for
        // Remove All…, which takes off what is put back in a moment:
        // this is two gigabytes, and may have screens running on it.
        // Nor "behind" for Update All: it is fetched when chosen.)
        HostThing::Screens => {
            let on_its_own = |s: Stands| Stands { there: false, behind: false, keeps_note: true, ..s };
            match &host.screens {
                None => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
                // (A helper from before it had screens, or none: Install
                // puts this Olo's there first, and says so.)
                Some(s) if !s.helper && crate::link::carried() && host.helper_setting => Stands {
                    which: Some(olo_kit::words::tr("Install puts the typing helper there first (the Rex tab’s).").into()),
                    ..absent(more(stands(Light::Off, "Not installed"), "needs the typing helper first"))
                },
                Some(s) if !s.helper => more(stands(Light::Off, "Not installed"), "needs the typing helper first"),
                Some(s) => match (s.summary(), s.lacking()) {
                    (None, _) => absent(stands(Light::Off, "Not installed")),
                    (what, Some(why)) => on_its_own(Stands { more: what, why: Some(why), ..installed(stands(Light::Down, "Can’t run")) }),
                    // (What is there and what updating brings; its line
                    // has two buttons then, and room for no more. With
                    // screens running an update waits: said before it is
                    // pressed, not after it is refused.)
                    (_, None) if host.this_olo.as_ref().zip(s.committed).is_some_and(|((mine, _), theirs)| theirs < *mine) => {
                        // (Their names are the list's, under it, where
                        // the host listed them: said once.)
                        let waits = match s.running_said() {
                            Some(_) if !s.rows.is_empty() => Some(olo_kit::words::tr("Update once the running screens below are stopped.").to_string()),
                            Some(running) => Some(format!("{running} {}", olo_kit::words::tr("Update once they are stopped."))),
                            None => None,
                        };
                        on_its_own(Stands {
                            more: host.this_olo.as_ref().and_then(|(_, mine)| s.update_said(mine)),
                            which: waits,
                            can: Some((HostOp::Update, false)),
                            ..installed(stands(Light::Waiting, "Update available"))
                        })
                    }
                    // (Which run: the list under it says, where the host
                    // listed them; said here of a helper that doesn't.)
                    (what, None) => on_its_own(Stands { more: what, which: s.running_said().filter(|_| s.rows.is_empty()), ..installed(stands(Light::On, "Installed")) }),
                },
            }
        }
    }
}

/// The Rex server's line: running (or update available · old → new · running).
fn server_stands(host: &HostStands) -> Stands {
    if host.conn != HostConnState::Connected {
        return stands(Light::Off, "Not known");
    }
    if let Some(newer) = &host.server_newer {
        let running = olo_kit::words::tr("running");
        let more = match &host.version {
            Some(ver) => format!("{ver} → {newer}  ·  {running}"),
            None => format!("{newer}  ·  {running}"),
        };
        return Stands { more: Some(more), ..stands(Light::Waiting, "Update available") };
    }
    Stands { more: host.version.clone(), ..stands(Light::On, "Running") }
}

/// A section's heading, over its rule (the Server window's).
fn section(title: &'static str) -> Element {
    // (A literal, looked up in the person's language: a `String` is shown
    // as it is.)
    div().w_full().flex().flex_col().child(StringView::new(title).bold(true).height(m::LABEL_ROW)).child(div().h(px(2.0)).flex_shrink_0()).child(separator()).into_element()
}

/// On Tweaks: from a tweak's reason to the next tweak's line, and how
/// tall a reason is (14 pt exact, matching `m::CONTROL_HEIGHT = 24 pt`
/// so every row's pitch is `24 + 14 + 6 = 44 pt`, a whole pixel count
/// at 1×, 1.25×, 1.5×, 1.75× and 2×: 44, 55, 66, 77, 88 px).
const ROW_GAP: f32 = 6.0;
const ROW_REASON: f32 = 14.0;
/// On Tweaks: from Heavy work's last reason to Sign-in shell's 24 pt
/// heading box (whose cap top sits 8 pt down -> 24 pt to the heading's
/// cap top, matching the window's other tabs).
const SECTION_GAP: f32 = 16.0;
/// The seven tweaks under "Heavy work" (Brush shell is in its own
/// section below).
pub(crate) const HEAVY_WORK: [olo_rex_client::tune::Tune; 7] = [
    olo_rex_client::tune::Tune::Sessions,
    olo_rex_client::tune::Tune::Files,
    olo_rex_client::tune::Tune::Kernel,
    olo_rex_client::tune::Tune::Writeback,
    olo_rex_client::tune::Tune::Memory,
    olo_rex_client::tune::Tune::TempFiles,
    olo_rex_client::tune::Tune::RexLimits,
];

/// Heavy-work tweaks that can be applied on this host. "Apply Suggested…"
/// is offered only when two or more rows can be applied (one row's own
/// Apply… is the same thing).
pub(crate) fn tweaks_suggested(tuning: &olo_rex_client::tune::Tuning) -> Vec<olo_rex_client::tune::Tune> {
    tuning.short().into_iter().filter(|kind| HEAVY_WORK.contains(kind)).collect()
}

/// Plain-word amount for one Rex server limit value ("512 tabs", "1 tab",
/// "no limit on tabs").
fn rex_limit_amount(flag: &str, value: u64) -> String {
    use olo_rex_client::tune::said;
    let (singular, plural, none) = match flag {
        "--max-windows" => ("1 tab", "{} tabs", "no limit on tabs"),
        "--max-panes" => ("1 pane", "{} panes", "no limit on panes"),
        "--max-sessions" => ("1 session", "{} sessions", "no limit on sessions"),
        _ => ("1 block", "{} blocks", "no limit on blocks"),
    };
    if value == u64::MAX {
        olo_kit::words::tr(none).into()
    } else if value == 1 {
        olo_kit::words::tr(singular).into()
    } else {
        olo_kit::words::tr(plural).replace("{}", &said(flag, value))
    }
}

/// Plain-word waiting-for-restart status for [`Tune::RexLimits`]: names the
/// state first (`applied` or `put back`) followed by what holds after Rex
/// restarts (`applied, 512 tabs after Rex restarts` / `put back, 64 tabs after Rex restarts`).
fn pending_restart_status(kept: bool, running: olo_rex_client::tune::RexLimitValues, configured: olo_rex_client::tune::RexLimitValues) -> String {
    let state = olo_kit::words::tr(if kept { "applied" } else { "put back" });
    let next_said = if running.windows != configured.windows {
        rex_limit_amount("--max-windows", configured.windows)
    } else if running.panes != configured.panes {
        rex_limit_amount("--max-panes", configured.panes)
    } else if running.sessions != configured.sessions {
        rex_limit_amount("--max-sessions", configured.sessions)
    } else {
        rex_limit_amount("--max-blocks", configured.blocks)
    };
    olo_kit::tr!("{}, {} after Rex restarts", state, next_said).into()
}

/// How one tweak's line stands in the Tweaks tab: its light and its
/// status words (after the dot beside the tweak's name).
pub(crate) fn tweak_state(tuning: &olo_rex_client::tune::Tuning, tune: olo_rex_client::tune::Tune) -> (Light, String) {
    use olo_rex_client::tune::{Tune, said};
    let stands = tuning.of(tune);
    let short: Vec<_> = stands.values.iter().filter(|v| !v.met()).collect();
    if tune == Tune::RexLimits && tuning.rex_limits.blocked.is_some() {
        return (Light::Off, olo_kit::words::tr("can’t apply here").into());
    }
    if tune == Tune::RexLimits && tuning.rex_limits.stale {
        return (Light::Waiting, olo_kit::words::tr("no longer applied").into());
    }
    if tune == Tune::RexLimits
        && let Some((running, configured)) = tuning.rex_limits_pending_restart()
    {
        return (Light::Waiting, pending_restart_status(stands.kept, running, configured));
    }
    match (tune, short.as_slice()) {
        (_, []) if stands.kept => (Light::On, olo_kit::words::tr("applied").into()),
        (_, []) => (Light::On, olo_kit::words::tr("as the machine has it").into()),
        (Tune::Shell, _) if tuning.brush_there => (Light::Off, olo_kit::words::tr("installed, not what new accounts are given").into()),
        (Tune::Shell, _) => (Light::Off, olo_kit::words::tr("not installed").into()),
        (Tune::RexLimits, [first, ..]) => {
            let now_said = first.now.map_or_else(|| "?".to_owned(), |now| rex_limit_amount(first.name, now));
            (Light::Off, olo_kit::tr!("{} now, {} suggested", now_said, said(first.name, first.wanted)).into())
        }
        (_, [one]) if stands.values.len() == 1 => (Light::Off, olo_kit::tr!("{} now, {} suggested", one.now.map_or("?".to_owned(), |now| said(one.name, now)), said(one.name, one.wanted)).into()),
        (_, all) if all.len() == stands.values.len() => (Light::Off, olo_kit::tr!("{} settings to change", all.len()).into()),
        (_, some) => (Light::Off, olo_kit::tr!("{} of {} settings to change", some.len(), stands.values.len()).into()),
    }
}

/// The Tweaks tab's page, and whether it fills the tab's panel to its
/// frame: the machine's own settings, as root found them
/// (`olo_rex_client::tune`): each kind a line that says how the host
/// stands, with Apply… where something of it is short and Put Back…
/// where Olo's file is there, and its reason under it. They were the
/// Server window's Limits page (Ben, 2026-11-26: "they really belong in
/// manage host. please move those into a new tab in the manage host
/// window called 'Tweaks'"), with two more: temporary files kept on
/// disk, and brush as the sign-in shell.
///
/// The host is asked as the tab is first shown (root signed in, or
/// sudo: `server::for_tweaks`); while it is, while a change is put to
/// the person, is under way or didn't go, the page itself is that
/// stage's alert, as the Server window shows it: the alert's stripe
/// down the panel's own edge, its words and buttons ending where the
/// list's do. (It was a framed box in the page: a frame in a frame,
/// its edges an inset off the list's, a card set on the window; Opus's
/// look.) Where there is nothing to show, the page says why in a line
/// of the window's own grammar, under a heading, as its other tabs say
/// how a thing stands (it was an icon over centred words, the one
/// thing in the window that was).
fn tweaks_page(host: &HostStands, managed: bool, tweaks: Option<&crate::server::Managed>, phase: f32, wide: f32, on_do: &Rc<dyn Fn(&HostDo, &mut WindowCx)>) -> (Element, bool) {
    use crate::server::Stage;
    use crate::server_panel::ServerDo;
    use olo_rex_client::manage::Action;
    use olo_rex_client::tune::Tune;
    let ask = |what: ServerDo| HostDo::Tweak(what);
    // (At least as wide as the window's column of buttons, whose right
    // edge it ends at: "Apply Suggested…" and "Read Settings" are a
    // little wider, each alone on its line. Counted into the column's
    // width they widened every button of every tab, and a note beside
    // them on another tab took a line more: the window grew 15 points.)
    let button = |label: &'static str, id: &str, what: HostDo, enabled: bool| {
        let on_do = on_do.clone();
        Button::new(label).id(id.to_owned()).min_width(wide).enabled(enabled).on_click(move |_, cx| on_do(&what, cx))
    };
    let reason_text = |text: String| {
        let p = olo_kit::theme::palette();
        div().w_full().text_size(px(style::SMALL_SIZE)).line_height(px(ROW_REASON)).text_color(style::secondary_text(&p)).child(text).into_element()
    };
    let gap = |h: f32| div().h(px(h)).flex_shrink_0().into_element();
    let under = panels::LIGHT + m::TIGHT;
    let short_of = wide + m::SECTION;
    // A line of the page, in the window's own grammar: its light, its
    // name, how it stands after a dot, its button in the window's
    // column; and its reason right under it. The title row is fixed at
    // 24 pt and a single-line reason at 14 pt (`24 + 14 + 6 = 44 pt`
    // pitch, whole pixels at all five scales).
    let line = |lamp: Light, name: String, state: Option<String>, action: Option<Button>, reason: Element, span_full: bool| {
        let mut words = div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT)).child(panels::light(lamp)).child(StringView::new(name));
        if let Some(state) = state {
            words = words.child(div().flex_1().min_w_0().child(StringView::new(format!("·  {state}")).dim(true).cut(true)));
        }
        // (Fixed at a control's 24 pt height, with a button or without: one exact pitch down the page.)
        let mut row = div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().gap(px(m::SPACING)).child(words);
        let buttoned = action.is_some();
        if let Some(action) = action {
            row = row.child(action);
        }
        // (Short of the buttons' column where the line has a button; to
        // the page's edge where it has none or carries a full-width
        // single-line restart instruction beneath the button row.)
        let why = div().w_full().min_h(px(ROW_REASON)).pl(px(under)).pr(px(if buttoned && !span_full { short_of } else { 0.0 })).child(reason);
        div().w_full().flex().flex_col().child(row).child(why).into_element()
    };
    // Both sections share one 24 pt heading line with 4 pt clear above
    // the rule so "Apply Suggested…" sits centred on "Heavy work" and
    // hiding it moves nothing.
    let tweak_section = |title: &'static str, action: Option<Button>| {
        let mut head = div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().child(div().flex_1().min_w_0().child(StringView::new(title).bold(true).height(m::CONTROL_HEIGHT)));
        if let Some(action) = action {
            head = head.child(action);
        }
        div().w_full().flex().flex_col().child(head).child(gap(4.0)).child(separator()).into_element()
    };
    let titled =
        |title: &'static str, action: Option<Button>, under: Vec<Element>| div().w_full().flex().flex_col().child(tweak_section(title, action)).child(gap(m::TIGHT)).children(under).into_element();
    // (Top and bottom inset 4 pt less because the 24 pt heading row centres
    // its 16 pt text 4 pt lower, keeping the first heading text level with
    // other tabs and keeping Tweaks within the other tabs' height at every scale.)
    let page = |sections: Vec<Element>| div().w_full().py(px(m::SPACING - 4.0)).px(px(m::SPACING)).flex().flex_col().children(sections).into_element();
    let nothing = |name: &'static str, reason: String, action: Option<Button>| {
        (page(vec![titled("Settings", None, vec![line(Light::Off, olo_kit::words::tr(name).to_string(), None, action, reason_text(reason), false)])]), false)
    };
    if !managed {
        return nothing("Not reached over SSH", olo_kit::tr!("Rex changes a machine’s settings as root, over SSH. Olo reaches {} another way.", host.name).into(), None);
    }
    let Some(tweaks) = tweaks else {
        if host.conn != HostConnState::Connected {
            return nothing("Not connected", olo_kit::tr!("Connect to {} to see how it is set.", host.name).into(), None);
        }
        return nothing(
            "Not read",
            olo_kit::tr!("Rex reads {}’s settings as root. Nothing is changed until you choose something to do.", host.name).into(),
            Some(button("Read Settings", "host-tweaks-read", ask(ServerDo::Again), true)),
        );
    };
    // The alert a stage is: the panel to its frame (`TabView::fill_page`),
    // so the alert's stripe runs down the panel's own edge. Its words
    // and buttons end where the list's do: the list stands in from the
    // panel's outside by the panel's inset and the page's own, an alert
    // from the frame's inside by the window's inset.
    let frame = m::WINDOW_INSET - olo_kit::widgets::tabs::fill_reach();
    let more = (m::SPACING - frame).max(0.0);
    let width = HOST_WIDTH - 2.0 * m::WINDOW_INSET - 2.0 * frame - more;
    let stage_button = {
        let on_do = on_do.clone();
        move |label: &'static str, what: ServerDo| {
            let on_do = on_do.clone();
            let what = HostDo::Tweak(what);
            Button::new(olo_kit::words::tr(label).to_owned()).min_width(m::BUTTON_MIN_WIDTH).on_click(move |_, cx| on_do(&what, cx))
        }
    };
    // In brush's question: whether the accounts there are get it too
    // (the table under it follows the choice).
    let extra = match (&tweaks.stage, tweaks.server.as_ref().and_then(|server| server.tuning.as_ref())) {
        (Stage::Confirm { action: Action::Tune(kinds), .. }, Some(tuning)) if kinds.contains(&Tune::Shell) && !tuning.of(Tune::Shell).met() && !tuning.accounts.is_empty() => {
            let on_do = on_do.clone();
            let label: SharedString = match tuning.accounts.as_slice() {
                [(only, _)] => olo_kit::tr!("Also give Brush to {}, the account already on {}", only, host.name).into(),
                accounts => olo_kit::tr!("Also give Brush to the {} accounts already on {}", accounts.len(), host.name).into(),
            };
            Some(olo_kit::widgets::CheckBox::new(label, tuning.everyone).id("host-tweaks-everyone").on_toggle(move |on: &bool, cx| on_do(&HostDo::TweakEveryone(*on), cx)).into_element())
        }
        _ => None,
    };
    let wrap = |icon: &str, body: Vec<Element>, foot: Element| {
        let said = div().w_full().pr(px(more)).flex().flex_col().children(body).child(gap(m::SPACING + 4.0)).child(foot);
        // (Begun where the list's first line is, as it ends where the
        // list's lines do: the page's first line stood ten points
        // higher as a question than as the list, and jumped.)
        olo_kit::widgets::AlertFrame::new(icon.to_owned()).lower(more).child(said).into_element()
    };
    let on_key = {
        let on_do = on_do.clone();
        move |i: &usize, cx: &mut WindowCx| on_do(&HostDo::TweakKey(*i), cx)
    };
    if let Some(stage) = crate::server_panel::render_stage(tweaks, phase, width, true, extra, &wrap, &stage_button, on_key, |_: &usize, _: &mut WindowCx| {}) {
        return (stage, true);
    }
    let Some(tuning) = tweaks.server.as_ref().and_then(|server| server.tuning.as_ref()) else {
        return nothing("Not read", olo_kit::tr!("{} didn’t say how it is set.", host.name).into(), Some(button("Try Again", "host-tweaks-read", ask(ServerDo::Again), true)));
    };
    // One tweak: how it stands, and what its button asks.
    let space = f32::from(style::text_width(" ", style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
    let row = |tune: Tune| {
        let stands = tuning.of(tune);
        let blocked = (tune == Tune::RexLimits).then(|| tuning.rex_limits.blocked.as_deref()).flatten();
        let needs_apply = blocked.is_none() && (!stands.met() || (tune == Tune::RexLimits && tuning.rex_limits.stale));
        let (lamp, state) = tweak_state(tuning, tune);
        let id = format!("host-tweak-{}", tune.id());
        let action = match (blocked.is_some(), needs_apply, stands.kept) {
            (true, _, true) => Some(button("Put Back…", &id, ask(ServerDo::Ask(Action::Untune(tune))), true)),
            (true, _, false) => None,
            (false, true, _) => Some(button("Apply…", &id, ask(ServerDo::Ask(Action::Tune(vec![tune]))), true)),
            (false, false, true) => Some(button("Put Back…", &id, ask(ServerDo::Ask(Action::Untune(tune))), true)),
            (false, false, false) => Some(button("Apply…", &id, ask(ServerDo::Back), false)),
        };
        let (reason, span_full): (Element, bool) = if let Some(why) = blocked {
            (reason_text(why.to_owned()), false)
        } else if tune == Tune::RexLimits && tuning.rex_limits.stale {
            (reason_text(olo_kit::words::tr("Rex’s service changed after these limits were set. Apply them again.").into()), false)
        } else if tune == Tune::RexLimits && tuning.rex_limits_pending_restart().is_some() {
            if tuning.rex_limits.scope == olo_rex_client::tune::RexScope::Shared {
                let link_label = olo_kit::words::tr("Server window").to_string();
                let link_tw = f32::from(style::text_width(link_label.clone(), style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
                let link_extra = (((link_tw / 2.0).ceil() * 2.0) - link_tw).max(0.0);
                let on_server = on_do.clone();
                let link = olo_kit::widgets::LinkButton::new(link_label)
                    .size(style::SMALL_SIZE)
                    .line(ROW_REASON)
                    .id("host-tweak-rex-server")
                    .enabled(host.conn == HostConnState::Connected)
                    .on_click(move |_, cx| on_server(&HostDo::Server, cx));
                let template = olo_kit::words::tr("Restarting ends the programs in everyone’s tabs. Open the {} to restart.");
                let (before, after) = template.split_once("{}").unwrap_or((template, ""));
                let piece = |w: &str| StringView::new(w.trim().to_owned()).size(style::SMALL_SIZE).dim(true).line(ROW_REASON);
                let mut r = div().w_full().h(px(ROW_REASON)).flex().flex_row().items_center().gap(px(space));
                if !before.trim().is_empty() {
                    r = r.child(piece(before));
                }
                r = r.child(link);
                if !after.trim().is_empty() {
                    let trim = if after.starts_with(' ') { -link_extra } else { -space - link_extra };
                    r = r.child(div().ml(px(trim)).child(piece(after)));
                }
                (r.into_element(), true)
            } else {
                let lead = StringView::new(olo_kit::words::tr("Restarting ends every tab’s program. To restart:").to_owned()).size(style::SMALL_SIZE).dim(true).line(ROW_REASON);
                let cmd = StringView::new("systemctl --user restart rex.service".to_owned()).mono(true).size(panels::COMMAND_SIZE).height(ROW_REASON).beside(style::SMALL_SIZE);
                (div().w_full().h(px(ROW_REASON)).flex().flex_row().items_center().gap(px(space)).child(lead).child(cmd).into_element(), true)
            }
        } else {
            (reason_text(olo_kit::words::tr(tune.why()).into()), false)
        };
        line(lamp, olo_kit::words::tr(tune.title()).to_string(), Some(state), action, reason, span_full)
    };
    let rows = |kinds: &[Tune]| div().w_full().flex().flex_col().gap(px(ROW_GAP)).children(kinds.iter().map(|kind| row(*kind))).into_element();
    // What the machine comes with, changed for heavy work: "Apply Suggested…"
    // sits on the "Heavy work" heading's line, offered only when two or more
    // rows can be applied (one row's own Apply… is the same thing), so hiding
    // it moves nothing and the window keeps one height in every state.
    let suggested = tweaks_suggested(tuning);
    let apply_all = (suggested.len() >= 2).then(|| button("Apply Suggested…", "host-tweaks-all", ask(ServerDo::Ask(Action::Tune(suggested))), true));
    let heavy = vec![reason_text(olo_kit::words::tr("Changes a Linux machine’s defaults for long builds and many sessions.").into()), gap(m::STACK), rows(&HEAVY_WORK)];
    (page(vec![titled("Heavy work", apply_all, heavy), gap(SECTION_GAP), titled("Sign-in shell", None, vec![rows(&[Tune::Shell])])]), false)
}

/// The window's content for `host`: `managed` whether its Rex server
/// can be looked after from here.
/// (`tweaks`: the host as root found it, for the Tweaks tab, where it
/// has been asked; `phase`: where a busy bar's block is.)
pub fn render_host_panel(
    host: &HostStands,
    shown: &Shown,
    managed: bool,
    screens_list: Option<Element>,
    tweaks: Option<&crate::server::Managed>,
    phase: f32,
    on_do: impl Fn(&HostDo, &mut WindowCx) + 'static,
) -> Element {
    let on_do: Rc<dyn Fn(&HostDo, &mut WindowCx)> = Rc::new(on_do);
    // Every button one width, the widest of their words: one column
    // down the window's right side.
    let wide = [
        "Check Again",
        "Connect",
        "Manage…",
        "Update…",
        "Update",
        "Update All",
        "Permissions…",
        "Token Pool…",
        "Install",
        "Remove",
        "Try Again",
        "Remove All…",
        "Cancel",
        "Sync Now",
        "Forget…",
        "Forget",
        "Copy",
        "Look",
        "Stop",
        "Apply…",
        "Put Back…",
    ]
    .iter()
    .map(|l| button_width(l, false))
    .fold(0.0, f32::max);
    // (Its words a literal, which the kit looks up in the person's
    // language: a `String` is shown as it is, and every button here was in
    // English in every language.)
    let button = |label: &'static str, id: &str, what: HostDo, enabled: bool| {
        let on_do = on_do.clone();
        Button::new(label).id(id.to_owned()).min_width(wide).enabled(enabled).on_click(move |_, cx| on_do(&what, cx))
    };
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    // (What is under a line's words begins where they do, past the
    // light, and ends short of the buttons' column: the light alone in
    // the margin, the buttons with a column to themselves.)
    let under = panels::LIGHT + m::TIGHT;
    let short = wide + m::SECTION;
    // (A tab's panel: its border and its inset, each side.)
    let panel = 2.0 * (m::SPACING + 1.0);
    // (How many letters a line of a well under a line's words holds.)
    let letter = f32::from(style::text_width("0000000000", MONO_SIZE, style::mono_font(olo_kit::FontWeight::NORMAL))) / 10.0;
    let room = (((HOST_WIDTH - 2.0 * m::WINDOW_INSET - panel - under - 2.0 * panels::WELL_INSET - 2.0) / letter).floor() as usize).max(20);
    // A thing's line and what is under it: one steady 2-line slot under every
    // status line (description + sync policy on its own line), replaced in place
    // by a sunken monospace well on raw command errors or an Olo prose sentence
    // on gateway errors so the window keeps one height across every state.
    // (`slot`: something of the state's own that isn't words, in the
    // place its sentence would take: an install's bar.)
    let body = |s: &Stands, actions: Vec<Button>, slot: Option<Element>, note: String, policy: String, path: Option<&str>| {
        let p = olo_kit::theme::palette();
        // Regular-weight error labels need slightly higher contrast in light mode to match
        // bold error headlines optically at 1× and 2× (Gemini Round 2 review).
        let err_red = style::c(olo_theme::readable(p.failure, p.panel_background, 5.5));
        let lamp = if s.busy { panels::busy_of(panels::LIGHT) } else { panels::light(s.light) };
        let mut said = div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT)).child(lamp);
        let said_sv = if s.light == Light::Down { StringView::new(s.said.clone()).color(err_red) } else { StringView::new(s.said.clone()) };
        said = said.child(said_sv);
        if let Some(more) = &s.more {
            said = said.child(div().flex_1().min_w_0().child(StringView::new(format!("·  {more}")).dim(true).cut(true)));
        }
        let line = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(said).children(actions);
        let mut block = div().w_full().flex().flex_col().child(line);
        // (A bar is its status line's: 5 points under the row, 10 over
        // the description, which stays where it was to a point; and it
        // runs to the left edge of the button beside the line, which
        // it ended 25 points short of, at nothing. Set midway it stood
        // nearer the description, 9 points against 8, and read as that
        // paragraph's rule: both reviewers.)
        let barred = slot.is_some();
        let mut notes = div().w_full().pl(px(under)).flex().flex_col();
        if let Some(slot) = slot {
            notes = notes.child(div().w_full().pr(px(wide)).child(slot)).child(div().h(px(10.0)).flex_shrink_0());
        }
        if let Some(host_said) = &s.host_said {
            // Verbatim command stderr in the sunken monospace well replaces line 1 (with
            // 4 pt clearance under the action buttons) while keeping line 2 (sync policy)
            // beneath it so every failure row shares the exact same 4-part layout.
            let lines: Vec<String> = host_said.lines().filter(|l| !l.trim().is_empty()).flat_map(|line| crate::server_panel::hung(line, room)).take(3).collect();
            let lines = div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(MONO_SIZE).wrap(true)));
            notes = notes.child(div().h(px(4.0)).flex_shrink_0()).child(panels::well(lines.into_element()));
            if !policy.is_empty() {
                notes = notes.child(div().h(px(6.0)).flex_shrink_0()).child(div().w_full().pr(px(short)).child(small(policy)));
            }
        } else {
            let mut text_col = div().w_full().pr(px(short)).flex().flex_col();
            if let Some(why) = &s.why {
                // Olo's own message replaces the description line in UI font when
                // the row failed (e.g. Token Pool gateway error); on a Bridge SSH-keys
                // warning it sits cleanly above the description.
                text_col = text_col.child(StringView::new(why.clone()).size(style::SMALL_SIZE).wrap(true));
                if (s.light != Light::Down || s.keeps_note) && !note.is_empty() {
                    text_col = text_col.child(div().h(px(4.0)).flex_shrink_0()).child(small(note));
                }
            } else {
                if let Some(which) = &s.which {
                    text_col = text_col.child(small(which.clone())).child(div().h(px(4.0)).flex_shrink_0());
                }
                if !note.is_empty() {
                    text_col = text_col.child(small(note));
                }
            }
            if !policy.is_empty() {
                text_col = text_col.child(small(policy));
            }
            notes = notes.child(text_col);
        }
        if let Some(path) = path {
            notes = notes
                .child(div().h(px(4.0)).flex_shrink_0())
                .child(div().w_full().pr(px(short)).child(StringView::new(path.to_owned()).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(MONO_SIZE))));
        }
        block = block.child(div().h(px(if barred { 5.0 } else { m::TIGHT + 2.0 })).flex_shrink_0()).child(notes);
        block
    };
    let block_with = |title: &'static str, s: &Stands, actions: Vec<Button>, slot: Option<Element>, note: String, policy: String, path: Option<&str>| {
        div().w_full().flex().flex_col().child(section(title)).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(body(s, actions, slot, note, policy, path)).into_element()
    };
    let block = |title: &'static str, s: &Stands, actions: Vec<Button>, note: String, policy: String, path: Option<&str>| block_with(title, s, actions, None, note, policy, path);
    let reached = host.conn == HostConnState::Connected && host.ssh;
    let connected = host.conn == HostConnState::Connected;
    let busy = !shown.doing.is_empty();

    // Compute per-component state first so the header knows whether `Update All`
    // should appear beside `Check Again` and each tab knows its status dot.
    let srv_s = server_stands(host);
    let mut rex_actions = Vec::new();
    if host.server_newer.is_some() {
        rex_actions.push(button("Update…", "host-server-update", HostDo::UpdateServer, managed && connected));
    }
    rex_actions.push(button("Manage…", "host-server", HostDo::Server, managed && connected));
    let mut rex: Vec<Element> = vec![block(
        "Rex server",
        &srv_s,
        rex_actions,
        olo_kit::words::tr("Runs persistent sessions and shells on this host.").into(),
        olo_kit::words::tr("Updates only when you choose (needs root).").into(),
        None,
    )];
    let mut olo = Vec::new();

    let mut there = Vec::new();
    let mut behind = Vec::new();
    let mut rex_dot: Option<Light> = if srv_s.light == Light::Waiting { Some(Light::Waiting) } else { None };
    let mut olo_dot: Option<Light> = None;
    let bump_dot = |slot: &mut Option<Light>, light: Light| match light {
        Light::Down => *slot = Some(Light::Down),
        Light::Waiting if *slot != Some(Light::Down) => *slot = Some(Light::Waiting),
        _ => {}
    };

    for (thing, title, id, note, policy_ok, policy_behind, path) in [
        (HostThing::Bridge, "Bridge", "bridge", "Runs from this computer over Rex (nothing installed on the host).", "Always matches this Olo.", "Always matches this Olo.", None),
        (
            HostThing::Commands,
            "Desktop commands",
            "commands",
            "Lets xdg-open, notify-send, op and clipboard commands act here.",
            "Updates automatically when you connect.",
            "Updates automatically next time you connect.",
            Some("~/.local/share/olo/rex-commands"),
        ),
        (
            HostThing::Skill,
            "Skill for agents",
            "skill",
            "Tells Claude Code and other agents there how to use Olo Bridge and Rex.",
            "Updates automatically when you connect.",
            "Updates automatically next time you connect.",
            Some("~/.claude/skills/olo-rex-bridge"),
        ),
        (
            HostThing::Pool,
            "Token Pool",
            "pool",
            "Lets agents there use your accounts, even with this computer off.",
            "Syncs automatically while Token Pool runs.",
            "Syncs automatically while Token Pool runs.",
            None,
        ),
        (
            HostThing::Helper,
            "Typing helper",
            "helper",
            "Speeds up typing and powers remote files, sync and activity.",
            "Updates automatically when you connect.",
            "Updates automatically next time you connect.",
            Some("~/.local/share/olo/rex-link"),
        ),
    ] {
        let s = thing_stands(host, shown, thing);
        if s.there {
            there.push(thing);
        }
        if s.behind && !s.busy {
            behind.push(thing);
        }
        if !s.busy && matches!(s.light, Light::Down | Light::Waiting) {
            if thing == HostThing::Helper {
                bump_dot(&mut rex_dot, s.light);
            } else {
                bump_dot(&mut olo_dot, s.light);
            }
        }
        let mut actions = Vec::new();
        if thing == HostThing::Bridge {
            actions.push(button("Permissions…", "host-bridge-permissions", HostDo::Permissions, true));
        } else if thing == HostThing::Pool {
            if s.behind && !s.busy && s.can.is_none() {
                let label = if s.light == Light::Down { "Try Again" } else { "Update" };
                let btn_id = if s.light == Light::Down { "host-pool-again" } else { "host-pool-update" };
                actions.push(button(label, btn_id, HostDo::Change(HostThing::Pool, HostOp::Update), reached));
            } else if let Some((op, true)) = s.can {
                actions.push(button("Try Again", "host-pool-again", HostDo::Change(HostThing::Pool, op), reached));
            }
            actions.push(button("Manage…", "host-pool-open", HostDo::OpenPool, true));
        } else {
            if s.behind && !s.busy && !matches!(s.can, Some((_, true))) {
                let label = if s.light == Light::Down { "Try Again" } else { "Update" };
                let btn_id = if s.light == Light::Down { format!("host-{id}-again") } else { format!("host-{id}-update") };
                actions.push(button(label, &btn_id, HostDo::Change(thing, HostOp::Update), reached));
            }
            actions.push(match s.can {
                Some((op, true)) => button("Try Again", &format!("host-{id}-again"), HostDo::Change(thing, op), true),
                Some((HostOp::Install, false)) => button("Install", &format!("host-{id}-install"), HostDo::Change(thing, HostOp::Install), true),
                Some((HostOp::Update, false)) => button("Update", &format!("host-{id}-update"), HostDo::Change(thing, HostOp::Update), true),
                Some((HostOp::Remove, false)) => button("Remove", &format!("host-{id}-remove"), HostDo::Change(thing, HostOp::Remove), true),
                None => button(if s.there { "Remove" } else { "Install" }, &format!("host-{id}-wait"), HostDo::Check, false),
            });
        }
        let (note, policy): (String, String) = if thing == HostThing::Helper && !host.helper_setting {
            (olo_kit::words::tr("To use it, turn on Olo’s typing helper in Preferences\u{a0}▸\u{a0}Rex.").into(), String::new())
        } else {
            let pol = if s.behind && s.light == Light::Waiting { policy_behind } else { policy_ok };
            (olo_kit::words::tr(note).into(), olo_kit::words::tr(pol).into())
        };
        let made = block(title, &s, actions, note, policy, path);
        if thing == HostThing::Helper { rex.push(made) } else { olo.push(made) }
    }

    // The host itself, at the window's head.
    let state: String = if shown.checking { olo_kit::words::tr("Checking…").into() } else { panels::host_state(&host.conn, false) };
    let again = match &host.conn {
        HostConnState::Connected => button("Check Again", "host-check", HostDo::Check, host.ssh && !shown.checking),
        HostConnState::Error(_) => button("Try Again", "host-check", HostDo::Connect, true),
        conn => button("Connect", "host-check", HostDo::Connect, *conn == HostConnState::Disconnected),
    };
    let mut head_row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(
            canvas(|g, b| {
                icons::paint(g, b, "network");
            })
            .paint_key(0)
            .w(px(ICON))
            .h(px(ICON))
            .flex_shrink_0(),
        )
        .child(
            div().flex_1().min_w_0().h(px(NAME_ROW + STATE_ROW)).flex().flex_col().child(StringView::new(host.name.clone()).bold(true).size(NAME_SIZE).cut(true).height(NAME_ROW)).child(
                div()
                    .w_full()
                    .h(px(STATE_ROW))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(if shown.checking { panels::busy_of(panels::LIGHT) } else { panels::light(panels::light_for(&host.conn)) })
                    .child(StringView::new(state).size(style::SMALL_SIZE).height(STATE_ROW))
                    .child(div().flex_1().min_w_0().child(StringView::new(format!("·  {}", host.address)).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW))),
            ),
        );
    // (`Update All` sits beside `Check Again` in the header — never ringed as a
    // default button and never crowding host-lifecycle buttons in the footer.)
    if !behind.is_empty() {
        head_row = head_row.child(button("Update All", "host-update-all", HostDo::UpdateAll(behind.clone()), reached && !busy));
    }
    let head = head_row.child(again);

    let why: Option<String> = match &host.conn {
        HostConnState::Error(_) => Some(olo_kit::tr!("What’s installed on {} isn’t known until it connects.", host.name).into()),
        HostConnState::Disconnected => Some(olo_kit::tr!("What’s installed on {} isn’t known until it’s connected.", host.name).into()),
        HostConnState::Connected if !host.ssh => Some(olo_kit::tr!("Nothing can be installed on {} from here: it isn’t reached over SSH.", host.name).into()),
        _ => None,
    };
    let mut head = div().w_full().pb(px(m::TIGHT)).flex().flex_col().child(head);
    if let HostConnState::Error(said) = &host.conn {
        let room = (((HOST_WIDTH - 2.0 * m::WINDOW_INSET - 2.0 * panels::WELL_INSET - 2.0) / letter).floor() as usize).max(20);
        let lines: Vec<String> = said.lines().filter(|l| !l.trim().is_empty()).flat_map(|line| crate::server_panel::hung(line, room)).collect();
        let lines = div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(MONO_SIZE).wrap(true)));
        head = head.child(div().h(px(WELL_OVER + 3.0)).flex_shrink_0()).child(panels::well(lines.into_element()));
        if let Some(why) = why {
            head = head.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(small(why));
        }
    } else if let Some(why) = why {
        head = head.child(div().h(px(HEAD_NOTE_UNDER)).flex_shrink_0()).child(small(why));
    }

    {
        let on_account = on_do.clone();
        let space = f32::from(style::text_width(" ", style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
        let link = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space))
            .child(StringView::new(olo_kit::words::tr("Your agents and repositories:").to_string()).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW))
            .child(
                olo_kit::widgets::LinkButton::new(olo_kit::words::tr("Set Up This Host’s Account…").to_string())
                    .size(style::SMALL_SIZE)
                    .id("host-account")
                    .on_click(move |_, cx| on_account(&HostDo::Account, cx)),
            );
        head = head.child(div().h(px(m::SPACING)).flex_shrink_0()).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(space))
                .child(div().child(StringView::new(olo_kit::words::tr("Rex and Olo on this host.").to_string()).size(style::SMALL_SIZE).dim(true).wrap(true)))
                .child(link),
        );
    }
    let mut page = vec![head.into_element()];

    // The host's folders kept in step here (an agent's `olo sync` there),
    // a list that scrolls in the room the other tabs make (the window
    // keeps its height from tab to tab: both looks). Each a line with its
    // name first and how it stands after, as the Account window's rows
    // read (both looks: the state first buried the name); under it where
    // it is here, a link that opens it in Tracker, and where it is there,
    // each said ("Here", "There": two notations with nothing saying which
    // was which, Opus's look). Stop lets it go (what it brought stays).
    let here_there = [olo_kit::words::tr("Here").to_string(), olo_kit::words::tr("There").to_string(), olo_kit::words::tr("Sync").to_string()];
    let label_w = here_there.iter().map(|l| f32::from(style::text_width(l.clone(), style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)))).fold(0.0, f32::max).ceil();
    // (A detail label column, its value aligned cleanly after a fixed gap.)
    let labelled = |label: &str, column: f32, height: f32, value: Element| {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT + 2.0))
            .child(div().w(px(column)).flex_shrink_0().child(StringView::new(label.to_owned()).size(style::SMALL_SIZE).dim(true).height(height)))
            .child(div().flex_1().min_w_0().child(value))
            .into_element()
    };
    // (A sentence with a link in it, where its language puts it: the
    // words before and after the template's "{}".)
    let space = f32::from(style::text_width(" ", style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
    // (What is typed, in the fixed-width face within a sentence: on the
    // sentence's own line and baseline (a line of its own height beside
    // it stood a pixel off at 1.5× and 1.75×: the layout review's), and
    // a point of room after it: its last letter fills its cell, and
    // "host or" stood 2 pixels apart at 1× where the words either side
    // stood 4 (a reviewer's, measured).)
    let typed =
        |text: &str| div().mr(px(1.0)).child(StringView::new(text.to_owned()).mono(true).size(MONO_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)).beside(style::SMALL_SIZE)).into_element();
    let with_link = |template: &str, link: olo_kit::widgets::LinkButton| {
        let (before, after) = template.split_once("{}").unwrap_or((template, ""));
        let words = |w: &str| StringView::new(w.trim().to_owned()).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW);
        let mut row = div().w_full().flex().flex_row().items_center().gap(px(space));
        if !before.trim().is_empty() {
            row = row.child(words(before));
        }
        row = row.child(link);
        // (What follows the link, its full stop too, set right against it.)
        if !after.trim().is_empty() {
            row = row.child(div().ml(px(if after.starts_with(' ') { 0.0 } else { -space })).child(words(after)));
        }
        row.into_element()
    };
    let mut files = div().w_full().flex().flex_col();
    if host.kept.is_empty() && !host.ssh {
        files = files.child(small(olo_kit::tr!("Folders are kept in step only from a host reached over SSH, which {} isn’t.", host.name).into()));
    } else if host.kept.is_empty() {
        let command = format!("olo sync {}", olo_kit::words::tr("FOLDER"));
        let copy = button("Copy", "host-kept-copy", HostDo::Copy(command.clone()), true);
        let on_show = on_do.clone();
        let here = olo_kit::widgets::LinkButton::new(olo_kit::tr!("Downloads\u{a0}▸ Rex\u{a0}▸ {}", host.name).to_string())
            .size(style::SMALL_SIZE)
            .id("host-kept-show")
            .on_click(move |_, cx| on_show(&HostDo::ShowKept(None), cx));
        // (Command hint directly attached 6 pt under the well with flags in mono;
        // destination sentence 14 pt below that: both Opus & Gemini reviews.)
        let hint_row = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(space))
            .child(StringView::new(olo_kit::words::tr("Add").to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
            .child(typed("--two-way"))
            .child(StringView::new(olo_kit::words::tr("to sync edits both ways, or").to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
            .child(typed("--to-host"))
            .child(StringView::new(olo_kit::words::tr("to push from here.").to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)));
        files = files
            .child(small(olo_kit::tr!("No folders on {} are kept in step here. An agent there keeps one with:", host.name).into()))
            .child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0())
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().child(panels::command_well(StringView::new(command).mono(true).size(MONO_SIZE).into_element())))
                    .child(copy),
            )
            .child(div().h(px(m::TIGHT)).flex_shrink_0())
            .child(hint_row)
            .child(div().h(px(m::SECTION - 2.0)).flex_shrink_0())
            .child(with_link(olo_kit::words::tr("Kept folders land in: {}"), here));
    }
    // Sort kept folders alphabetically by leaf name so rows stay in a stable place
    // rather than jumping as syncs finish (Opus's review).
    let leaf_of = |there: &str| there.trim_end_matches('/').rsplit('/').next().unwrap_or(there).to_owned();
    let bold_font = style::ui_font(olo_kit::FontWeight::BOLD);
    let mut kept_sorted: Vec<&crate::hub::Kept> = host.kept.iter().collect();
    kept_sorted.sort_by(|a, b| leaf_of(&a.there).to_lowercase().cmp(&leaf_of(&b.there).to_lowercase()).then_with(|| a.there.cmp(&b.there)));
    let detail_col = label_w;
    for (i, k) in kept_sorted.into_iter().enumerate() {
        let leaf = leaf_of(&k.there);
        let leaf_w = (f32::from(style::text_width(leaf.clone(), style::PLAIN_SIZE, bold_font.clone())).ceil() + 1.0).min(NAME_MOST);
        let s = kept_stands(k);
        let lamp = if s.busy { panels::ring_of(s.light, panels::LIGHT) } else { panels::light(s.light) };
        // Inline header: bold folder name followed directly by symmetric `·` and state
        // (Opus #3/#4/#5 & Gemini #1). Conflict count is a clickable link to the folder.
        let mut said = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .child(lamp)
            .child(div().w(px(leaf_w)).flex_shrink_0().child(StringView::new(leaf.clone()).bold(true).cut(true)))
            .child(div().flex_shrink_0().child(StringView::new("·").dim(true)));
        if matches!(k.state, crate::hub::KeptState::InStep) && !k.conflicts.is_empty() {
            let conf_label = olo_kit::words::count(k.conflicts.len() as u64, "1 conflict copy", "{n} conflict copies").to_string();
            let on_conf = on_do.clone();
            let there_conf = k.there.clone();
            let conf_link = olo_kit::widgets::LinkButton::new(conf_label)
                .size(style::PLAIN_SIZE)
                .id(format!("host-kept-conflicts-{i}"))
                .on_click(move |_, cx| on_conf(&HostDo::ShowKept(Some(there_conf.clone())), cx));
            said = said
                .child(div().flex_shrink_0().child(StringView::new(s.said.clone()).dim(true)))
                .child(div().flex_shrink_0().child(StringView::new("·").dim(true)))
                .child(div().flex_shrink_0().child(conf_link));
            if let Some((at, _, _)) = k.last {
                let ago = olo_kit::ago::ago(at, olo_kit::ago::long);
                said = said.child(div().flex_1().min_w_0().child(StringView::new(format!("·  {ago}")).dim(true).cut_words(true)));
            }
        } else {
            let state = match &s.more {
                Some(more) => format!("{}  ·  {more}", s.said),
                None => s.said.clone(),
            };
            said = said.child(div().flex_1().min_w_0().child(StringView::new(state).dim(true).cut_words(true)));
        }
        // Fixed button slots: Sync Now / Try Again on left, Forget… / Forget on right
        // (Opus #2: never "Stop" next to "Sync Now").
        let mut actions = Vec::new();
        let gone = k.state == crate::hub::KeptState::Gone;
        if matches!(k.state, crate::hub::KeptState::Failed(_)) {
            actions.push(button("Try Again", &format!("host-kept-again-{i}"), HostDo::RetryKept(k.there.clone()), reached));
        } else if !gone {
            let can_sync = reached && k.state != crate::hub::KeptState::Syncing;
            actions.push(button("Sync Now", &format!("host-kept-sync-{i}"), HostDo::SyncKept(k.there.clone()), can_sync));
        }
        actions.push(button(if gone { "Forget" } else { "Forget…" }, &format!("host-kept-stop-{i}"), HostDo::StopKept(k.there.clone(), !gone), true));
        let line = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(said).children(actions);
        // Calm Here line: parent prefix in secondary text, only the folder leaf in link colour (Opus #3).
        let on_show = on_do.clone();
        let there = k.there.clone();
        let here_prefix = olo_kit::tr!("Downloads\u{a0}▸ Rex\u{a0}▸ {}\u{a0}▸", host.name).to_string();
        let here_leaf =
            olo_kit::widgets::LinkButton::new(leaf.clone()).size(style::SMALL_SIZE).id(format!("host-kept-show-{i}")).on_click(move |_, cx| on_show(&HostDo::ShowKept(Some(there.clone())), cx));
        let here_row = div().w_full().flex().flex_row().items_center().gap(px(space)).child(StringView::new(here_prefix).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW)).child(here_leaf);
        // Sync line: direction in primary text naming the host, modifiers dim, exclude globs in mono (Opus #7/#8).
        let (dir_text, mods, excludes) = kept_mode_parts(k, &host.name);
        let mut sync_row = div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT)).child(StringView::new(dir_text).size(style::SMALL_SIZE).height(style::line_row(style::SMALL_SIZE)));
        for m_word in mods {
            sync_row = sync_row
                .child(StringView::new("·").size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
                .child(StringView::new(m_word).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)));
        }
        if !excludes.is_empty() {
            sync_row = sync_row
                .child(StringView::new("·").size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
                .child(StringView::new(olo_kit::words::tr("excluding").to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
                .child(
                    div().flex_1().min_w_0().child(StringView::new(excludes.join(", ")).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(MONO_SIZE)).beside(style::SMALL_SIZE)),
                );
        }
        let notes = div()
            .w_full()
            .pl(px(under))
            .flex()
            .flex_col()
            .child(labelled(&here_there[0], detail_col, m::LABEL_ROW, here_row.into_element()))
            .child(div().h(px(2.0)).flex_shrink_0())
            .child(labelled(
                &here_there[1],
                detail_col,
                style::line_row(MONO_SIZE),
                StringView::new(k.there.clone()).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(MONO_SIZE)).beside(style::SMALL_SIZE).into_element(),
            ))
            .child(div().h(px(2.0)).flex_shrink_0())
            .child(labelled(&here_there[2], detail_col, style::line_row(style::SMALL_SIZE), sync_row.into_element()));
        if i > 0 {
            files = files.child(div().h(px(m::SECTION - m::TIGHT)).flex_shrink_0());
        }
        files = files.child(div().w_full().flex().flex_col().child(line).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(notes));
    }
    // Equal 20 pt inset on all four sides (measured from the scrollbar's left
    // edge when scrolled: both Opus #6 & Gemini #6).
    let scroll = FILES_SCROLL.with(|h| h.clone());
    let inset = 20.0;
    let files = olo_kit::widgets::ScrollView::new(&scroll)
        .border(false)
        .auto_hide(true)
        .child(
            div().w_full().pl(px(inset)).pr(px(inset)).pt(px(inset)).pb(px(inset)).flex().flex_col().child(section("Folders kept in step")).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(files),
        )
        .into_element();

    // Agent Screens of the host's own, in a tab of their own: Olo put
    // there (its line as every thing's is, with Remove beside Update
    // where it is behind this Olo), and under it where an agent's
    // screen opens when the agent doesn't say.
    let scr_s = thing_stands(host, shown, HostThing::Screens);
    let mut screens_dot: Option<Light> = None;
    if !scr_s.busy && scr_s.light == Light::Down {
        bump_dot(&mut screens_dot, scr_s.light);
    }
    let olo_there = host.screens.as_ref().is_some_and(|s| s.installed.is_some());
    let mut scr_actions = Vec::new();
    match scr_s.can {
        Some((op, true)) => scr_actions.push(button("Try Again", "host-screens-again", HostDo::Change(HostThing::Screens, op), reached)),
        Some((HostOp::Install, false)) => scr_actions.push(button("Install", "host-screens-install", HostDo::Change(HostThing::Screens, HostOp::Install), reached)),
        Some((HostOp::Update, false)) => {
            // (Greyed while screens run there, as its line says: it
            // looked ready beside "Update once … are stopped".)
            let none_run = host.screens.as_ref().is_some_and(|s| s.running.is_empty());
            scr_actions.push(button("Update", "host-screens-update", HostDo::Change(HostThing::Screens, HostOp::Update), reached && none_run));
            scr_actions.push(button("Remove", "host-screens-remove", HostDo::Change(HostThing::Screens, HostOp::Remove), reached));
        }
        Some((HostOp::Remove, false)) => scr_actions.push(button("Remove", "host-screens-remove", HostDo::Change(HostThing::Screens, HostOp::Remove), reached)),
        // (Being put there: the one thing to do is to stop it. Not a
        // greyed copy of the button just pressed, which read as stuck:
        // both reviewers.)
        None if host.screens_doing.is_some() => scr_actions.push(button("Cancel", "host-screens-cancel", HostDo::ScreensCancel, reached)),
        // (Not known yet what is there, or being taken off: no button
        // that guesses it. Both reviewers, of a greyed Install beside
        // "Asking…" that became Remove as the host answered.)
        None if host.screens.is_none() || scr_s.busy => {}
        None => scr_actions.push(button(if olo_there { "Remove" } else { "Install" }, "host-screens-wait", HostDo::Check, false)),
    }
    // (What it does, then how it is updated, a line each, as the Rex
    // server's row says of itself; both sizes said where the choice is
    // made, each by its name: "about 600 MB" alone, beside "2.3 GB"
    // once it was there, was two figures that disagreed. And the host's
    // copy isn't called a desktop, which `--on desktop` below means
    // this computer by. The same under every state. A reviewer's.)
    // (How far putting it there is: the kit's bar, in the place a
    // state's sentence takes, over the description. A reviewer's: "a
    // 600 MB job … shows up only as a dim number inside a sentence".)
    // (Filled in the neutral slate of every meter: the palette's blue
    // is a choice's, and this window's lights have green and amber.)
    let putting = host.screens_doing.as_ref().map(|(_, far)| olo_kit::widgets::StatusBar::new(*far).bar_height(8.0).color(style::neutral_bar(&olo_kit::theme::palette())).into_element());
    let screens_block = block_with(
        // ("Agent Screens": with "Screens on this host" two sections
        // down, "Agent Screens on this host" read as the list's heading.)
        "Agent Screens",
        &scr_s,
        scr_actions,
        putting,
        olo_kit::words::tr("Puts Olo and Chrome on this host so agents’ screens can run on it.").into(),
        olo_kit::words::tr("Updates when you choose: a 600 MB download, 2.2 GB on the host.").into(),
        Some("~/.local/share/olo/desktop"),
    );
    // Where screens open by default: one of two, as chosen there (shown
    // as chosen at the click, until the host has answered). The host's
    // own can't be chosen where a screen couldn't run there, and its
    // note says why not (a greyed choice with its usual words was a
    // puzzle; one chosen already stays as it is, to be seen).
    let on_host = shown.screens_default.unwrap_or_else(|| host.screens.as_ref().is_some_and(|s| s.on_host));
    let helper_there = host.screens.as_ref().is_some_and(|s| s.helper);
    let can_choose = reached && shown.screens_default.is_none() && helper_there;
    let could_run = olo_there && host.screens.as_ref().is_some_and(|s| s.lacking().is_none());
    let host_note = match &host.screens {
        Some(s) if !s.helper => "Needs the typing helper first.",
        // (While it is being put there, Install is Cancel: no note that
        // names a button that isn't there. Gemini 4 Neon's look.)
        Some(s) if s.installed.is_none() && host.screens_doing.is_some() => "You can choose this once Agent Screens are installed.",
        Some(s) if s.installed.is_none() => "Install Agent Screens above to choose this.",
        Some(s) if s.lacking().is_some() => "Agent Screens can’t run on this host.",
        _ => "Keeps working with this computer off. You can’t watch it from here.",
    };
    // (On this computer is the bridge's to allow: said where it isn't.)
    let desktop_note = if connected && !host.granted.contains(&"Agent Screens") {
        "Off in this host’s permissions (the Olo Integration tab)."
    } else {
        "You can watch it and take over. Needs this computer on and connected."
    };
    let (to_desktop, to_host) = (on_do.clone(), on_do.clone());
    // (Under the choices, at their words' edge: at the section's own it
    // read as a third choice that had lost its button. In the choices'
    // order, each flag with what it means: a reviewer's.)
    let option_inset = RadioButton::LABEL_INSET;
    let said = |words: &'static str| StringView::new(olo_kit::words::tr(words).to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE));
    let where_block = div()
        .w_full()
        .flex()
        .flex_col()
        .child(section("Where screens open by default"))
        // (As far under its rule as the status line is under its own.)
        .child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0())
        .child(
            div().w_full().pr(px(short)).child(
                RadioButton::new("On this computer", !on_host)
                    .id("host-screens-desktop")
                    .enabled(can_choose)
                    .on_select(move |_, cx| to_desktop(&HostDo::ScreensDefault(false), cx))
                    .note(olo_kit::words::tr(desktop_note)),
            ),
        )
        .child(div().h(px(m::TIGHT)).flex_shrink_0())
        .child(
            div().w_full().pr(px(short)).child(
                RadioButton::new("On the host itself", on_host)
                    .id("host-screens-host")
                    .enabled(can_choose && (could_run || on_host))
                    .on_select(move |_, cx| to_host(&HostDo::ScreensDefault(true), cx))
                    .note(olo_kit::words::tr(host_note)),
            ),
        )
        .child(div().h(px(m::SPACING + 2.0)).flex_shrink_0())
        .child(
            div()
                .w_full()
                .pl(px(option_inset))
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(space))
                .child(said("An agent can say otherwise:"))
                .child(typed("olo screen --on desktop"))
                .child(said("(this computer) or"))
                .child(typed("--on host"))
                .child(div().ml(px(-space - 1.0)).child(said("."))),
        )
        .into_element();
    // The host's screens, last: a list in a well as the window's other
    // lists are, built by the window (`screens_section`), which keeps
    // its rows and which is chosen. There only where Olo is.
    let mut screens = vec![screens_block, where_block];
    if let Some(list) = screens_list {
        screens.push(list);
    }

    // (Every tab's page laid out, the panel as tall as the taller of Rex's
    // and Olo's, so the window keeps its height and its foot its place
    // from tab to tab: both looks. The folders' list fills that room and
    // scrolls in it.)
    let tab = shown.tab.min(crate::host::TWEAKS);
    let on_tab = on_do.clone();
    let mut files_dot: Option<Light> = None;
    for k in &host.kept {
        let ks = kept_stands(k);
        if !ks.busy && matches!(ks.light, Light::Down | Light::Waiting) {
            bump_dot(&mut files_dot, ks.light);
        }
    }
    let names: Vec<SharedString> = ["Rex", "Olo Integration", "Files", "Screens", "Tweaks"].into_iter().map(SharedString::from).collect();
    // The machine's own settings, last: what root found, each a line
    // as the window's other things are; or the alert of the stage the
    // work is at, in the page.
    let tweaks_dot = tweaks.and_then(|t| matches!(t.stage, crate::server::Stage::Failed { .. }).then_some(Light::Down));
    let (tweaks_page, tweaks_fills) = tweaks_page(host, managed, tweaks, phase, wide, &on_do);
    let page_of = |things: Vec<Element>| div().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(things).into_element();
    let mut tabs = TabView::new("host-tabs", names, tab)
        .lights([rex_dot, olo_dot, files_dot, screens_dot, tweaks_dot])
        .grows()
        .on_select(move |i: &usize, cx| on_tab(&HostDo::Tab(*i), cx))
        .pages([page_of(rex), page_of(olo), files, page_of(screens), tweaks_page])
        .fill_page(2);
    // (Tweaks, while it is an alert: the panel to its frame.)
    if tweaks_fills {
        tabs = tabs.fill_page(crate::host::TWEAKS);
    }
    page.push(tabs.into_element());

    // Everything of Olo's off the host at once: the one thing here
    // that asks first, in its own place at the window's foot.
    let foot = if shown.asking_all {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(div().flex_1().min_w_0().child(StringView::new(olo_kit::words::tr("Remove the desktop commands, skill and typing helper?")).cut(true)))
            .child(button("Cancel", "host-all-cancel", HostDo::AskAll(false), true).default(true))
            .child(button("Remove All", "host-all-remove", HostDo::RemoveAll(there.clone()), reached && !busy && !there.is_empty()))
    } else {
        // Footer stays host-lifecycle actions (`Remove All…` / `Move to Another Host…`)
        // in every state; `Update All` lives beside `Check Again` in the header.
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(button("Remove All…", "host-all", HostDo::AskAll(true), reached && !busy && !there.is_empty()))
            .child(div().flex_1())
            .child(button("Move to Another Host…", "host-move", HostDo::Move, reached && !busy))
    };
    // (Under the tabs' panel, whose edge is its rule.)
    page.push(foot.into_element());

    // (A section's step between them, as the Server window's have.)
    div().w_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(page).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostStands {
        HostStands {
            kept: Vec::new(),
            name: "far".into(),
            address: "you@far.example".into(),
            conn: HostConnState::Connected,
            ssh: true,
            helper: HostHelper::None,
            helper_current: None,
            helper_sync_err: None,
            helper_kept_off: false,
            helper_setting: true,
            bridge: Some(false),
            granted: Vec::new(),
            agent_keys: None,
            skill: Some(false),
            skill_current: None,
            skill_sync_err: None,
            commands: Some(false),
            commands_current: None,
            commands_sync_err: None,
            screens: None,
            screens_doing: None,
            this_olo: None,
            pool: None,
            version: Some("0.19.2".into()),
            server_newer: None,
            account: None,
            carried: None,
            sessions: None,
            sessions_window: Default::default(),
            repos: Vec::new(),
            repos_there: Some(Vec::new()),
        }
    }

    fn said(s: &Stands) -> (Light, &str, Option<&str>) {
        (s.light, s.said.as_str(), s.more.as_deref())
    }

    /// Agent Screens of a host's own, on their line: asked, a helper
    /// that hasn't them, not there, there (with what, and which screens
    /// run), behind this Olo (the version alone, with Update beside
    /// Remove), lacking something (why, as a sentence), being put there
    /// (how far), and what couldn't be done as Olo's own sentence. Never
    /// among what Remove All… takes off or Update All updates.
    #[test]
    fn a_hosts_own_screens_say_how_they_stand() {
        use crate::host_screens::ScreensThere;
        let (mut h, mut shown) = (host(), Shown { key: "k".into(), name: "far".into(), ..Shown::default() });
        let of = |h: &HostStands, shown: &Shown| thing_stands(h, shown, HostThing::Screens);
        assert_eq!((said(&of(&h, &shown)), of(&h, &shown).busy), ((Light::Waiting, "Asking…", None), true));
        h.screens = Some(ScreensThere::default());
        let s = of(&h, &shown);
        // (A helper from before it had screens. This Olo carries one to put
        // there or not, by how it was built: with one, Install does both.)
        assert_eq!(said(&s), (Light::Off, "Not installed", Some("needs the typing helper first")));
        assert_eq!(s.can.is_some(), crate::link::carried() && h.helper_setting, "{:?}", s.which);
        h.screens = Some(ScreensThere { helper: true, software: true, ..Default::default() });
        let s = of(&h, &shown);
        assert_eq!((said(&s), s.can, s.there), ((Light::Off, "Not installed", None), Some((HostOp::Install, false)), false));

        let all = ScreensThere {
            helper: true,
            software: true,
            installed: Some("2026.11.26-c2a7db3c".into()),
            committed: Some(200),
            chrome: Some("158.0.8127.88".into()),
            bytes: 2_394_000_000,
            ..Default::default()
        };
        h.screens = Some(all.clone());
        let s = of(&h, &shown);
        assert_eq!((said(&s), s.can), ((Light::On, "Installed", Some("Olo 2026.11.26, Chrome 158, 2.2 GB")), Some((HostOp::Remove, false))));
        assert_eq!((s.there, s.behind, s.which.clone()), (false, false, None), "not Remove All's, nor Update All's");
        h.screens = Some(ScreensThere { running: vec!["review".into()], ..all.clone() });
        assert_eq!(of(&h, &shown).which.as_deref(), Some("1 screen running: review."));
        // (Where the host listed its screens, the list under the line
        // says which run: once. The list's rows, running first as the
        // host's answer was read, each with what is being done to it.)
        use crate::host_screens::ScreenRow;
        let row = |name: &str, running| ScreenRow { name: name.into(), running, ..Default::default() };
        h.screens = Some(ScreensThere { running: vec!["review".into()], rows: vec![row("review", true), row("notes", false)], ..all.clone() });
        assert_eq!(of(&h, &shown).which, None);
        shown.screen_doing.push(("review".into(), ScreenDo::Stop));
        let lines = screen_lines(&h, &shown);
        assert_eq!(lines.iter().map(|l| (l.row.name.as_str(), l.doing)).collect::<Vec<_>>(), [("review", Some(ScreenDo::Stop)), ("notes", None)]);
        shown.screen_doing.clear();
        // (No list where Olo isn't there: nothing to list.)
        let none = HostStands { screens: Some(ScreensThere { helper: true, software: true, rows: vec![row("left-over", false)], ..Default::default() }), ..h.clone() };
        assert!(screen_lines(&none, &shown).is_empty());
        h.screens = Some(ScreensThere { running: vec!["review".into()], ..all.clone() });
        // This Olo's release is the later: an update, the version alone on the line.
        // (What is there and what updating brings; and with a screen
        // running, that the update waits for it, before it is pressed.)
        h.this_olo = Some((300, "2026.11.27-0a1b2c3d".into()));
        let s = of(&h, &shown);
        assert_eq!((said(&s), s.can, s.behind), ((Light::Waiting, "Update available", Some("2026.11.26 → 2026.11.27")), Some((HostOp::Update, false)), false));
        assert_eq!(s.which.as_deref(), Some("1 screen running: review. Update once they are stopped."));
        // (The same release, or a host's later one: none.)
        h.this_olo = Some((200, "2026.11.26-c2a7db3c".into()));
        assert_eq!(said(&of(&h, &shown)).1, "Installed");
        h.this_olo = Some((100, "2026.11.25-00000000".into()));
        assert_eq!(said(&of(&h, &shown)).1, "Installed");

        h.screens = Some(ScreensThere { missing: vec!["libEGL_mesa.so.0".into()], software: false, ..all.clone() });
        let s = of(&h, &shown);
        assert_eq!((s.light, s.said.as_str(), s.can, s.keeps_note), (Light::Down, "Can’t run", Some((HostOp::Remove, false)), true));
        assert!(s.why.as_deref().is_some_and(|why| why.contains("libEGL_mesa.so.0") && why.contains("Check Again")), "{:?}", s.why);

        // Being put there: the hub's own word of how far, whatever the window was doing.
        h.screens = Some(ScreensThere { helper: true, software: true, ..Default::default() });
        h.screens_doing = Some(("Downloading Olo".into(), 0.21));
        let s = of(&h, &shown);
        assert_eq!((said(&s), s.busy, s.can), ((Light::Waiting, "Being installed…", Some("Downloading Olo")), true, None));
        h.screens = Some(all.clone());
        assert_eq!(said(&of(&h, &shown)).1, "Being updated…");
        h.screens_doing = None;
        // What couldn't be done: Olo's own sentence under the line, and Try Again.
        // (A sentence: with its full stop, where what was said had none.)
        shown.failed.push((HostThing::Screens, HostOp::Update, "GitHub couldn’t be reached: timed out".into()));
        let s = of(&h, &shown);
        assert_eq!(
            (s.light, s.said.as_str(), s.why.as_deref(), s.host_said.clone(), s.can),
            (Light::Down, "Couldn’t update", Some("GitHub couldn’t be reached: timed out."), None, Some((HostOp::Update, true)))
        );
        shown.failed = vec![(HostThing::Screens, HostOp::Remove, "Still running: review. Stop it, then try again.".into())];
        let s = of(&h, &shown);
        assert_eq!((s.why.as_deref(), s.keeps_note), (Some("Still running: review. Stop it, then try again."), true));
        // The typing helper's own line says what runs through it, where Olo is there for screens.
        h.helper = HostHelper::Used;
        h.helper_current = Some(true);
        assert_eq!(thing_stands(&h, &Shown::default(), HostThing::Helper).which.as_deref(), Some("Agent Screens on this host run through it."));
    }

    /// Each line says how its thing stands, in one set of words, in
    /// every state it has: what is being done to a thing before what
    /// couldn't be, and that before how the host last said it stands;
    /// and its button is what can be done to it then.
    #[test]
    fn each_thing_says_how_it_stands_and_what_can_be_done() {
        let (mut h, mut shown) = (host(), Shown { key: "k".into(), name: "far".into(), ..Shown::default() });
        let of = |h: &HostStands, shown: &Shown, thing| thing_stands(h, shown, thing);

        // The bridge: off with nothing granted, on with something;
        // nothing of it is installed, so nothing to install or remove.
        let s = of(&h, &shown, HostThing::Bridge);
        assert_eq!((said(&s), s.there, s.can), ((Light::Off, "Off", Some("nothing granted")), false, None));
        h.granted = vec!["links", "Passwords"];
        let s = of(&h, &shown, HostThing::Bridge);
        assert_eq!((said(&s), s.there, s.can), ((Light::On, "On", Some("links, Passwords")), false, None));
        // (Granted to a host that isn't connected: said, with no light.)
        let away = HostStands { conn: HostConnState::Disconnected, ..h.clone() };
        assert_eq!(said(&of(&away, &shown, HostThing::Bridge)), (Light::Off, "On", Some("links, Passwords")));
        // The SSH keys: by how many the host was offered; where none,
        // why, under the line, and the light amber. (Not asked yet:
        // said as granted.)
        let keys = |offered: Option<Result<Vec<String>, String>>| {
            let s = of(&HostStands { granted: vec!["links", "SSH keys"], agent_keys: offered, ..h.clone() }, &shown, HostThing::Bridge);
            (said(&s).0, s.more.clone(), s.why.clone(), s.which.clone())
        };
        assert_eq!(keys(None), (Light::On, Some("links, SSH keys".into()), None, None));
        // (Which, by name, under the line.)
        let named = |names: &[&str]| Some(Ok(names.iter().map(|n| n.to_string()).collect()));
        assert_eq!(keys(named(&["GitHub", "Work laptop"])), (Light::On, Some("links, 2 SSH keys".into()), None, Some("SSH keys offered: GitHub, Work laptop.".into())));
        assert_eq!(keys(named(&["GitHub"])).1.as_deref(), Some("links, 1 SSH key"));
        assert_eq!(keys(named(&[])), (Light::Waiting, Some("links, no SSH keys".into()), Some("Passwords has no SSH keys to offer.".into()), None));
        assert_eq!(keys(Some(Err("Passwords isn’t running".into()))), (Light::Waiting, Some("links, no SSH keys".into()), Some("Passwords isn’t running.".into()), None));
        // (A host not connected: nothing under the line.)
        let away = HostStands { conn: HostConnState::Disconnected, granted: vec!["links", "SSH keys"], agent_keys: named(&["GitHub"]), ..h.clone() };
        assert_eq!(of(&away, &shown, HostThing::Bridge).which, None);

        // The skill: asked, not there, up to date, update available, and update failed.
        h.skill = None;
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can), ((Light::Waiting, "Asking…", None), None));
        h.skill = Some(false);
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can), ((Light::Off, "Not installed", None), Some((HostOp::Install, false))));
        h.skill = Some(true);
        h.skill_current = Some(true);
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.there, s.behind, s.can), ((Light::On, "Up to date", None), true, false, Some((HostOp::Remove, false))));
        h.skill_current = Some(false);
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.there, s.behind, s.host_said.as_deref()), ((Light::Waiting, "Update available", None), true, true, None));
        h.skill_sync_err = Some("read-only".into());
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.there, s.behind, s.host_said.as_deref()), ((Light::Down, "Update failed", None), true, true, Some("read-only")));
        h.skill = Some(false);
        h.skill_current = None;
        h.skill_sync_err = None;

        // The Token Pool, as this computer's own says: not running
        // here, none there, being installed, there and up to date,
        // there and update available, there and can't sync (Olo prose in `why`), couldn't install.
        use crate::pool::PoolStands;
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.can, s.there), ((Light::Off, "Not known", Some("Token Pool isn’t running on this computer")), None, false));
        h.pool = Some(PoolStands::default());
        assert_eq!(said(&of(&h, &shown, HostThing::Pool)), (Light::Off, "Not installed", None));
        h.pool = Some(PoolStands { doing: Some("Installing".into()), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.busy), ((Light::Waiting, "Being installed…", None), true));
        h.pool = Some(PoolStands { wanted: true, in_step: true, accounts: Some(2), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        // (Never one of what Remove All… takes off: its accounts are the Token Pool's to move.)
        assert_eq!((said(&s), s.there, s.behind, s.can), ((Light::On, "Up to date", Some("2 accounts")), false, false, None));
        h.pool = Some(PoolStands { wanted: true, in_step: false, accounts: Some(1), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.behind, s.why.as_deref()), ((Light::Waiting, "Update available", Some("1 account")), true, None));
        h.pool = Some(PoolStands { wanted: true, error: Some("no route".into()), accounts: Some(1), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.behind, s.why.as_deref()), ((Light::Down, "Can’t sync", Some("1 account")), true, Some("no route")));
        h.pool = Some(PoolStands { error: Some("no room".into()), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.why.as_deref(), s.can), ((Light::Down, "Couldn’t install", None), Some("no room"), None));

        // The helper: in use (up to date & outdated), for the next connection, taken off while
        // in use, couldn't be put there (the host's words kept), off.
        h.helper = HostHelper::Used;
        h.helper_current = Some(true);
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.there, s.behind), ((Light::On, "Up to date", Some("in use")), true, false));
        h.helper_current = Some(false);
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.there, s.behind), ((Light::Waiting, "Update available", Some("older build in use")), true, true));
        h.helper_current = Some(true);
        h.helper_kept_off = true;
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.there, s.can, s.busy), ((Light::Waiting, "Not installed", Some("this connection still uses it")), false, Some((HostOp::Install, false)), false));
        h.helper_kept_off = false;
        h.helper = HostHelper::Ready;
        assert_eq!(said(&of(&h, &shown, HostThing::Helper)), (Light::On, "Up to date", Some("used the next time you connect")));
        h.helper = HostHelper::Without("no room".into());
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.host_said.as_deref(), s.can), ((Light::Down, "Couldn’t install", None), Some("no room"), Some((HostOp::Install, true))));
        h.helper = HostHelper::None;
        h.helper_setting = false;
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.can), ((Light::Off, "Not installed", Some("turned off for every host")), None));

        // What couldn't be done: the red light, the host's words, and
        // the same again to try; while it is tried, that it is.
        shown.failed.push((HostThing::Skill, HostOp::Remove, "rm: busy".into()));
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.host_said.as_deref(), s.can), ((Light::Down, "Couldn’t remove", None), Some("rm: busy"), Some((HostOp::Remove, true))));
        shown.doing.push((HostThing::Skill, HostOp::Update));
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can, s.busy), ((Light::Waiting, "Being updated…", None), None, true));

        // A host that isn't connected: nothing is known, nothing to press.
        h.conn = HostConnState::Disconnected;
        for thing in [HostThing::Skill, HostThing::Helper, HostThing::Pool] {
            let s = of(&h, &Shown::default(), thing);
            assert_eq!((said(&s), s.can), ((Light::Off, "Not known", None), None));
        }
        assert_eq!(said(&server_stands(&h)), (Light::Off, "Not known", None));
        h.conn = HostConnState::Connected;
        assert_eq!(said(&server_stands(&h)), (Light::On, "Running", Some("0.19.2")));
        h.server_newer = Some("0.19.5".into());
        assert_eq!(said(&server_stands(&h)), (Light::Waiting, "Update available", Some("0.19.2 → 0.19.5  ·  running")));

        // Kept folders: direction summaries and conflict amber state.
        use crate::hub::{Kept, KeptState};
        use olo_protocol::RexSyncDirection as Dir;
        let base_kept = Kept {
            host_key: "k".into(),
            there: "/home/dev/notes".into(),
            mirror: false,
            direction: Dir::Both,
            shallow: false,
            exclude: vec!["*.tmp".into()],
            conflicts: vec!["plan.conflict-far-20261125-100000.md".into()],
            state: KeptState::InStep,
            last: None,
        };
        let ks = kept_stands(&base_kept);
        assert_eq!(said(&ks), (Light::Waiting, "In step", Some("1 conflict copy")));
        assert_eq!(kept_mode_parts(&base_kept, "dev-box"), ("Both ways".into(), vec![], vec!["*.tmp".into()]));
        let to_host = Kept { direction: Dir::ToHost, mirror: true, shallow: true, exclude: vec![], conflicts: vec![], ..base_kept.clone() };
        assert_eq!(said(&kept_stands(&to_host)), (Light::On, "In step", None));
        assert_eq!(kept_mode_parts(&to_host, "dev-box"), ("To dev-box".into(), vec!["mirrored".into(), "top level only".into()], vec![]));
        let syncing = Kept { state: KeptState::Syncing, conflicts: vec![], ..base_kept.clone() };
        let ss = kept_stands(&syncing);
        assert_eq!((said(&ss), ss.busy), ((Light::On, "Syncing…", None), true));
        let gone = Kept { state: KeptState::Gone, conflicts: vec![], ..base_kept };
        assert_eq!(said(&kept_stands(&gone)), (Light::Off, "Gone from the host", None));
    }

    #[test]
    fn rex_server_limits_row_says_short_pending_restart_met_and_stale() {
        use olo_rex_client::tune::{RexLimitValues, Tune, Tuning};
        let base = "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run --log-format text\nrex.v1.stale.system=0\n";
        let short = Tuning::read(&format!("{base}rex.v1.loaded.system=/usr/bin/rex server run --log-format text\n"));
        assert_eq!(tweak_state(&short, Tune::RexLimits), (Light::Off, "64 tabs now, 512 suggested".into()));
        let partial = Tuning::read(&format!("{base}rex.v1.loaded.system=/usr/bin/rex server run --log-format text --max-windows 512 --max-blocks 4096\n"));
        assert_eq!(tweak_state(&partial, Tune::RexLimits), (Light::Off, "64 panes now, 128 suggested".into()));
        // Waiting for restart: state word first (`applied` / `put back`), then what holds after restart.
        let with_run = |loaded: &str, running: RexLimitValues, kept: bool| {
            let mut t = Tuning::read(&format!(
                "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system={}\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
                 rex.v1.unit.system=/usr/bin/rex server run --log-format text\nrex.v1.loaded.system={loaded}\nrex.v1.stale.system=0\n",
                usize::from(kept)
            ));
            t.rex_limits.running = Some(running);
            t
        };
        let wanted_cmd = "/usr/bin/rex server run --log-format text --max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096";
        assert_eq!(tweak_state(&with_run(wanted_cmd, RexLimitValues::defaults(), true), Tune::RexLimits), (Light::Waiting, "applied, 512 tabs after Rex restarts".into()));
        // Put Back pending decrease while server still runs at 512 tabs:
        assert_eq!(
            tweak_state(&with_run("/usr/bin/rex server run --log-format text", RexLimitValues::wanted(), false), Tune::RexLimits),
            (Light::Waiting, "put back, 64 tabs after Rex restarts".into())
        );
        // Unlimited tabs pending restart (both directions):
        assert_eq!(
            tweak_state(&with_run("/usr/bin/rex server run --log-format text --max-windows 0 --max-panes 128 --max-sessions 128 --max-blocks 4096", RexLimitValues::defaults(), true), Tune::RexLimits),
            (Light::Waiting, "applied, no limit on tabs after Rex restarts".into())
        );
        assert_eq!(
            tweak_state(&with_run("/usr/bin/rex server run --log-format text", RexLimitValues { windows: u64::MAX, ..RexLimitValues::defaults() }, false), Tune::RexLimits),
            (Light::Waiting, "put back, 64 tabs after Rex restarts".into())
        );
        assert_eq!(
            tweak_state(&with_run(wanted_cmd, RexLimitValues { windows: 512, ..RexLimitValues::defaults() }, true), Tune::RexLimits),
            (Light::Waiting, "applied, 128 panes after Rex restarts".into())
        );
        assert_eq!(
            tweak_state(&with_run(wanted_cmd, RexLimitValues { windows: 512, panes: 128, ..RexLimitValues::defaults() }, true), Tune::RexLimits),
            (Light::Waiting, "applied, 128 sessions after Rex restarts".into())
        );
        assert_eq!(
            tweak_state(&with_run(wanted_cmd, RexLimitValues { windows: 512, panes: 128, sessions: 128, blocks: 1024 }, true), Tune::RexLimits),
            (Light::Waiting, "applied, 4,096 blocks after Rex restarts".into())
        );
        assert_eq!(tweak_state(&with_run(wanted_cmd, RexLimitValues::wanted(), true), Tune::RexLimits), (Light::On, "applied".into()));

        // Blocked ("not here") state: grey light, "can’t apply here", excluded from Apply Suggested.
        let blocked = Tuning::read(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=blocked\n\
             rex.v1.why.system=/etc/systemd/system/rex.service.d/20-site.conf sets this server’s command: raise its limits there.\n",
        );
        assert_eq!(tweak_state(&blocked, Tune::RexLimits), (Light::Off, "can’t apply here".into()));
        assert!(!tweaks_suggested(&blocked).contains(&Tune::RexLimits));

        let stale = Tuning::read(&format!(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
             rex.v1.unit.system=/usr/bin/rex server run --log-format text\nrex.v1.loaded.system={wanted_cmd}\n\
             rex.v1.pkg.system=/usr/bin/rex server run --old\nrex.v1.stale.system=1\n"
        ));
        assert_eq!(tweak_state(&stale, Tune::RexLimits), (Light::Waiting, "no longer applied".into()));
        // Two-or-more rule for "Apply Suggested…": hidden for 0 or 1 Heavy work rows, shown for 2+.
        let all_met = "ram 65536000\nMaxSessions 1024\nopen files 1048576\nDefaultLimitNOFILE 1048576\nDefaultTasksMax infinity\nfs.file-max 9223372036854775807\nfs.inotify.max_user_watches 1048576\n\
            fs.inotify.max_user_instances 8192\nfs.inotify.max_queued_events 65536\nfs.aio-max-nr 1048576\nkernel.pid_max 4194304\nvm.max_map_count 1048576\nvm.dirty_background_bytes 8589934592\n\
            vm.dirty_bytes 33554432000\nvm.dirty_expire_centisecs 6000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\ntemporary files 1\n\
            rex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.user=0\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run --log-format text\n\
            rex.v1.loaded.system=/usr/bin/rex server run --log-format text --max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096\nrex.v1.stale.system=0\nbrush 0\n";
        assert_eq!(tweaks_suggested(&Tuning::read(all_met)).len(), 0);
        // Only RexLimits stale (+ Brush short, which is in Sign-in shell): still 1 Heavy work row -> hidden.
        let one_stale = all_met.replace("rex.v1.stale.system=0", "rex.v1.pkg.system=/usr/bin/rex server run --old\nrex.v1.stale.system=1");
        assert_eq!(tweaks_suggested(&Tuning::read(&one_stale)), vec![Tune::RexLimits]);
        // Two Heavy work rows short -> shown.
        let two_short = one_stale.replace("MaxSessions 1024", "MaxSessions 10");
        assert_eq!(tweaks_suggested(&Tuning::read(&two_short)), vec![Tune::Sessions, Tune::RexLimits]);
    }
}
