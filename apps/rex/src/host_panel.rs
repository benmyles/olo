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
    Element, IntoElement as _, ParentElement as _, Styled as _, WindowCx, canvas, div, icons, metrics as m, px, style,
    widgets::{Button, StringView, button_width, separator},
};

use crate::host::Shown;
use crate::hub::{HostConnState, HostHelper, HostStands, HostThing};
use crate::panels::{self, Light};

/// The window's width: Hosts and Ports', which it is opened from.
pub const HOST_WIDTH: f32 = 580.0;

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
    /// Remove All… pressed (`true`), or its question answered no.
    AskAll(bool),
    /// Its question answered yes: these taken off.
    RemoveAll(Vec<HostThing>),
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
    /// It is being done now (its light a ring, not a light at rest).
    pub busy: bool,
    /// Whether it is on the host, to be taken off.
    pub there: bool,
    /// What its button does, if it can be pressed now: put it there
    /// (`true`) or take it off, and whether that is a second try.
    pub can: Option<(bool, bool)>,
}

fn stands(light: Light, said: &'static str) -> Stands {
    Stands { light, said: olo_kit::words::tr(said).into(), more: None, host_said: None, why: None, busy: false, there: false, can: None }
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
        let keys = |name: &&'static str| match (*name, &host.agent_keys) {
            ("SSH keys", Some(Ok(0) | Err(_))) => olo_kit::words::tr("no SSH keys").to_string(),
            ("SSH keys", Some(Ok(n))) => olo_kit::words::count(*n as u64, "1 SSH key", "{n} SSH keys").to_string(),
            _ => olo_kit::words::tr(name).to_string(),
        };
        let granted = host.granted.iter().map(keys).collect::<Vec<_>>().join(", ");
        let connected = host.conn == HostConnState::Connected;
        // (Under it, the reason alone, as a sentence.)
        let none = match &host.agent_keys {
            Some(Ok(0)) => Some(olo_kit::words::tr("Passwords has no SSH keys to offer.").to_string()),
            Some(Err(why)) => Some(format!("{}.", why.trim_end_matches('.'))),
            _ => None,
        }
        .filter(|_| connected && host.granted.contains(&"SSH keys"));
        let light = match (connected, &none) {
            (false, _) => Light::Off,
            (true, Some(_)) => Light::Waiting,
            (true, None) => Light::On,
        };
        return Stands { more: Some(granted), why: none, ..stands(light, "On") };
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
    }
}

/// The account's line: how much of what it is given for work with
/// agents is there (`account::tally`), and what isn't, by name. Its
/// window (Account…) has each by itself.
pub fn account_stands(host: &HostStands) -> Stands {
    let reached = host.conn == HostConnState::Connected && host.ssh;
    match crate::account::tally(host) {
        None if reached => Stands { busy: true, ..stands(Light::Waiting, "Asking…") },
        None => stands(Light::Off, "Not known"),
        Some((0, _, _)) => stands(Light::Off, "Not set up"),
        Some((done, all, _)) if done == all => stands(Light::On, "Ready"),
        // (Part of it: amber, not the grey of nothing there. An
        // account with Claude Code in it isn't a broken one.)
        Some((done, all, left)) => {
            // (Two of what is missing by name, and how many more: all
            // five would run under the line's button.)
            let named = match left.as_slice() {
                [a, b, rest @ ..] if !rest.is_empty() => olo_kit::tr!("without {}, {} and {} more", a, b, rest.len()).to_string(),
                few => olo_kit::tr!("without {}", few.join(", ")).to_string(),
            };
            Stands { said: olo_kit::tr!("{} of {} ready", done, all).into(), more: Some(named), ..stands(Light::Waiting, "") }
        }
    }
}

/// The Rex server's line: running, and its version.
fn server_stands(host: &HostStands) -> Stands {
    if host.conn == HostConnState::Connected { Stands { more: host.version.clone(), ..stands(Light::On, "Running") } } else { stands(Light::Off, "Not known") }
}

/// A section's heading, over its rule (the Server window's).
fn section(title: &str) -> Element {
    div().w_full().flex().flex_col().child(StringView::new(title.to_owned()).bold(true).height(m::LABEL_ROW)).child(div().h(px(2.0)).flex_shrink_0()).child(separator()).into_element()
}

/// The window's content for `host`: `managed` whether its Rex server
/// can be looked after from here.
pub fn render_host_panel(host: &HostStands, shown: &Shown, managed: bool, on_do: impl Fn(&HostDo, &mut WindowCx) + 'static) -> Element {
    let on_do: Rc<dyn Fn(&HostDo, &mut WindowCx)> = Rc::new(on_do);
    // Every button one width, the widest of their words: one column
    // down the window's right side.
    let wide = ["Check Again", "Connect", "Manage…", "Account…", "Permissions…", "Token Pool…", "Install", "Remove", "Try Again", "Remove All…", "Cancel"]
        .iter()
        .map(|l| button_width(l, false))
        .fold(0.0, f32::max);
    let button = |label: &str, id: &str, what: HostDo, enabled: bool| {
        let on_do = on_do.clone();
        Button::new(label.to_owned()).id(id.to_owned()).min_width(wide).enabled(enabled).on_click(move |_, cx| on_do(&what, cx))
    };
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    // (What is under a line's words begins where they do, past the
    // light, and ends short of the buttons' column: the light alone in
    // the margin, the buttons with a column to themselves.)
    let under = panels::LIGHT + m::TIGHT;
    let short = wide + m::SECTION;
    // (How many letters a line of a well under a line's words holds.)
    let letter = f32::from(style::text_width("0000000000", MONO_SIZE, style::mono_font(olo_kit::FontWeight::NORMAL))) / 10.0;
    let room = (((HOST_WIDTH - 2.0 * m::WINDOW_INSET - under - 2.0 * panels::WELL_INSET - 2.0) / letter).floor() as usize).max(20);
    let block = |title: &str, s: &Stands, actions: Vec<Button>, note: String, path: Option<&str>| {
        let lamp = if s.busy { panels::busy_of(panels::LIGHT) } else { panels::light(s.light) };
        let mut said = div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT)).child(lamp).child(StringView::new(s.said.clone()));
        if let Some(more) = &s.more {
            said = said.child(div().flex_1().min_w_0().child(StringView::new(format!("·  {more}")).dim(true).cut(true)));
        }
        let line = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(said).children(actions);
        let mut block = div().w_full().flex().flex_col().child(section(title)).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(line);
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
        notes = notes.child(small(note));
        if let Some(path) = path {
            notes = notes.child(StringView::new(path.to_owned()).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(MONO_SIZE)));
        }
        block.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(notes).into_element()
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
    // of the host's own, under its name (from the name's own edge, as
    // every note here hangs from its line's words), as far under the
    // state as a section's note is under its line. What its
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
    let from_name = ICON + m::SPACING;
    // (The head a step further from the first section than the
    // sections are from each other: it has no rule under it.)
    let mut head = div().w_full().pb(px(m::TIGHT)).flex().flex_col().child(head);
    // (What a connection that failed said comes first, right under
    // the state, and the note under it: a failure has one shape in
    // this window, its line, the well, then the note. The well as
    // far under the state's words as a section's is under its line's.)
    if let HostConnState::Error(said) = &host.conn {
        let room = (((HOST_WIDTH - 2.0 * m::WINDOW_INSET - from_name - 2.0 * panels::WELL_INSET - 2.0) / letter).floor() as usize).max(20);
        let lines: Vec<String> = said.lines().filter(|l| !l.trim().is_empty()).flat_map(|line| crate::server_panel::hung(line, room)).collect();
        let lines = div().flex().flex_col().w_full().children(lines.into_iter().map(|line| StringView::new(line).mono(true).size(MONO_SIZE).wrap(true)));
        head = head.child(div().h(px(WELL_OVER + 3.0)).flex_shrink_0()).child(div().w_full().pl(px(from_name)).child(panels::well(lines.into_element())));
        if let Some(why) = why {
            head = head.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(div().w_full().pl(px(from_name)).child(small(why)));
        }
    } else if let Some(why) = why {
        head = head.child(div().h(px(HEAD_NOTE_UNDER)).flex_shrink_0()).child(div().w_full().pl(px(from_name)).child(small(why)));
    }

    let mut page = vec![head.into_element()];

    page.push(block(
        "Rex server",
        &server_stands(host),
        // (Where its line says "Not known", there is nothing to manage.)
        vec![button("Manage…", "host-server", HostDo::Server, managed && connected)],
        olo_kit::words::tr("Its services, accounts and log, managed as root in their own window.").into(),
        None,
    ));

    // The person's own account there, made ready for work in its own
    // window (the agents, and what they need).
    page.push(block(
        "Account",
        &account_stands(host),
        vec![button("Account…", "host-account", HostDo::Account, reached)],
        olo_kit::words::tr("Agents and settings in your account there.").into(),
        None,
    ));

    let mut there = Vec::new();
    for (thing, title, id, note, path) in [
        (HostThing::Bridge, "Bridge", "bridge", "Lets programs there use links, Passwords, SSH keys and Agent Screens here.", None),
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
        page.push(block(title, &s, actions, note, path));
    }

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
        div().w_full().flex().flex_row().items_center().child(button("Remove All…", "host-all", HostDo::AskAll(true), reached && !busy && !there.is_empty())).child(div().flex_1())
    };
    // (Its rule as far over the button as the window's foot is under
    // it: the separator's line is three points into it.)
    page.push(div().w_full().flex().flex_col().child(separator()).child(div().h(px(m::WINDOW_INSET - 3.0)).flex_shrink_0()).child(foot).into_element());

    // (A section's step between them, as the Server window's have.)
    div().w_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(page).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostStands {
        HostStands {
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

    /// The account's line: asked, nothing there, part of it (amber,
    /// with what is missing by name), all of it; not known where the
    /// host isn't connected.
    #[test]
    fn the_accounts_line_says_how_much_of_it_is_ready() {
        use olo_rex_client::ready::Account;
        let mut h = host();
        let s = account_stands(&h);
        assert_eq!((said(&s), s.busy), ((Light::Waiting, "Asking…", None), true));
        h.carried = Some((9, 0, false));
        h.account = Some(Account { has_curl: true, has_git: true, ..Account::default() });
        assert_eq!(said(&account_stands(&h)), (Light::Off, "Not set up", None));
        h.account = Some(Account { path: true, claude: Some("2.1.331".into()), git: Some(("You".into(), "you@example.org".into())), has_curl: true, has_git: true, ..Account::default() });
        assert_eq!(said(&account_stands(&h)), (Light::Waiting, "3 of 7 ready", Some("without Codex, Antigravity and 2 more")));
        h.skill = Some(true);
        h.carried = Some((0, 9, false));
        assert_eq!(said(&account_stands(&h)), (Light::Waiting, "5 of 7 ready", Some("without Codex, Antigravity")));
        h.account = Some(Account { codex: Some("0.172.0".into()), antigravity: Some("1.5.15".into()), ..h.account.clone().unwrap() });
        h.skill = Some(true);
        assert_eq!(said(&account_stands(&h)), (Light::On, "Ready", None));
        h.conn = HostConnState::Disconnected;
        assert_eq!(said(&account_stands(&h)), (Light::Off, "Not known", None));
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
        let keys = |offered: Option<Result<usize, String>>| {
            let s = of(&HostStands { granted: vec!["links", "SSH keys"], agent_keys: offered, ..h.clone() }, &shown, HostThing::Bridge);
            (said(&s).0, s.more.clone(), s.why.clone())
        };
        assert_eq!(keys(None), (Light::On, Some("links, SSH keys".into()), None));
        assert_eq!(keys(Some(Ok(2))), (Light::On, Some("links, 2 SSH keys".into()), None));
        assert_eq!(keys(Some(Ok(1))).1.as_deref(), Some("links, 1 SSH key"));
        assert_eq!(keys(Some(Ok(0))), (Light::Waiting, Some("links, no SSH keys".into()), Some("Passwords has no SSH keys to offer.".into())));
        assert_eq!(keys(Some(Err("Passwords isn’t running".into()))), (Light::Waiting, Some("links, no SSH keys".into()), Some("Passwords isn’t running.".into())));

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
    }
}
