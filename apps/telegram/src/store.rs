//! What the app knows of the account, kept up to date from the backend's
//! events: the chats (sorted for the list), each chat's messages that
//! have been loaded, its topics, who is typing, the folders, and our
//! messages still on their way.

use std::collections::{BTreeMap, HashMap};

use olo_tg::{Chat, ChatId, Doing, Folder, Message, MsgId, Pending, Topic};

/// A chat's loaded messages (or a topic's).
#[derive(Default)]
pub struct Thread {
    pub msgs: BTreeMap<MsgId, Message>,
    /// Older messages are left to load.
    pub more: bool,
    /// Asked for a page and waiting.
    pub loading: bool,
    /// Has had its first page.
    pub loaded: bool,
    /// Ours being sent: shown after the rest, by their local id.
    pub pending: Vec<Message>,
    /// Bumped whenever anything in it changes (for layout caches).
    pub version: u64,
}

impl Thread {
    pub fn oldest(&self) -> Option<MsgId> {
        self.msgs.keys().next().copied()
    }

    pub fn put(&mut self, m: Message) {
        self.msgs.insert(m.id, m);
        self.version += 1;
    }

    /// Everything in order, ours being sent last.
    pub fn all(&self) -> impl Iterator<Item = &Message> {
        self.msgs.values().chain(self.pending.iter())
    }
}

/// Someone doing something in a chat, until `until`.
pub struct Typing {
    pub who: String,
    pub doing: Doing,
    pub until: f64,
}

#[derive(Default)]
pub struct Store {
    pub chats: HashMap<ChatId, Chat>,
    /// The chat list's order (pinned first, then by latest).
    pub order: Vec<ChatId>,
    /// (chat, topic) → its messages.
    pub threads: HashMap<(ChatId, Option<MsgId>), Thread>,
    pub topics: HashMap<ChatId, Vec<Topic>>,
    pub folders: Vec<Folder>,
    pub typing: HashMap<(ChatId, Option<MsgId>), Vec<Typing>>,
    /// Bumped when the list's order or any row changes.
    pub version: u64,
    /// The whole list has come.
    pub complete: bool,
}

impl Store {
    pub fn put_chat(&mut self, c: Chat) {
        // A cached copy never undoes something newer.
        if let Some(old) = self.chats.get(&c.id)
            && old == &c
        {
            return;
        }
        self.chats.insert(c.id, c);
        self.version += 1;
    }

    /// Sort the list: pinned (in their order), then the latest first.
    pub fn sort(&mut self) {
        let mut ids: Vec<&Chat> = self.chats.values().collect();
        ids.sort_by(|a, b| {
            let pa = (a.pinned && !a.archived, b.pinned && !b.archived);
            pa.1.cmp(&pa.0).then_with(|| if a.pinned && b.pinned { a.pin_order.cmp(&b.pin_order) } else { std::cmp::Ordering::Equal }).then_with(|| b.order.cmp(&a.order)).then_with(|| a.id.cmp(&b.id))
        });
        self.order = ids.into_iter().map(|c| c.id).collect();
        self.version += 1;
    }

    pub fn thread(&mut self, chat: ChatId, topic: Option<MsgId>) -> &mut Thread {
        self.threads.entry((chat, topic)).or_default()
    }

    /// A page of messages has come.
    pub fn page(&mut self, chat: ChatId, topic: Option<MsgId>, msgs: Vec<Message>, more: bool, latest: bool) {
        let t = self.thread(chat, topic);
        if latest {
            // The latest page replaces what was cached, keeping older pages.
            let first = msgs.first().map(|m| m.id);
            if let Some(first) = first {
                t.msgs.retain(|&id, _| id < first);
            }
            if t.msgs.is_empty() {
                t.more = more;
            }
        } else {
            t.more = more;
        }
        for m in msgs {
            t.msgs.insert(m.id, m);
        }
        t.loading = false;
        t.loaded = true;
        t.version += 1;
    }

    /// A message has come (new or edited): into its chat's thread, and
    /// its topic's.
    pub fn message(&mut self, m: Message) {
        if let Some(t) = self.threads.get_mut(&(m.chat, m.topic))
            && m.topic.is_some()
        {
            t.put(m.clone());
        }
        if let Some(t) = self.threads.get_mut(&(m.chat, None)) {
            t.put(m);
        }
    }

    pub fn deleted(&mut self, chat: Option<ChatId>, ids: &[MsgId]) {
        for ((c, _), t) in self.threads.iter_mut() {
            if chat.is_none_or(|x| x == *c) {
                let before = t.msgs.len();
                t.msgs.retain(|id, _| !ids.contains(id));
                if t.msgs.len() != before {
                    t.version += 1;
                }
            }
        }
    }

    /// Ours, shown at once while it's sent.
    pub fn pending(&mut self, m: Message) {
        self.thread(m.chat, None).pending.push(m.clone());
        self.thread(m.chat, None).version += 1;
        if m.topic.is_some() {
            let t = self.thread(m.chat, m.topic);
            t.pending.push(m);
            t.version += 1;
        }
    }

    /// Ours is in Telegram (or failed): out of the pending ones.
    pub fn sent(&mut self, local: u64, msg: Option<Message>) {
        for t in self.threads.values_mut() {
            let before = t.pending.len();
            match &msg {
                Some(_) => t.pending.retain(|p| p.pending.is_none_or(|(l, _)| l != local)),
                None => {
                    for p in &mut t.pending {
                        if p.pending.is_some_and(|(l, _)| l == local) {
                            p.pending = Some((local, Pending::Failed));
                        }
                    }
                }
            }
            if t.pending.len() != before || msg.is_none() {
                t.version += 1;
            }
        }
        if let Some(m) = msg {
            self.message(m);
        }
    }

    pub fn total_unread(&self) -> u32 {
        self.chats.values().filter(|c| !c.muted && !c.archived).map(|c| c.unread + u32::from(c.marked && c.unread == 0)).sum()
    }

    /// Who is doing what in a chat now ("Mira is typing").
    pub fn doing(&self, chat: ChatId, topic: Option<MsgId>, now: f64) -> Option<String> {
        let list = self.typing.get(&(chat, topic)).or_else(|| self.typing.get(&(chat, None)))?;
        let live: Vec<&Typing> = list.iter().filter(|t| t.until > now).collect();
        let first = live.first()?;
        let many = self.chats.get(&chat).is_some_and(|c| c.kind.many());
        Some(match (live.len(), many) {
            (1, false) => format!("{}…", first.doing.describe()),
            (1, true) => format!("{} is {}…", first.who.split(' ').next().unwrap_or(&first.who), first.doing.describe()),
            (2, _) => format!("{} and {} are typing…", first.who.split(' ').next().unwrap_or(""), live[1].who.split(' ').next().unwrap_or("")),
            (n, _) => format!("{n} people are typing…"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: ChatId, order: i64, pinned: bool) -> Chat {
        Chat { id, order, pinned, ..Chat::default() }
    }

    #[test]
    fn list_order() {
        let mut s = Store::default();
        s.put_chat(chat(1, 10, false));
        s.put_chat(chat(2, 30, false));
        s.put_chat(Chat { pin_order: 1, ..chat(3, 5, true) });
        s.put_chat(Chat { pin_order: 0, ..chat(4, 1, true) });
        s.sort();
        assert_eq!(s.order, [4, 3, 2, 1]);
    }

    #[test]
    fn pages_and_pending() {
        let mut s = Store::default();
        let m = |id| Message { id, chat: 7, ..Message::default() };
        s.page(7, None, vec![m(5), m(6)], true, true);
        s.page(7, None, vec![m(3), m(4)], false, false);
        assert_eq!(s.thread(7, None).msgs.keys().copied().collect::<Vec<_>>(), [3, 4, 5, 6]);
        assert!(!s.thread(7, None).more);
        s.pending(Message { pending: Some((1, Pending::Sending)), ..m(0) });
        assert_eq!(s.thread(7, None).all().count(), 5);
        s.sent(1, Some(m(7)));
        assert_eq!(s.thread(7, None).all().map(|m| m.id).collect::<Vec<_>>(), [3, 4, 5, 6, 7]);
        s.deleted(Some(7), &[4]);
        assert_eq!(s.thread(7, None).msgs.len(), 4);
    }
}
