//! A group of shortcuts, begun by its first chord (Ben, 2026-11-18: "you
//! do the first part to start Float/Move/etc and then the second part to
//! select the specific option. this chord support must be made native to
//! olo, supported and customizable in keyboard shortcut prefs").
//!
//! A shortcut's keys are one chord or two in turn (`olo_protocol::Keys`,
//! "Ctrl+Alt+Cmd+Z V"). The keymap takes the first of two (`ChordBegun`:
//! it goes to no program) and waits; the next key that isn't a modifier
//! picks from the group (pressed alone, or with the first's modifiers
//! still held) or lets go of it (Escape, any key that's none of its
//! own), and goes nowhere either way (`ChordEnded`, then the shortcut).
//! Left waiting, the group is let go of after `keymap::CHORD_WAIT`, and
//! by a press of the pointer.
//!
//! What a group can be finished with is shown while it waits, by the
//! palette (`olo-palette --chord FIRST`: a kit panel, each second chord
//! and what it does, read from here with `Request::Chord`), but only
//! once it has waited `SHOW_AFTER`: keys typed straight through show
//! nothing. The palette takes it down at `Event::Chord` with no first.
//!
//! An app's commands can have keys of two parts too (Preferences ▸
//! Keyboard ▸ Shortcuts gives any shortcut some): the profile's entries
//! for the focused app are the keymap's `app_chords`, and one picked is
//! chosen from the app's menus over its commands socket, as an agent
//! would choose it (the kit's `invoke_in`, sent off this thread:
//! `window_drop::invoke`); Chrome's is pressed as what Chrome takes for
//! it (`app_keys::CHROME_NATIVE`, by the keymap).
//!
//! Nothing here runs but at a key: a profile with no app's keys of two
//! parts (as they come) costs a key press one test of an empty list.

use std::time::Duration;

use olo_protocol::{Chord, ChordInfo, ChordOption, Keys, app_keys, shortcuts};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    keymap::CHORD_WAIT,
    state::{AnvilState, Backend},
};

/// How long a group waits before what it can be finished with is shown.
const SHOW_AFTER: Duration = Duration::from_millis(350);

/// Every app's command with keys of two parts in `profile`: its keys,
/// whose it is (None: every app's) and its entry's id.
pub fn app_chords(profile: &olo_protocol::KeyboardProfile) -> Vec<(Keys, Option<&'static str>, &'static str)> {
    let mut out = Vec::new();
    for e in app_keys::APP_KEYS {
        let Some(entry) = shortcuts::entry(e.id) else { continue };
        out.extend(entry.keys(profile).into_iter().filter(|k| k.then.is_some()).map(|k| (k, e.app, e.id)));
    }
    out
}

impl<B: Backend + 'static> AnvilState<B> {
    /// The profile changed: its apps' keys of two parts, read again.
    pub fn chords_changed(&mut self) {
        self.app_chords = app_chords(&self.settings.keyboard.profile);
        self.chord_app = None;
        self.keymap.set_app_chords(Vec::new());
    }

    /// Before a key goes to the keymap: the keys of two parts of the app
    /// that has the keyboard (told the keymap when that app changes).
    pub fn chord_focus(&mut self, app_id: &str) {
        if self.app_chords.is_empty() || self.chord_app.as_deref() == Some(app_id) {
            return;
        }
        self.chord_app = Some(app_id.to_owned());
        let own = self.app_chords.iter().filter(|(_, app, _)| app.is_none_or(|a| a == app_id)).map(|&(k, _, id)| (k, id)).collect();
        self.keymap.set_app_chords(own);
    }

    /// A group's first chord went down: listeners are told, what it can
    /// be finished with is shown if it's still waiting in a moment, and
    /// it's let go of if nothing comes.
    pub fn chord_begun(&mut self, first: Chord) {
        self.chord_turn += 1;
        let turn = self.chord_turn;
        tracing::debug!(%first, "a group of shortcuts begun");
        self.ipc.broadcast(olo_protocol::Event::Chord { first: Some(first.to_string()) });
        let waiting = move |state: &Self| state.chord_turn == turn && state.keymap.chord_waiting() == Some(first);
        let _ = self.handle.insert_source(Timer::from_duration(SHOW_AFTER), move |_, _, state| {
            if waiting(state) {
                // (On the display shortcuts act on.)
                let first = first.to_string();
                match state.active_output().map(|o| o.name()) {
                    Some(output) => state.spawn_program("olo-palette", &["--chord", &first, &output]),
                    None => state.spawn_program("olo-palette", &["--chord", &first]),
                }
            }
            TimeoutAction::Drop
        });
        let _ = self.handle.insert_source(Timer::from_duration(CHORD_WAIT), move |_, _, state| {
            if waiting(state) {
                state.chord_let_go();
            }
            TimeoutAction::Drop
        });
    }

    /// The group waits no more (picked from, or let go of by a key).
    pub fn chord_ended(&mut self) {
        self.chord_turn += 1;
        self.ipc.broadcast(olo_protocol::Event::Chord { first: None });
    }

    /// The group that's waiting is let go of: it waited too long, or the
    /// pointer was pressed.
    pub fn chord_let_go(&mut self) {
        if self.keymap.chord_let_go() {
            self.chord_ended();
        }
    }

    /// What `first` begins (as profiles write it; None: the group that's
    /// waiting now), for whoever shows it (`Request::Chord`): what can
    /// be done here, as a menu lists it (the workspaces there are;
    /// another display only where there is one).
    pub fn chord_info(&self, first: Option<&str>) -> ChordInfo {
        let waiting = self.keymap.chord_waiting();
        let Some(chord) = first.map_or(waiting, |f| f.parse().ok()) else { return ChordInfo::default() };
        let profile = &self.settings.keyboard.profile;
        let (name, mut own) = shortcuts::begun_by(chord, profile);
        let workspaces = self.settings.workspace_count.max(1);
        let displays = self.output_infos().iter().filter(|o| o.mirror_of.is_none()).count();
        own.retain(|(_, info)| match info.id {
            "next-display" | "previous-display" => displays > 1,
            "window-to-next-workspace" | "window-to-previous-workspace" => workspaces > 1,
            id => id.strip_prefix("window-to-workspace-").and_then(|n| n.parse::<u32>().ok()).is_none_or(|n| n <= workspaces),
        });
        // (Where the focused window is already: its workspace.)
        let on = self.focused_window().and_then(|w| w.meta().workspace.get().or_else(|| self.display_of(&w).map(|o| self.showing(&o))));
        let option = |then: Chord, id: &str, title: &str| ChordOption {
            keys: then.to_string(),
            label: then.label(profile.platform),
            id: id.to_owned(),
            title: title.to_owned(),
            current: on.is_some() && id.strip_prefix("window-to-workspace-").and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| Some(n - 1) == on),
        };
        let mut options: Vec<ChordOption> = own.iter().map(|(then, info)| option(*then, info.id, info.title)).collect();
        // The focused app's own (and every app's) that begin with it.
        let app = self.chord_app.as_deref();
        for (keys, of, id) in self.app_chords.iter().filter(|(k, of, _)| k.first == chord && of.is_none_or(|a| Some(a) == app)) {
            let _ = of;
            if let (Some(then), Some(entry)) = (keys.then, shortcuts::entry(id)) {
                options.push(option(then, entry.id, entry.title));
            }
        }
        // (A name only for a group that's all one menu's, apps' aside.)
        let name = name.filter(|_| options.len() == own.len()).map(str::to_owned);
        ChordInfo { first: Some(chord.to_string()), label: chord.label(profile.platform), name, waiting: waiting == Some(chord), options }
    }

    /// A group's second chord picked a command of the focused app's
    /// (the profile's entry `id`): chosen from that window's menus, as
    /// from its menu bar. (Chrome's is pressed as Chrome takes it, by the
    /// keymap.)
    pub fn app_command(&mut self, id: &'static str) {
        let (Some(entry), Some(window)) = (app_keys::by_id(id), self.focused_window()) else { return };
        tracing::info!(id, app = window.app_id(), "an app's command, by its keys of two parts");
        let Some(pid) = window.meta().owner.borrow().as_ref().map(|(pid, _)| *pid) else { return };
        let Some(surface) = window.wl_surface().map(|s| smithay::reexports::wayland_server::Resource::id(&*s).protocol_id()) else { return };
        crate::window_drop::invoke(pid, surface, entry.command);
    }
}
