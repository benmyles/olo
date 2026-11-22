//! Olo notifications for work outside the focused workspace. A pane owns one
//! notification until it is seen, dismissed, or its session goes away.
use std::collections::HashMap;

use olo_kit::notify::{Note, NoteEvent, NoteId, NoteKind, Notifier};

use crate::{hub::BlockKey, ui::WinId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Pane(BlockKey),
    Session { host_key: String, session_id: String },
    Host(String),
}

impl Target {
    fn visible_in(&self, front: &Self) -> bool {
        match (self, front) {
            (Self::Host(h), Self::Pane(p)) => h == &p.host_key,
            (Self::Host(h), Self::Session { host_key, .. }) => h == host_key,
            (Self::Session { host_key, session_id }, Self::Pane(p)) => host_key == &p.host_key && session_id == &p.session_id,
            _ => self == front,
        }
    }

    fn args(&self) -> Vec<String> {
        match self {
            Self::Pane(k) => vec!["--focus-block".into(), k.host_key.clone(), k.session_id.clone(), k.block_id.clone()],
            Self::Session { host_key, session_id } => vec!["--session".into(), host_key.clone(), session_id.clone()],
            Self::Host(k) => vec!["--focus-host".into(), k.clone()],
        }
    }
}

pub struct Attention {
    notifier: Notifier,
    notes: HashMap<Target, NoteId>,
    showing: HashMap<WinId, Target>,
}

impl Attention {
    pub fn new() -> Self {
        Self { notifier: Notifier::with_events("Rex", "olo-rex", crate::ui::notification_event), notes: HashMap::new(), showing: HashMap::new() }
    }

    pub fn show(&mut self, target: Target, title: String, body: String, kind: NoteKind) {
        if self.showing.values().any(|front| target.visible_in(front)) {
            return;
        }
        let action = match target {
            Target::Pane(_) => "Show Pane",
            Target::Session { .. } => "Show Session",
            Target::Host(_) => "Show Host",
        };
        let note = Note::new(title).body(body).kind(kind).icon("rex").action("default", action).shows(target.args());
        let id = self.notifier.show(self.notes.get(&target).copied(), note);
        self.notes.insert(target, id);
    }

    pub fn showing(&mut self, window: WinId, front: Target) {
        self.notes.retain(|target, id| {
            if target.visible_in(&front) {
                self.notifier.close(*id);
                false
            } else {
                true
            }
        });
        self.showing.insert(window, front);
    }

    pub fn hide(&mut self, window: WinId) {
        self.showing.remove(&window);
    }

    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }

    pub fn prune(&mut self, live: impl Fn(&Target) -> bool) {
        self.notes.retain(|target, id| {
            if live(target) {
                true
            } else {
                self.notifier.close(*id);
                false
            }
        });
    }

    pub fn event(&mut self, event: NoteEvent) -> Option<(Target, Option<String>)> {
        match event {
            NoteEvent::Action { id, key, token } if key == "default" => self.notes.iter().find(|(_, v)| **v == id).map(|(target, _)| (target.clone(), token)),
            NoteEvent::Closed { id } => {
                self.notes.retain(|_, v| *v != id);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_uses_the_notifications_own_scope() {
        let pane = |host: &str, session: &str, block: &str| Target::Pane(BlockKey { host_key: host.into(), session_id: session.into(), block_id: block.into() });
        let session = Target::Session { host_key: "first".into(), session_id: "work".into() };
        let host = Target::Host("first".into());
        let front = pane("first", "work", "one");

        assert!(front.visible_in(&front) && session.visible_in(&front) && host.visible_in(&front));
        assert!(!pane("first", "work", "two").visible_in(&front));
        assert!(!pane("second", "work", "one").visible_in(&front));
        assert!(!session.visible_in(&pane("first", "other", "one")));
        assert!(!session.visible_in(&pane("second", "work", "one")));
        assert!(session.visible_in(&session) && host.visible_in(&session));
        assert!(!front.visible_in(&session) && !session.visible_in(&host));
        assert_eq!(session.args(), ["--session", "first", "work"]);
    }
}
