//! Other programs' global shortcuts (`Request::GrabKeys`, which Olo's
//! GlobalShortcuts portal asks for an app: Discord's push-to-talk, a
//! browser extension's keys): chords a connection takes, sent to it as
//! `Event::KeyGrab` when pressed and released instead of to the focused
//! window, and let go when it closes. app_server's own shortcuts come
//! first and can't be taken, nor can typing: a plain letter, digit or the
//! like needs a modifier (a function key alone is fine).

use std::rc::Rc;

use olo_protocol::{Event, Reply, shortcuts::Chord};
use tracing::info;

use crate::{
    ipc::Conn,
    state::{AnvilState, Backend},
};

/// Whether `chord` is one a program may take: with a modifier, or a key
/// that types nothing (F1–F24, the media and extra keys).
pub fn takeable(chord: Chord) -> bool {
    let modified = chord.ctrl || chord.alt || chord.cmd || chord.shift && !(2..=57).contains(&chord.key);
    let function = matches!(chord.key, 59..=68 | 87 | 88 | 183..=194) || chord.key >= 113;
    chord.key != 0 && !olo_protocol::shortcuts::MODIFIER_KEYS.contains(&chord.key) && (modified || function)
}

impl<B: Backend> AnvilState<B> {
    /// Take `keys` for `conn` (replacing what it had).
    pub fn grab_keys(&mut self, conn: &Rc<Conn>, keys: Vec<(String, String)>) -> Reply {
        self.ipc.key_grabs.retain(|(c, _, _)| !Rc::ptr_eq(c, conn) && !c.is_dead());
        let mut refused = Vec::new();
        for (id, keys) in keys {
            let chord = keys.parse::<Chord>().ok().filter(|c| takeable(*c) && !self.keymap.is_bound(*c) && !self.ipc.key_grabs.iter().any(|(_, _, g)| g == c));
            match chord {
                Some(chord) => self.ipc.key_grabs.push((conn.clone(), id, chord)),
                None => refused.push(id),
            }
        }
        info!(by = conn.peer(), taken = self.ipc.key_grabs.iter().filter(|(c, _, _)| Rc::ptr_eq(c, conn)).count(), refused = refused.len(), "keys grabbed");
        self.sync_grabs();
        Reply::KeysGrabbed { refused }
    }

    /// `conn` closed: its keys are everyone's again.
    pub fn grabs_closed(&mut self, conn: &Rc<Conn>) {
        let before = self.ipc.key_grabs.len();
        self.ipc.key_grabs.retain(|(c, _, _)| !Rc::ptr_eq(c, conn) && !c.is_dead());
        if self.ipc.key_grabs.len() != before {
            self.sync_grabs();
        }
    }

    fn sync_grabs(&mut self) {
        self.keymap.set_grabs(self.ipc.key_grabs.iter().map(|(_, _, c)| *c).collect());
    }

    /// A taken chord went down or up: tell whoever took it.
    pub fn key_grabbed(&mut self, chord: Chord, pressed: bool) {
        if let Some((conn, id, _)) = self.ipc.key_grabs.iter().find(|(_, _, c)| *c == chord) {
            conn.send(&Reply::Event { event: Event::KeyGrab { id: id.clone(), pressed } });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_may_be_taken() {
        let c = |s: &str| s.parse::<Chord>().unwrap();
        assert!(takeable(c("Ctrl+Alt+K")));
        assert!(takeable(c("F12")) && takeable(c("Shift+F5")));
        // Typing stays typing.
        assert!(!takeable(c("K")) && !takeable(c("Shift+K")) && !takeable(c("Space")));
        assert!(takeable(c("Cmd+Space")));
    }
}
