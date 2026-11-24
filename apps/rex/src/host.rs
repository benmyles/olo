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

use crate::hub::{HostThing, RexHub};

/// What the window is of, and at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The host's key in the hub.
    pub key: String,
    /// The name it goes by, as the window opened (its title's: kept
    /// here so the title is made without asking the app for its hub,
    /// which whoever opens the window may be holding).
    pub name: String,
    /// What is being put there (`true`) or taken off now.
    pub doing: Vec<(HostThing, bool)>,
    /// What couldn't be done last, of each thing, and what the host
    /// said (put there, or taken off).
    pub failed: Vec<(HostThing, bool, String)>,
    /// The host is being asked again what it has (Check Again).
    pub checking: bool,
    /// Remove All… was pressed, and waits to be told yes or no.
    pub asking_all: bool,
    /// The tab shown: Rex, Olo Integration, Files.
    pub tab: usize,
}

impl Shown {
    /// Whether `thing` is being put there (`Some(true)`) or taken off.
    pub fn doing(&self, thing: HostThing) -> Option<bool> {
        self.doing.iter().find(|(t, _)| *t == thing).map(|(_, add)| *add)
    }

    /// What the host said when `thing` couldn't be put there or taken
    /// off, the last time.
    pub fn failed(&self, thing: HostThing) -> Option<(bool, &str)> {
        self.failed.iter().find(|(t, ..)| *t == thing).map(|(_, add, said)| (*add, said.as_str()))
    }
}

static LIVE: Mutex<Option<Shown>> = Mutex::new(None);

fn live() -> olo_sync::MutexGuard<'static, Option<Shown>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host the window is of, if it is open.
pub fn current() -> Option<Shown> {
    live().clone()
}

/// The window is of the host `key`, which goes by `name`, from now
/// (what was being done to another goes on, and is no longer shown).
pub fn open(key: &str, name: &str) {
    let mut live = live();
    match live.as_mut().filter(|shown| shown.key == key) {
        Some(shown) => shown.name = name.to_owned(),
        None => *live = Some(Shown { key: key.to_owned(), name: name.to_owned(), ..Shown::default() }),
    }
}

/// The window closed.
pub fn close() {
    *live() = None;
}

/// Puts `thing` on the window's host (`add`), or takes it off: begun
/// now, and the window told when the host has answered.
pub fn change(hub: &Arc<RexHub>, thing: HostThing, add: bool) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut() else { return };
        if shown.doing(thing).is_some() {
            return;
        }
        shown.doing.push((thing, add));
        shown.failed.retain(|(t, ..)| *t != thing);
        shown.key.clone()
    };
    crate::ui::host_changed();
    let of = key.clone();
    hub.change_host(&key, thing, add, move |result| {
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of) {
            shown.doing.retain(|(t, _)| *t != thing);
            if let Err(said) = result {
                shown.failed.push((thing, add, said));
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

/// Remove All… was pressed (`true`), or its question answered no.
/// The window's tab `tab` shown.
/// The Host window's tab called `name` (`rex`, `olo`, `files`, or its
/// words, any case): `host.manage:KEY:TAB`.
pub fn tab_named(name: &str) -> Option<usize> {
    match name.trim().to_ascii_lowercase().as_str() {
        "rex" => Some(0),
        "olo" | "olo integration" | "integration" => Some(1),
        "files" => Some(2),
        _ => None,
    }
}

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
        change(hub, thing, false);
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
        assert_eq!(tab_named("ports"), None);
    }

    /// The window is of one host at a time: opened on another, what
    /// was shown of the first (a thing that failed there) is gone;
    /// asked for the same again, it is kept.
    #[test]
    fn the_window_is_of_one_host_and_keeps_what_it_shows_of_it() {
        close();
        open("a", "far");
        live().as_mut().unwrap().failed.push((HostThing::Bridge, true, "no room".into()));
        open("a", "far");
        assert_eq!(current().unwrap().failed(HostThing::Bridge), Some((true, "no room")));
        assert_eq!(current().unwrap().doing(HostThing::Bridge), None);
        open("b", "near");
        assert_eq!(current(), Some(Shown { key: "b".into(), name: "near".into(), ..Shown::default() }));
        close();
        assert_eq!(current(), None);
    }
}
