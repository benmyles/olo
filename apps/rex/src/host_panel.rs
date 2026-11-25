//! The Host window's drawing: everything of Olo's on a far host, in
//! one place (`host` is which host and what is being done to it; the
//! hub knows what the host has, `HubState::host_stands`).
//!
//! At its head, the host: its icon, its name, where it is and how its
//! connection stands, and Check Again (something may have been changed
//! there by hand). Then one section for each thing, as the Server
//! window has one for each service, in the order one rests on another:
//! the host's Rex server (which has a window of its own for the rest:
//! Manage…), Olo's bridge, the skill for agents, Olo's typing helper.
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
    widgets::{Button, StringView, TabView, button_width, separator},
};

use crate::host::Shown;
use crate::hub::{HostConnState, HostHelper, HostStands, HostThing};
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
    /// `thing` put on the host (`true`), or taken off.
    Change(HostThing, bool),
    /// The host asked again what it has.
    Check,
    /// A host that isn't connected, connected to (Check Again, there).
    Connect,
    /// The host's Rex server, in its own window.
    Server,
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
    /// A tab chosen: Rex, Olo Integration, Files.
    Tab(usize),
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
    /// What its button does, if it can be pressed now: put it there
    /// (`true`) or take it off, and whether that is a second try.
    pub can: Option<(bool, bool)>,
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
    Stands { light, said: olo_kit::words::tr(said).into(), more: None, host_said: None, why: None, which: None, busy: false, there: false, can: None }
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
    if let Some(add) = shown.doing(thing) {
        return Stands { busy: true, ..stands(Light::Waiting, if add { "Being installed…" } else { "Being removed…" }) };
    }
    if !reached {
        return stands(Light::Off, "Not known");
    }
    if let Some((add, said)) = shown.failed(thing) {
        return Stands { host_said: Some(said.to_owned()), can: Some((add, true)), ..stands(Light::Down, if add { "Couldn’t install" } else { "Couldn’t remove" }) };
    }
    let installed = |s: Stands| Stands { there: true, can: Some((false, false)), ..s };
    let absent = |s: Stands| Stands { can: Some((true, false)), ..s };
    match thing {
        HostThing::Helper => match &host.helper {
            HostHelper::Installing => Stands { busy: true, ..stands(Light::Waiting, "Being installed…") },
            // (Taken off while a connection goes through it: that one
            // goes on through it, a program running.)
            HostHelper::Used if host.helper_kept_off => absent(more(stands(Light::Waiting, "Not installed"), "this connection still uses it")),
            HostHelper::Used => installed(more(stands(Light::On, "Installed"), "in use")),
            HostHelper::Ready => installed(more(stands(Light::On, "Installed"), "used the next time you connect")),
            HostHelper::Without(why) => Stands { host_said: Some(why.clone()), can: Some((true, true)), ..stands(Light::Down, "Couldn’t install") },
            HostHelper::None if !host.helper_setting => more(stands(Light::Off, "Not installed"), "turned off for every host"),
            HostHelper::None if !crate::link::carried() => stands(Light::Off, "Not installed"),
            HostHelper::None => absent(stands(Light::Off, "Not installed")),
        },
        HostThing::Bridge => unreachable!("answered above"),
        // (As this computer's Token Pool says, which puts it there and
        // keeps its accounts in step: `pool`.)
        // (Never "there" for Remove All…, and with no Install or Remove
        // of this window's: its accounts are the Token Pool's to move.)
        HostThing::Pool => {
            let accounts = |pool: &crate::pool::PoolStands, then: &'static str| -> String {
                let then = olo_kit::words::tr(then);
                match pool.accounts {
                    Some(1) => format!("{}, {then}", olo_kit::words::tr("1 account")),
                    Some(n) => format!("{}, {then}", olo_kit::tr!("{} accounts", n)),
                    None => then.to_string(),
                }
            };
            match &host.pool {
                None => more(stands(Light::Off, "Not known"), "Token Pool isn’t running on this computer"),
                Some(pool) if pool.doing.is_some() => Stands { busy: true, ..stands(Light::Waiting, if pool.wanted { "Being updated…" } else { "Being installed…" }) },
                Some(pool) if pool.wanted && pool.in_step => Stands { more: Some(accounts(pool, "up to date")), ..stands(Light::On, "Installed") },
                Some(pool) if pool.wanted => Stands { more: Some(accounts(pool, "behind")), host_said: pool.error.clone(), ..stands(Light::Waiting, "Installed") },
                Some(pool) if pool.error.is_some() => Stands { host_said: pool.error.clone(), ..stands(Light::Down, "Couldn’t install") },
                Some(_) => stands(Light::Off, "Not installed"),
            }
        }
        HostThing::Skill => match host.skill {
            Some(true) => installed(stands(Light::On, "Installed")),
            Some(false) => absent(stands(Light::Off, "Not installed")),
            None => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
        },
        HostThing::Commands => match host.commands {
            Some(true) => installed(stands(Light::On, "Installed")),
            Some(false) => absent(stands(Light::Off, "Not installed")),
            None => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
        },
    }
}

/// The Rex server's line: running, and its version.
fn server_stands(host: &HostStands) -> Stands {
    if host.conn == HostConnState::Connected { Stands { more: host.version.clone(), ..stands(Light::On, "Running") } } else { stands(Light::Off, "Not known") }
}

/// A section's heading, over its rule (the Server window's).
fn section(title: &'static str) -> Element {
    // (A literal, looked up in the person's language: a `String` is shown
    // as it is.)
    div().w_full().flex().flex_col().child(StringView::new(title).bold(true).height(m::LABEL_ROW)).child(div().h(px(2.0)).flex_shrink_0()).child(separator()).into_element()
}

/// The window's content for `host`: `managed` whether its Rex server
/// can be looked after from here.
pub fn render_host_panel(host: &HostStands, shown: &Shown, managed: bool, on_do: impl Fn(&HostDo, &mut WindowCx) + 'static) -> Element {
    let on_do: Rc<dyn Fn(&HostDo, &mut WindowCx)> = Rc::new(on_do);
    // Every button one width, the widest of their words: one column
    // down the window's right side.
    let wide = ["Check Again", "Connect", "Manage…", "Permissions…", "Token Pool…", "Install", "Remove", "Try Again", "Remove All…", "Cancel", "Sync Now", "Forget…", "Forget", "Copy"]
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
    // A thing's line and what is under it; `named`: the line says what it
    // is (one with no heading of its own, under another's).
    let body = |s: &Stands, actions: Vec<Button>, note: String, path: Option<&str>, named: Option<&str>, link: Option<Element>| {
        let lamp = if s.busy { panels::busy_of(panels::LIGHT) } else { panels::light(s.light) };
        let mut said = div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT)).child(lamp);
        // (Its state first and then what it is, as every line here reads:
        // Opus's review.)
        said = said.child(StringView::new(s.said.clone()));
        if let Some(name) = named {
            said = said.child(div().flex_shrink_0().child(StringView::new(format!("·  {name}")).dim(true)));
        }
        if let Some(more) = &s.more {
            said = said.child(div().flex_1().min_w_0().child(StringView::new(format!("·  {more}")).dim(true).cut(true)));
        }
        let line = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(said).children(actions);
        let mut block = div().w_full().flex().flex_col().child(line);
        // (What the host said, as it said it, under the line that
        // says it couldn't: a line a line, a long one's rest set in.)
        if let Some(host_said) = &s.host_said {
            let lines: Vec<String> = host_said.lines().filter(|l| !l.trim().is_empty()).flat_map(|line| crate::server_panel::hung(line, room)).collect();
            // (As far under the line's button as the note is under
            // the well: ten points each, as they measure.)
            let lines = div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(MONO_SIZE).wrap(true)));
            block = block.child(div().h(px(WELL_OVER)).flex_shrink_0()).child(div().w_full().pl(px(under)).child(panels::well(lines.into_element())));
        }
        // (The note, and where the thing is on the host as its second
        // line: a point smaller, in a line's own height, so a section
        // that ends with one is as far from the next as one that ends
        // with its note.)
        let mut notes = div().w_full().pl(px(under)).pr(px(short)).flex().flex_col();
        // (A reason, a little apart from the description under it: at
        // the same size and indent they read as one paragraph that
        // changed colour halfway, both looks said.)
        if let Some(why) = &s.why {
            notes = notes.child(StringView::new(why.clone()).size(style::SMALL_SIZE).wrap(true)).child(div().h(px(4.0)).flex_shrink_0());
        }
        // (Which, as apart: dim, as it is no trouble.)
        if let Some(which) = &s.which {
            notes = notes.child(small(which.clone())).child(div().h(px(4.0)).flex_shrink_0());
        }
        if !note.is_empty() {
            notes = notes.child(small(note));
        }
        if let Some(link) = link {
            notes = notes.child(link);
        }
        if let Some(path) = path {
            notes = notes.child(StringView::new(path.to_owned()).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(MONO_SIZE)));
        }
        block.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(notes)
    };
    let block = |title: &'static str, s: &Stands, actions: Vec<Button>, note: String, path: Option<&str>| {
        div().w_full().flex().flex_col().child(section(title)).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(body(s, actions, note, path, None, None)).into_element()
    };
    let reached = host.conn == HostConnState::Connected && host.ssh;

    // The host itself, at the window's head.
    // (How its connection stands in the words' own colour, as every
    // state here is, behind a light as large as a section's; the dot
    // and where it is, dimmer.)
    let connected = host.conn == HostConnState::Connected;
    let state: String = if shown.checking { olo_kit::words::tr("Checking…").into() } else { panels::host_state(&host.conn, false) };
    // (The head's one button, named for what it does to the host as
    // it stands: Check Again asks one that is connected what it has;
    // one that isn't is connected to, Connect, as Hosts and Ports
    // says; one that failed, Try Again, as a thing that failed here
    // says. One label for all three had a note under it explaining
    // what the button did: the sixth design review.)
    let again = match &host.conn {
        HostConnState::Connected => button("Check Again", "host-check", HostDo::Check, host.ssh && !shown.checking),
        HostConnState::Error(_) => button("Try Again", "host-check", HostDo::Connect, true),
        conn => button("Connect", "host-check", HostDo::Connect, *conn == HostConnState::Disconnected),
    };
    let head = div()
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
        )
        .child(again);

    // Why nothing is known of it, or nothing can be done to it: a note
    // of the host's own, under the head at the window's margin (with the
    // line under it there too: hung from the name, they made three left
    // edges, Opus's look), as far under the state as a section's note is
    // under its line. What its
    // connection said, where it failed, in a well, as the host's
    // words always are here. (The host's name not first: most
    // are in small letters, and a sentence that began with one read
    // as a slip. And the note says what isn't known and what would
    // end that, never a guess at why, nor the state over it again:
    // "couldn't be reached" stood over a well that said the host had
    // refused the keys.)
    let why: Option<String> = match &host.conn {
        HostConnState::Error(_) => Some(olo_kit::tr!("What’s installed on {} isn’t known until it connects.", host.name).into()),
        HostConnState::Disconnected => Some(olo_kit::tr!("What’s installed on {} isn’t known until it’s connected.", host.name).into()),
        HostConnState::Connected if !host.ssh => Some(olo_kit::tr!("Nothing can be installed on {} from here: it isn’t reached over SSH.", host.name).into()),
        _ => None,
    };
    // (The head a step further from the first section than the
    // sections are from each other: it has no rule under it.)
    let mut head = div().w_full().pb(px(m::TIGHT)).flex().flex_col().child(head);
    // (What a connection that failed said comes first, right under
    // the state, and the note under it: a failure has one shape in
    // this window, its line, the well, then the note. The well as
    // far under the state's words as a section's is under its line's.)
    if let HostConnState::Error(said) = &host.conn {
        let room = (((HOST_WIDTH - 2.0 * m::WINDOW_INSET - 2.0 * panels::WELL_INSET - 2.0) / letter).floor() as usize).max(20);
        let lines: Vec<String> = said.lines().filter(|l| !l.trim().is_empty()).flat_map(|line| crate::server_panel::hung(line, room)).collect();
        let lines = div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(MONO_SIZE).wrap(true)));
        // (Everything under the head's row at the window's margin, as the
        // line under it is: one left edge, not three, Opus's second look.)
        head = head.child(div().h(px(WELL_OVER + 3.0)).flex_shrink_0()).child(panels::well(lines.into_element()));
        if let Some(why) = why {
            head = head.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(small(why));
        }
    } else if let Some(why) = why {
        head = head.child(div().h(px(HEAD_NOTE_UNDER)).flex_shrink_0()).child(small(why));
    }

    // What the window is for, and where the account's own window is, under
    // the head at the window's margin, as the Account window says it the
    // other way round (Ben, 2026-11-24: "make the account one like a button
    // or link somewhere explaining that manage host is for host-wide
    // management, and a link or button to open the manage your user account
    // one"). (Not "for the whole host and every account on it": most of
    // what is here is in the account's home there, Opus's look said. The
    // link where its words fit, else on a line of its own.)
    {
        let on_account = on_do.clone();
        // (A space between its parts, as between words: a wider gap read
        // as a hole, Opus's second look.)
        let space = f32::from(style::text_width(" ", style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
        let link = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space))
            // (In the link's row, not a control's: a line, not a button's.)
            .child(StringView::new(olo_kit::words::tr("Your agents and repositories:").to_string()).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW))
            // (In the size of the words it ends, as every link here is: the
            // detail size read a size larger.)
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
                // (As wide as its words, narrower and wrapped only where the
                // line is too short: a wrapping view alone is as wide as
                // the line, and put the link on a line of its own.)
                .child(div().child(StringView::new(olo_kit::words::tr("Rex and Olo on this host.").to_string()).size(style::SMALL_SIZE).dim(true).wrap(true)))
                .child(link),
        );
    }
    let mut page = vec![head.into_element()];

    // The host's Rex: its server, and the helper that keeps typing quick.
    let mut rex: Vec<Element> = vec![block(
        "Rex server",
        &server_stands(host),
        // (Where its line says "Not known", there is nothing to manage.)
        vec![button("Manage…", "host-server", HostDo::Server, managed && connected)],
        olo_kit::words::tr("Its services, accounts and log, managed as root in their own window.").into(),
        None,
    )];
    // Olo's integration there: the bridge and its commands, the pool.
    let mut olo = Vec::new();

    let mut there = Vec::new();
    for (thing, title, id, note, path) in [
        (HostThing::Bridge, "Bridge", "bridge", "Lets programs there use links, Passwords, SSH keys and Agent\u{a0}Screens here.", None),
        // (The bridge's way on the host: the commands its programs
        // already call, in a Rex session. Under the bridge's line, with
        // no heading of its own.)
        (HostThing::Commands, "desktop commands", "commands", "Lets xdg-open, notify-send and copy and paste commands there act here.", Some("~/.local/share/olo/rex-commands")),
        // (What runs for the host first, the files put there after.)
        (HostThing::Pool, "Token Pool", "pool", "Lets agents there use your accounts, even with this computer off.", None),
        (HostThing::Skill, "Skill for agents", "skill", "Tells Claude Code and other agents there how to use the bridge and Rex.", Some("~/.claude/skills/olo-rex-bridge")),
        (HostThing::Helper, "Typing helper", "helper", "Keeps typing quick over a slow link.", Some("~/.local/share/olo/rex-link")),
    ] {
        let s = thing_stands(host, shown, thing);
        if s.there {
            there.push(thing);
        }
        let mut actions = Vec::new();
        // (What the bridge may do is set in Preferences: a step from
        // here, and its one button, since nothing of it is installed.)
        if thing == HostThing::Bridge {
            actions.push(button("Permissions…", "host-bridge-permissions", HostDo::Permissions, true));
        } else if thing == HostThing::Pool {
            // (Installed, kept up to date and removed in the Token
            // Pool's own Hosts window, which this opens: starting the
            // pool where it isn't running.)
            actions.push(button("Token Pool…", "host-pool-open", HostDo::OpenPool, true));
        } else {
            actions.push(match s.can {
                Some((add, true)) => button("Try Again", &format!("host-{id}-again"), HostDo::Change(thing, add), true),
                Some((true, false)) => button("Install", &format!("host-{id}-install"), HostDo::Change(thing, true), true),
                Some((false, false)) => button("Remove", &format!("host-{id}-remove"), HostDo::Change(thing, false), true),
                None => button(if s.there { "Remove" } else { "Install" }, &format!("host-{id}-wait"), HostDo::Check, false),
            });
        }
        // (The skill is the account's, and has its row in the Account
        // window: here it is still one of what Remove All… takes off.)
        if thing == HostThing::Skill {
            continue;
        }
        let note: String = if thing == HostThing::Helper && !host.helper_setting {
            olo_kit::words::tr("To use it, turn on Olo’s typing helper in Preferences\u{a0}▸\u{a0}Rex.").into()
        } else {
            olo_kit::words::tr(note).into()
        };
        if thing == HostThing::Commands
            && let Some(bridge) = olo.pop()
        {
            let words = olo_kit::words::tr(title);
            // (Nearer the bridge's line than a section is to the next: a
            // part of it.)
            olo.push(div().w_full().flex().flex_col().child(bridge).child(div().h(px(4.0)).flex_shrink_0()).child(body(&s, actions, note, path, Some(words), None)).into_element());
            continue;
        }
        let made = block(title, &s, actions, note, path);
        if thing == HostThing::Helper { rex.push(made) } else { olo.push(made) }
    }

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
            .child(StringView::new("--two-way".to_owned()).mono(true).size(MONO_SIZE).dim(true).height(style::line_row(MONO_SIZE)).beside(style::SMALL_SIZE))
            .child(StringView::new(olo_kit::words::tr("to sync edits both ways, or").to_string()).size(style::SMALL_SIZE).dim(true).height(style::line_row(style::SMALL_SIZE)))
            .child(StringView::new("--to-host".to_owned()).mono(true).size(MONO_SIZE).dim(true).height(style::line_row(MONO_SIZE)).beside(style::SMALL_SIZE))
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

    // (Every tab's page laid out, the panel as tall as the taller of Rex's
    // and Olo's, so the window keeps its height and its foot its place
    // from tab to tab: both looks. The folders' list fills that room and
    // scrolls in it.)
    let tab = shown.tab.min(2);
    let on_tab = on_do.clone();
    let names: Vec<SharedString> = ["Rex", "Olo Integration", "Files"].into_iter().map(SharedString::from).collect();
    let page_of = |things: Vec<Element>| div().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(things).into_element();
    page.push(TabView::new("host-tabs", names, tab).grows().on_select(move |i: &usize, cx| on_tab(&HostDo::Tab(*i), cx)).pages([page_of(rex), page_of(olo), files]).fill_page(2).into_element());

    // Everything of Olo's off the host at once: the one thing here
    // that asks first, in its own place at the window's foot.
    let busy = !shown.doing.is_empty();
    let foot = if shown.asking_all {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            // (One line, in the foot's own height: nothing moves when it
            // is asked. What "all" is, by name, and no more: with the
            // host's name and "The Rex server stays." after it, it was
            // cut at the buttons. Cancel is what Return does.)
            .child(div().flex_1().min_w_0().child(StringView::new(olo_kit::words::tr("Remove the bridge, skill and typing helper?")).cut(true)))
            .child(button("Cancel", "host-all-cancel", HostDo::AskAll(false), true).default(true))
            .child(button("Remove All", "host-all-remove", HostDo::RemoveAll(there.clone()), reached && !busy && !there.is_empty()))
    } else {
        // (The account moved to another host, whole: the other thing done
        // to the host as a whole, at the foot's other end.)
        div().w_full().flex().flex_row().items_center().child(button("Remove All…", "host-all", HostDo::AskAll(true), reached && !busy && !there.is_empty())).child(div().flex_1()).child(button(
            "Move to Another Host…",
            "host-move",
            HostDo::Move,
            reached && !busy,
        ))
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
            helper_kept_off: false,
            helper_setting: true,
            bridge: Some(false),
            granted: Vec::new(),
            agent_keys: None,
            skill: Some(false),
            commands: Some(false),
            pool: None,
            version: Some("0.19.2".into()),
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

        // The skill: asked, not there, there.
        h.skill = None;
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can), ((Light::Waiting, "Asking…", None), None));
        h.skill = Some(false);
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can), ((Light::Off, "Not installed", None), Some((true, false))));

        // The Token Pool, as this computer's own says: not running
        // here, none there, being installed, there and up to date,
        // there and behind (why, in the host's words), couldn't be.
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
        assert_eq!((said(&s), s.there, s.can), ((Light::On, "Installed", Some("2 accounts, up to date")), false, None));
        h.pool = Some(PoolStands { wanted: true, error: Some("no route".into()), accounts: Some(1), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.host_said.as_deref()), ((Light::Waiting, "Installed", Some("1 account, behind")), Some("no route")));
        h.pool = Some(PoolStands { error: Some("no room".into()), ..Default::default() });
        let s = of(&h, &shown, HostThing::Pool);
        assert_eq!((said(&s), s.host_said.as_deref(), s.can), ((Light::Down, "Couldn’t install", None), Some("no room"), None));

        // The helper: in use, for the next connection, taken off while
        // in use, couldn't be put there (the host's words kept), off.
        h.helper = HostHelper::Used;
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.there), ((Light::On, "Installed", Some("in use")), true));
        h.helper_kept_off = true;
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.there, s.can, s.busy), ((Light::Waiting, "Not installed", Some("this connection still uses it")), false, Some((true, false)), false));
        h.helper_kept_off = false;
        h.helper = HostHelper::Ready;
        assert_eq!(said(&of(&h, &shown, HostThing::Helper)), (Light::On, "Installed", Some("used the next time you connect")));
        h.helper = HostHelper::Without("no room".into());
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.host_said.as_deref(), s.can), ((Light::Down, "Couldn’t install", None), Some("no room"), Some((true, true))));
        h.helper = HostHelper::None;
        h.helper_setting = false;
        let s = of(&h, &shown, HostThing::Helper);
        assert_eq!((said(&s), s.can), ((Light::Off, "Not installed", Some("turned off for every host")), None));

        // What couldn't be done: the red light, the host's words, and
        // the same again to try; while it is tried, that it is.
        shown.failed.push((HostThing::Skill, false, "rm: busy".into()));
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.host_said.as_deref(), s.can), ((Light::Down, "Couldn’t remove", None), Some("rm: busy"), Some((false, true))));
        shown.doing.push((HostThing::Skill, false));
        let s = of(&h, &shown, HostThing::Skill);
        assert_eq!((said(&s), s.can, s.busy), ((Light::Waiting, "Being removed…", None), None, true));

        // A host that isn't connected: nothing is known, nothing to press.
        h.conn = HostConnState::Disconnected;
        for thing in [HostThing::Skill, HostThing::Helper, HostThing::Pool] {
            let s = of(&h, &Shown::default(), thing);
            assert_eq!((said(&s), s.can), ((Light::Off, "Not known", None), None));
        }
        assert_eq!(said(&server_stands(&h)), (Light::Off, "Not known", None));
        h.conn = HostConnState::Connected;
        assert_eq!(said(&server_stands(&h)), (Light::On, "Running", Some("0.19.2")));

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
}
