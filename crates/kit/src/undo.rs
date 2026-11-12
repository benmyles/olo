//! Undo and Redo, one way for every app (Ben, 2026-11-07: "we need
//! generally and unviversally to support undo in all olo apps where it
//! makes sense").
//!
//! An app keeps a [`History`] of steps, each named for what it did ("Rename",
//! "“Play a sound when Olo starts”") and holding what the app needs to take
//! it back (`C`: files moved from where to where, settings as they were).
//! Doing something records a step ([`History::done`], or
//! [`History::done_merging`] for a run of the same thing: a slider dragged,
//! a field typed in, one step). Undo takes the last step off
//! ([`History::take_undo`]), the app takes it back, and records what that
//! did as the step Redo takes back ([`History::record`] with
//! [`Purpose::Undo`]); Redo the other way round. So both lists hold what was
//! done, and a step goes back and forth as often as wanted; doing something
//! new forgets what Redo had.
//!
//! Edit ▸ Undo and Redo name the next steps, with the keyboard profile's
//! keys (⌘Z, ⇧⌘Z on a Mac's: [`History::edit_items`]). A window without a
//! menu bar gets those keys from its Frame when nothing in it took them (a
//! text field's ⌘Z is its own), as its view's `undo` and `redo` commands
//! (`View::command`). Agents and the command palette
//! find Undo and Redo among the app's menus (`agent::set_menus`, the menu
//! bar's), and choose them as any command.

use std::time::{Duration, Instant};

use crate::menu::Menu;

/// The most steps kept each way.
const MOST: usize = 100;
/// A run of the same thing merges into one step while each comes within
/// this of the last.
const MERGE: Duration = Duration::from_millis(1500);

/// A step: what to call it (Edit ▸ Undo *name*) and what it did.
#[derive(Debug, Clone, PartialEq)]
pub struct Step<C> {
    pub name: String,
    pub change: C,
    /// What it was a run of (`done_merging`), and when it last grew.
    key: Option<String>,
    at: Instant,
}

/// Why a change is recorded: done, or an undo or a redo of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Do,
    Undo,
    Redo,
}

#[derive(Debug, Clone)]
pub struct History<C> {
    undo: Vec<Step<C>>,
    redo: Vec<Step<C>>,
}

impl<C> Default for History<C> {
    fn default() -> Self {
        History { undo: Vec::new(), redo: Vec::new() }
    }
}

impl<C> History<C> {
    /// Something was done: Undo takes it back next, and what was undone
    /// can't be redone any more.
    pub fn done(&mut self, name: impl Into<String>, change: C) {
        self.record(Purpose::Do, name, change);
    }

    /// `done`, as part of a run of `key` (a slider dragged, a field typed
    /// in): merged into the last step (`merge(last, this)`) if that was
    /// `key`'s too, within a moment, and nothing was undone since.
    pub fn done_merging(&mut self, key: &str, name: impl Into<String>, change: C, merge: impl FnOnce(&mut C, C)) {
        let now = Instant::now();
        if self.redo.is_empty()
            && let Some(last) = self.undo.last_mut()
            && last.key.as_deref() == Some(key)
            && now.duration_since(last.at) < MERGE
        {
            merge(&mut last.change, change);
            last.at = now;
            return;
        }
        self.done(name, change);
        if let Some(last) = self.undo.last_mut() {
            last.key = Some(key.to_owned());
        }
    }

    /// What a change for `purpose` did: Undo takes it back after a step
    /// was done or redone, Redo after one was undone.
    pub fn record(&mut self, purpose: Purpose, name: impl Into<String>, change: C) {
        let step = Step { name: name.into(), change, key: None, at: Instant::now() };
        match purpose {
            Purpose::Do => {
                self.redo.clear();
                self.undo.push(step);
            }
            Purpose::Redo => self.undo.push(step),
            Purpose::Undo => self.redo.push(step),
        }
        for list in [&mut self.undo, &mut self.redo] {
            if list.len() > MOST {
                list.drain(..list.len() - MOST);
            }
        }
    }

    /// The step to undo next, taken off.
    pub fn take_undo(&mut self) -> Option<Step<C>> {
        self.undo.pop()
    }

    /// The step to redo next, taken off.
    pub fn take_redo(&mut self) -> Option<Step<C>> {
        self.redo.pop()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Edit ▸ Undo's name: "Undo Rename", or "Undo" with nothing to undo.
    pub fn undo_label(&self) -> String {
        label("Undo", self.undo.last())
    }

    pub fn redo_label(&self) -> String {
        label("Redo", self.redo.last())
    }

    /// `menu` with Undo and Redo (commands `undo` and `redo`), named for
    /// their steps, with the profile's keys.
    pub fn edit_items(&self, menu: Menu) -> Menu {
        edit_items_named(menu, self.undo.last().map(|s| s.name.as_str()), self.redo.last().map(|s| s.name.as_str()))
    }

    /// Forget every step (what they'd take back is gone).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// Edit ▸ Undo and Redo for an app that keeps its steps its own way (a
/// document's, StyledEdit's): each step's name (None: nothing to take
/// back; "": a step without one), as [`History::edit_items`] has them.
pub fn edit_items_named(menu: Menu, undo: Option<&str>, redo: Option<&str>) -> Menu {
    edit_items_as(menu, undo, redo, ("undo", "redo"))
}

/// [`edit_items_named`] with commands of the app's own (`edit-undo`).
pub fn edit_items_as(menu: Menu, undo: Option<&str>, redo: Option<&str>, commands: (&str, &str)) -> Menu {
    let label = |verb: &str, name: Option<&str>| match name {
        Some(n) if !n.is_empty() => format!("{verb} {n}"),
        _ => verb.to_owned(),
    };
    menu.item(label("Undo", undo), commands.0).key("z").enabled(undo.is_some()).item(label("Redo", redo), commands.1).shift_key("z").enabled(redo.is_some())
}

fn label<C>(verb: &str, step: Option<&Step<C>>) -> String {
    match step {
        Some(s) if !s.name.is_empty() => format!("{verb} {}", s.name),
        _ => verb.to_owned(),
    }
}

/// Whether `k` is the profile's Undo (Some(false)) or Redo (Some(true)).
pub(crate) fn pressed(k: &olo_interface::Keystroke) -> Option<bool> {
    let is = |command: &str| olo_kit_base::keys::shortcut(command).is_some_and(|s| crate::keys::pressed(&s, k));
    if is("redo") {
        Some(true)
    } else if is("undo") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps go back and forth between the lists; something new forgets
    /// what Redo had; the menu names them.
    #[test]
    fn back_and_forth() {
        let mut h: History<i32> = History::default();
        assert_eq!((h.undo_label(), h.can_undo()), ("Undo".into(), false));
        h.done("Rename", 1);
        h.done("Move", 2);
        assert_eq!(h.undo_label(), "Undo Move");
        let s = h.take_undo().unwrap();
        h.record(Purpose::Undo, s.name, -s.change);
        assert_eq!((h.undo_label(), h.redo_label()), ("Undo Rename".into(), "Redo Move".into()));
        let s = h.take_redo().unwrap();
        assert_eq!(s.change, -2);
        h.record(Purpose::Redo, s.name, 2);
        assert_eq!(h.undo_label(), "Undo Move");
        let s = h.take_undo().unwrap();
        h.record(Purpose::Undo, s.name, -2);
        h.done("Copy", 3);
        assert!(!h.can_redo());
    }

    /// A run of the same thing is one step; another thing, or a pause,
    /// starts the next.
    #[test]
    fn runs_merge() {
        let mut h: History<Vec<i32>> = History::default();
        for v in 1..=3 {
            h.done_merging("volume", "Volume", vec![v], |a, b| a.extend(b));
        }
        h.done_merging("brightness", "Brightness", vec![9], |a, b| a.extend(b));
        assert_eq!(h.take_undo().unwrap().change, vec![9]);
        assert_eq!(h.take_undo().unwrap().change, vec![1, 2, 3]);
        assert!(h.take_undo().is_none());
    }
}
