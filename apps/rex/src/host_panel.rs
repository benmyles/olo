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

use crate::host::Shown;
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
    /// A tab chosen: Rex, Olo Integration, Files, Screens.
    Tab(usize),
    /// Where an agent's screen opens there when it doesn't say: on the
    /// host itself (`true`), or on this computer.
    ScreensDefault(bool),
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
        return Stands { busy: true, more: host.screens_doing.clone(), ..stands(Light::Waiting, word) };
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
                        let waits = s.running_said().map(|running| format!("{running} {}", olo_kit::words::tr("Update once they are stopped.")));
                        on_its_own(Stands {
                            more: host.this_olo.as_ref().and_then(|(_, mine)| s.update_said(mine)),
                            which: waits,
                            can: Some((HostOp::Update, false)),
                            ..installed(stands(Light::Waiting, "Update available"))
                        })
                    }
                    (what, None) => on_its_own(Stands { more: what, which: s.running_said(), ..installed(stands(Light::On, "Installed")) }),
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

/// The window's content for `host`: `managed` whether its Rex server
/// can be looked after from here.
pub fn render_host_panel(host: &HostStands, shown: &Shown, managed: bool, on_do: impl Fn(&HostDo, &mut WindowCx) + 'static) -> Element {
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
    let body = |s: &Stands, actions: Vec<Button>, note: String, policy: String, path: Option<&str>| {
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
        let mut notes = div().w_full().pl(px(under)).flex().flex_col();
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
        block = block.child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(notes);
        block
    };
    let block = |title: &'static str, s: &Stands, actions: Vec<Button>, note: String, policy: String, path: Option<&str>| {
        div().w_full().flex().flex_col().child(section(title)).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(body(s, actions, note, policy, path)).into_element()
    };
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
            scr_actions.push(button("Update", "host-screens-update", HostDo::Change(HostThing::Screens, HostOp::Update), reached));
            scr_actions.push(button("Remove", "host-screens-remove", HostDo::Change(HostThing::Screens, HostOp::Remove), reached));
        }
        Some((HostOp::Remove, false)) => scr_actions.push(button("Remove", "host-screens-remove", HostDo::Change(HostThing::Screens, HostOp::Remove), reached)),
        // (Not known yet what is there, or being put there: no button
        // that guesses it or repeats the one just pressed. Both
        // reviewers, of a greyed Install beside "Asking…" that became
        // Remove as the host answered, and beside "Being installed…"
        // where it read as stuck.)
        None if host.screens.is_none() || scr_s.busy => {}
        None => scr_actions.push(button(if olo_there { "Remove" } else { "Install" }, "host-screens-wait", HostDo::Check, false)),
    }
    // (What it does, then how it is updated, a line each, as the Rex
    // server's row says of itself; both sizes said where the choice is
    // made, each by its name: "about 600 MB" alone, beside "2.3 GB"
    // once it was there, was two figures that disagreed. And the host's
    // copy isn't called a desktop, which `--on desktop` below means
    // this computer by. The same under every state. A reviewer's.)
    let screens_block = block(
        "Agent Screens on this host",
        &scr_s,
        scr_actions,
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
    let screens = vec![screens_block, where_block];

    // (Every tab's page laid out, the panel as tall as the taller of Rex's
    // and Olo's, so the window keeps its height and its foot its place
    // from tab to tab: both looks. The folders' list fills that room and
    // scrolls in it.)
    let tab = shown.tab.min(3);
    let on_tab = on_do.clone();
    let mut files_dot: Option<Light> = None;
    for k in &host.kept {
        let ks = kept_stands(k);
        if !ks.busy && matches!(ks.light, Light::Down | Light::Waiting) {
            bump_dot(&mut files_dot, ks.light);
        }
    }
    let names: Vec<SharedString> = ["Rex", "Olo Integration", "Files", "Screens"].into_iter().map(SharedString::from).collect();
    let page_of = |things: Vec<Element>| div().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(things).into_element();
    page.push(
        TabView::new("host-tabs", names, tab)
            .lights([rex_dot, olo_dot, files_dot, screens_dot])
            .grows()
            .on_select(move |i: &usize, cx| on_tab(&HostDo::Tab(*i), cx))
            .pages([page_of(rex), page_of(olo), files, page_of(screens)])
            .fill_page(2)
            .into_element(),
    );

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
        h.screens_doing = Some("Downloading Olo, 42%".into());
        let s = of(&h, &shown);
        assert_eq!((said(&s), s.busy, s.can), ((Light::Waiting, "Being installed…", Some("Downloading Olo, 42%")), true, None));
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
}
