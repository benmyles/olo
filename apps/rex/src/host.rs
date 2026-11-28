//! The Host window's state: everything of Olo's on a far host, looked
//! after in one place (Ben, 2026-11-23: "we also need like a manage
//! host button or something where we can manage all the services and
//! things installed on a host in one place instead of only through
//! random menus and stuff").
//!
//! What a host has is the hub's to know (`HubState::host_stands`: its
//! typing helper, Olo's bridge, the skill for agents, its Rex server);
//! here is which host the window is of, what is being done to it, and
//! what the host said of the last thing that couldn't be done. Each
//! thing is put there or taken off over SSH, off the connection's own
//! path (`RexHub::change_host`), and the window shows what the host
//! says once it has. One host at a time (there is one Host window).

use std::sync::Arc;

use olo_sync::Mutex;

use crate::hub::{HostOp, HostThing, RexHub};

/// What the window is of, and at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The host's key in the hub.
    pub key: String,
    /// The host's instance ID when the window opened (so clicks queued for an
    /// older/replaced host never run on its replacement).
    pub host_instance_id: u64,
    /// The name it goes by, as the window opened (its title's: kept
    /// here so the title is made without asking the app for its hub,
    /// which whoever opens the window may be holding).
    pub name: String,
    /// What is being installed, updated, or removed now.
    pub doing: Vec<(HostThing, HostOp)>,
    /// What couldn't be done last, of each thing, and what the host
    /// said.
    pub failed: Vec<(HostThing, HostOp, String)>,
    /// The host is being asked again what it has (Check Again).
    pub checking: bool,
    /// Remove All… was pressed, and waits to be told yes or no.
    pub asking_all: bool,
    /// The tab shown: Rex, Olo Integration, Files, Screens, Tweaks.
    pub tab: usize,
    /// Where an agent's screen opens there is being changed (to the
    /// host's own, `true`): shown as chosen until the host has answered.
    pub screens_default: Option<bool>,
    /// What is being done to which of the host's screens now.
    pub screen_doing: Vec<(String, ScreenDo)>,
    /// What couldn't be done to one of them last: its name, what wasn't
    /// done ("Couldn’t stop review") and why, the two halves of one
    /// sentence (the first in the ink of the window's other failures).
    pub screen_failed: Option<(String, String, String)>,
}

/// What is done to one of a host's screens from the Host window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScreenDo {
    /// Stopped there (what runs on it ends; the screen is kept).
    Stop,
}

impl Shown {
    /// What is being done to the host's screen `name` now, if anything.
    pub fn screen_doing(&self, name: &str) -> Option<ScreenDo> {
        self.screen_doing.iter().find(|(n, _)| n == name).map(|(_, what)| *what)
    }

    /// What is being done to `thing` right now, if anything.
    pub fn doing(&self, thing: HostThing) -> Option<HostOp> {
        self.doing.iter().find(|(t, _)| *t == thing).map(|(_, op)| *op)
    }

    /// What the host said when `thing` couldn't be installed, updated,
    /// or removed, the last time.
    pub fn failed(&self, thing: HostThing) -> Option<(HostOp, &str)> {
        self.failed.iter().find(|(t, ..)| *t == thing).map(|(_, op, said)| (*op, said.as_str()))
    }
}

static LIVE: Mutex<Option<Shown>> = Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_LIVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn live() -> olo_sync::MutexGuard<'static, Option<Shown>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host the window is of, if it is open.
pub fn current() -> Option<Shown> {
    live().clone()
}

/// The window is of the host `key`, which goes by `name`, from now
/// (what was being done to another goes on, and is no longer shown).
#[cfg(test)]
pub fn open(key: &str, name: &str, host_instance_id: u64) {
    open_with_tab(key, name, host_instance_id, None);
}

/// Like [`open`], optionally selecting an initial `tab` before the Host window
/// is created or raised so it never builds and fits its height on tab 0 first.
pub fn open_with_tab(key: &str, name: &str, host_instance_id: u64, tab: Option<usize>) {
    let mut live = live();
    match live.as_mut().filter(|shown| shown.key == key && shown.host_instance_id == host_instance_id) {
        Some(shown) => {
            shown.name = name.to_owned();
            if let Some(t) = tab {
                shown.tab = t;
            }
        }
        None => {
            let mut shown = Shown { key: key.to_owned(), host_instance_id, name: name.to_owned(), tab: tab.unwrap_or(0), ..Shown::default() };
            // (Rex's demo, for the window's pictures and its lint: what
            // couldn't be done of a host's own screens, as each is said.
            // `OLO_REX_DEMO_SCREENS=failed`, `refused`, `stopping`,
            // `stopfailed`.)
            if crate::server::demo_mode() {
                match std::env::var("OLO_REX_DEMO_SCREENS").ok().as_deref() {
                    Some("failed") => shown.failed.push((HostThing::Screens, HostOp::Install, crate::host_screens::why_not_installed("GitHub couldn't be reached: the connection timed out"))),
                    Some("refused") => shown.failed.push((
                        HostThing::Screens,
                        HostOp::Remove,
                        crate::host_screens::why_not_removed("olo screen: screens still run here (review, agent-2): `olo screen --on host stop NAME` first"),
                    )),
                    // (And of one of its screens: being stopped; one
                    // that couldn't be.)
                    Some("stopping") => shown.screen_doing.push(("review".into(), ScreenDo::Stop)),
                    Some("stopfailed") => shown.screen_failed = Some(("agent-2".into(), "Couldn’t stop agent-2".into(), "the host didn’t answer in time.".into())),
                    _ => {}
                }
            }
            *live = Some(shown);
        }
    }
}

/// The window closed.
pub fn close() {
    *live() = None;
}

/// Installs, updates, or removes `thing` on the window's host: begun
/// now, and the window told when the host has answered.
pub fn change(hub: &Arc<RexHub>, thing: HostThing, op: HostOp) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut() else { return };
        if shown.doing(thing).is_some() {
            return;
        }
        shown.doing.push((thing, op));
        shown.failed.retain(|(t, ..)| *t != thing);
        shown.key.clone()
    };
    crate::ui::host_changed();
    let of = key.clone();
    hub.change_host(&key, thing, op, move |result| {
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of) {
            shown.doing.retain(|(t, _)| *t != thing);
            if let Err(said) = result {
                shown.failed.push((thing, op, said));
            }
        }
        crate::ui::host_changed();
    });
}

/// Asks the window's host again what it has.
pub fn check(hub: &Arc<RexHub>) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| !shown.checking) else { return };
        shown.checking = true;
        shown.key.clone()
    };
    crate::ui::host_changed();
    let of = key.clone();
    hub.check_host(&key, move || {
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of) {
            shown.checking = false;
        }
        crate::ui::host_changed();
    });
}

/// Where an agent's screen opens on the window's host when the agent
/// doesn't say: the host's own (`true`), or this computer's.
pub fn screens_default(hub: &Arc<RexHub>, on_host: bool) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| shown.screens_default.is_none()) else { return };
        shown.screens_default = Some(on_host);
        shown.failed.retain(|(t, ..)| *t != HostThing::Screens);
        shown.key.clone()
    };
    crate::ui::host_changed();
    let of = key.clone();
    hub.screens_default(&key, on_host, move |result| {
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of) {
            shown.screens_default = None;
            if let Err(said) = result {
                shown.failed.push((HostThing::Screens, HostOp::Update, said));
            }
        }
        crate::ui::host_changed();
    });
}

/// Olo being put on the window's host for its screens, stopped where
/// it is: what the window was waiting for is no longer being done.
pub fn cancel_screens(hub: &Arc<RexHub>) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut() else { return };
        shown.doing.retain(|(t, _)| *t != HostThing::Screens);
        shown.key.clone()
    };
    hub.cancel_screens(&key, crate::ui::host_changed);
    crate::ui::host_changed();
}

/// Stop one displayed screen through Rex's guarded Stop path, verifying that
/// the Host window is still showing the same `(host_key, host_instance_id)`
/// that the button was rendered for (so a socket-driven switch to another host
/// never retargets a queued click).
pub fn stop_screen(hub: &Arc<RexHub>, host_key: &str, host_instance_id: u64, name: &str, expected_started: Option<u64>) {
    {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| shown.key == host_key && shown.host_instance_id == host_instance_id && shown.screen_doing(name).is_none()) else { return };
        shown.screen_doing.push((name.to_owned(), ScreenDo::Stop));
        shown.screen_failed = None;
    }
    crate::ui::host_changed();
    let (of, inst, named) = (host_key.to_owned(), host_instance_id, name.to_owned());
    let ended = move |result: Result<(), String>| {
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of && shown.host_instance_id == inst) {
            shown.screen_doing.retain(|(n, _)| *n != named);
            if let Err(why) = result {
                let why = crate::host_screens::why_not_done(&why);
                let what = olo_kit::tr!("Couldn’t stop {}", named);
                shown.screen_failed = Some((named.clone(), what.to_string(), why));
            }
        }
        crate::ui::host_changed();
    };
    hub.stop_screen_guarded(host_key, host_instance_id, name, expected_started, ended);
}

/// Record a screen action failure (e.g. "Look at Screen" refused on an older
/// helper or vanished screen) directly on the Host window's `screen_failed`
/// row when it is still showing `(host_key, host_instance_id)` (Neon #12).
pub fn screen_failed(host_key: &str, host_instance_id: u64, name: &str, what: &str, why: &str) {
    let mut updated = false;
    {
        let mut live = live();
        if let Some(shown) = live.as_mut().filter(|shown| shown.key == host_key && (host_instance_id == 0 || shown.host_instance_id == host_instance_id)) {
            shown.screen_failed = Some((name.to_owned(), what.to_owned(), why.to_owned()));
            updated = true;
        }
    }
    if updated {
        crate::ui::host_changed();
    }
}

/// Remove All… was pressed (`true`), or its question answered no.
/// The Tweaks tab's place among the window's tabs: the machine's own
/// settings, changed as root (`server::for_tweaks`).
pub const TWEAKS: usize = 4;

/// The Host window's tab called `name` (`rex`, `olo`, `files`,
/// `screens`, `tweaks`, or its words, any case): `host.manage:KEY:TAB`.
pub fn tab_named(name: &str) -> Option<usize> {
    match name.trim().to_ascii_lowercase().as_str() {
        "rex" => Some(0),
        "olo" | "olo integration" | "integration" => Some(1),
        "files" => Some(2),
        "screens" | "agent screens" => Some(3),
        "tweaks" => Some(TWEAKS),
        _ => None,
    }
}

/// The window's tab `tab` shown.
pub fn show_tab(tab: usize) {
    if let Some(shown) = live().as_mut() {
        shown.tab = tab;
    }
    crate::ui::host_changed();
}

pub fn ask_all(asking: bool) {
    if let Some(shown) = live().as_mut() {
        shown.asking_all = asking;
    }
    crate::ui::host_changed();
}

/// Its question answered yes: each of `things` taken off the host.
pub fn remove_all(hub: &Arc<RexHub>, things: &[HostThing]) {
    if let Some(shown) = live().as_mut() {
        shown.asking_all = false;
    }
    for &thing in things {
        change(hub, thing, HostOp::Remove);
    }
    crate::ui::host_changed();
}

/// Updates every outdated Olo component in `things` on the host at once.
pub fn update_all(hub: &Arc<RexHub>, things: &[HostThing]) {
    for &thing in things {
        change(hub, thing, HostOp::Update);
    }
    crate::ui::host_changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tab by its name, as `host.manage:KEY:TAB` gives it.
    #[test]
    fn a_tab_by_its_name() {
        assert_eq!((tab_named("rex"), tab_named("Olo"), tab_named("Olo Integration"), tab_named("FILES")), (Some(0), Some(1), Some(1), Some(2)));
        assert_eq!((tab_named("screens"), tab_named("Agent Screens")), (Some(3), Some(3)));
        assert_eq!((tab_named("tweaks"), tab_named("Tweaks")), (Some(TWEAKS), Some(TWEAKS)));
        assert_eq!(tab_named("ports"), None);
    }

    /// The window is of one host at a time: opened on another, what
    /// was shown of the first (a thing that failed there) is gone;
    /// asked for the same again, it is kept.
    #[test]
    fn the_window_is_of_one_host_and_keeps_what_it_shows_of_it() {
        let _guard = TEST_LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        close();
        open("a", "far", 1);
        live().as_mut().unwrap().failed.push((HostThing::Bridge, HostOp::Install, "no room".into()));
        open("a", "far", 1);
        assert_eq!(current().unwrap().failed(HostThing::Bridge), Some((HostOp::Install, "no room")));
        assert_eq!(current().unwrap().doing(HostThing::Bridge), None);
        open("b", "near", 2);
        assert_eq!(current(), Some(Shown { key: "b".into(), host_instance_id: 2, name: "near".into(), ..Shown::default() }));
        // `open_with_tab` sets the initial tab atomically before opening the window (Agy Finding 7).
        open_with_tab("c", "screens-box", 3, Some(3));
        assert_eq!(current(), Some(Shown { key: "c".into(), host_instance_id: 3, name: "screens-box".into(), tab: 3, ..Shown::default() }));
        open_with_tab("c", "screens-box", 3, Some(1));
        assert_eq!(current().unwrap().tab, 1);
        close();
        assert_eq!(current(), None);
    }
}
