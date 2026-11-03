//! The person's changes to their chats on their way to Telegram: a chat
//! archived or not, pinned, muted, read, marked unread. Each is written to
//! disk before it's sent and leaves only once Telegram has it, so none is
//! lost to quitting, a crash, a dropped connection or Telegram asking us
//! to wait: what's left goes when the app starts again. Until then the
//! chat shows the change (`apply`), whatever older state Telegram or the
//! cache still has.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Chat, ChatId, MsgId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Archive { chat: ChatId, on: bool },
    Pin { chat: ChatId, on: bool },
    Mute { chat: ChatId, on: bool },
    /// Read up to `up_to`.
    Read { chat: ChatId, up_to: MsgId },
    MarkUnread { chat: ChatId, on: bool },
}

impl Change {
    pub fn chat(&self) -> ChatId {
        match *self {
            Change::Archive { chat, .. } | Change::Pin { chat, .. } | Change::Mute { chat, .. } | Change::Read { chat, .. } | Change::MarkUnread { chat, .. } => chat,
        }
    }

    /// Whether `other` changes the same thing of the same chat (a later
    /// one replaces it: unarchived after archived is only unarchived).
    fn same(&self, other: &Change) -> bool {
        self.chat() == other.chat() && std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// The chat's row as it is once Telegram has the change.
    pub fn apply(&self, c: &mut Chat) {
        match *self {
            Change::Archive { on, .. } => {
                c.archived = on;
                if on {
                    c.pinned = false;
                }
            }
            Change::Pin { on, .. } => c.pinned = on,
            Change::Mute { on, .. } => c.muted = on,
            Change::Read { up_to, .. } => {
                c.read_inbox = c.read_inbox.max(up_to);
                if c.last.as_ref().is_none_or(|l| l.id <= up_to) {
                    c.unread = 0;
                    c.mentions = 0;
                }
                c.marked = false;
            }
            Change::MarkUnread { on, .. } => c.marked = on,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Outbox {
    changes: Vec<Change>,
}

impl Outbox {
    fn path() -> PathBuf {
        crate::account::dir().join("outbox.json")
    }

    /// What was left unsent last time.
    pub fn load() -> Outbox {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// On disk (written whole, then put in place: never half a file).
    pub fn save(&self) {
        let path = Self::path();
        if self.changes.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        if let Err(e) = crate::account::private_dir().and_then(|_| crate::account::private_write(&path, &serde_json::to_vec(self).unwrap_or_default())) {
            tracing::warn!("telegram: the changes waiting to be sent couldn't be kept: {e}");
        }
    }

    /// A change to send, in place of one to the same thing still waiting.
    pub fn push(&mut self, change: Change) {
        self.changes.retain(|c| !c.same(&change));
        self.changes.push(change);
    }

    /// Those sent: gone, unless changed again since.
    pub fn sent(&mut self, done: &[Change]) {
        self.changes.retain(|c| !done.contains(c));
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// `c` as it is once what's waiting for it is sent.
    pub fn apply_to(&self, c: &mut Chat) {
        let id = c.id;
        for change in self.changes.iter().filter(|x| x.chat() == id) {
            change.apply(c);
        }
    }

    /// Whether a change to chat `id` is waiting.
    pub fn touches(&self, id: ChatId) -> bool {
        self.changes.iter().any(|c| c.chat() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Message, Preview};

    #[test]
    fn later_changes_replace_and_sent_ones_leave() {
        let mut o = Outbox::default();
        o.push(Change::Archive { chat: 1, on: true });
        o.push(Change::Mute { chat: 1, on: true });
        o.push(Change::Archive { chat: 1, on: false });
        o.push(Change::Archive { chat: 2, on: false });
        assert_eq!(o.changes(), [Change::Mute { chat: 1, on: true }, Change::Archive { chat: 1, on: false }, Change::Archive { chat: 2, on: false }]);
        // Sent as it was; one changed again meanwhile stays.
        o.sent(&[Change::Mute { chat: 1, on: true }, Change::Archive { chat: 2, on: true }]);
        assert_eq!(o.changes(), [Change::Archive { chat: 1, on: false }, Change::Archive { chat: 2, on: false }]);
    }

    #[test]
    fn waiting_changes_show_over_older_state() {
        let mut o = Outbox::default();
        o.push(Change::Archive { chat: 7, on: false });
        o.push(Change::Read { chat: 7, up_to: 50 });
        let last = Message { id: 50, chat: 7, ..Message::default() };
        // What Telegram (or the cache) still says: archived, 35 unread.
        let mut c = Chat { id: 7, archived: true, unread: 35, read_inbox: 15, last: Some(Preview::of(&last)), ..Chat::default() };
        o.apply_to(&mut c);
        assert!(!c.archived && c.unread == 0 && c.read_inbox == 50);
        let mut other = Chat { id: 8, archived: true, ..Chat::default() };
        o.apply_to(&mut other);
        assert!(other.archived);
        assert!(o.touches(7) && !o.touches(8));
    }
}
