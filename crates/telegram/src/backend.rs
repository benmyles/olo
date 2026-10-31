//! The app's side of Telegram: it sends [`Cmd`]s and hears [`Event`]s.
//! A [`Backend`] runs on threads of its own (a tokio runtime talking
//! MTProto, or the demo account), so nothing the app asks for ever
//! waits on the network: answers stream back as events, each list in
//! pages as they come.

use std::{path::PathBuf, sync::Arc};

use crate::model::*;

/// What the app asks for.
#[derive(Clone, Debug)]
pub enum Cmd {
    /// The chat list (cached first, then Telegram's), and the folders.
    Chats,
    /// A chat's latest messages (or a topic's), and its topics if it's a
    /// forum.
    Open { chat: ChatId, topic: Option<MsgId> },
    /// The page of messages before `before`.
    Older { chat: ChatId, topic: Option<MsgId>, before: MsgId },
    Send(Outgoing),
    Edit { chat: ChatId, id: MsgId, text: String, entities: Vec<Entity> },
    Delete { chat: ChatId, ids: Vec<MsgId>, everyone: bool },
    Forward { from: ChatId, ids: Vec<MsgId>, to: ChatId },
    /// Our reaction to a message (None takes it away).
    React { chat: ChatId, id: MsgId, emoji: Option<String> },
    /// We've read everything up to `up_to`.
    Read { chat: ChatId, up_to: MsgId },
    /// Mark the chat unread, or not.
    MarkUnread { chat: ChatId, on: bool },
    /// We're typing in it (sent at most every few seconds).
    Typing { chat: ChatId, topic: Option<MsgId>, doing: Doing },
    Draft { chat: ChatId, text: String },
    PinChat { chat: ChatId, on: bool },
    Archive { chat: ChatId, on: bool },
    Mute { chat: ChatId, on: bool },
    PinMessage { chat: ChatId, id: MsgId, on: bool },
    /// A picture: a chat's or person's (`Pic::Avatar`), or a message's
    /// thumbnail; or a message's whole file (`Pic::File`), into the cache.
    Fetch(Pic),
    /// Save a message's file as `to` (downloading it if need be).
    SaveFile { chat: ChatId, id: MsgId, to: PathBuf },
    /// Chats, people and messages matching `query`.
    Search { query: String },
    /// A message's place in its chat, to jump to it.
    Around { chat: ChatId, id: MsgId },
    Contacts,
    /// Open a chat with a person or a public chat by username.
    Resolve { username: String },
    SignOut,
}

/// A message to send.
#[derive(Clone, Debug, Default)]
pub struct Outgoing {
    pub chat: ChatId,
    pub topic: Option<MsgId>,
    pub text: String,
    pub entities: Vec<Entity>,
    pub reply_to: Option<MsgId>,
    /// Files sent with it (pictures as photos unless `as_files`).
    pub files: Vec<PathBuf>,
    pub as_files: bool,
    /// A voice message: the Ogg Opus file and its waveform.
    pub voice: Option<(PathBuf, f32, Vec<u8>)>,
    /// Ours, to match it to its pending bubble.
    pub local: u64,
}

/// A picture or file to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pic {
    /// A chat's or a person's picture (small).
    Avatar(ChatId),
    /// A message's picture (a photo's, a video's first frame, a
    /// sticker), as large as a bubble shows it.
    Thumb(ChatId, MsgId),
    /// A message's whole file.
    File(ChatId, MsgId),
}

/// How the connection is.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No API ID in Preferences ▸ Integrations yet.
    NotSetUp,
    /// No one is signed in.
    SignedOut,
    Connecting,
    /// Catching up on what happened while away.
    Updating,
    Online,
    /// Lost, trying again in a moment (why).
    Offline(String),
    /// Telegram asked us to wait this many seconds.
    Waiting(u32),
}

/// What the backend says.
#[derive(Clone, Debug)]
pub enum Event {
    Status(Status),
    Me(Me),
    /// A page of the chat list; `done` once it's all here (`cached` while
    /// it's what was kept from last time).
    Chats { chats: Vec<Chat>, done: bool, cached: bool },
    /// One chat changed (a message came, it was read, renamed, …).
    Chat(Chat),
    ChatGone(ChatId),
    Folders(Vec<Folder>),
    /// A page of messages, newest last; `more`: older ones are left.
    /// `before` is the page asked for (None: the latest).
    Messages { chat: ChatId, topic: Option<MsgId>, msgs: Vec<Message>, more: bool, before: Option<MsgId> },
    /// A page round a message (`Cmd::Around`).
    Around { chat: ChatId, id: MsgId, msgs: Vec<Message>, more: bool },
    /// A new message (theirs, or ours sent from elsewhere).
    New(Message),
    /// Ours, now in Telegram.
    Sent { local: u64, msg: Message },
    SendFailed { local: u64, chat: ChatId, why: String },
    Edited(Message),
    Deleted { chat: Option<ChatId>, ids: Vec<MsgId> },
    /// Read up to `up_to`: ours by them (`outbox`) or theirs by us.
    Read { chat: ChatId, outbox: bool, up_to: MsgId },
    Doing { chat: ChatId, topic: Option<MsgId>, who: String, doing: Doing },
    Topics { chat: ChatId, topics: Vec<Topic> },
    /// A picture or file is in the cache, at `path`.
    Fetched { pic: Pic, path: PathBuf },
    /// How far a download or upload is (0–1).
    Progress { pic: Option<Pic>, local: Option<u64>, done: f32 },
    FetchFailed { pic: Pic, why: String },
    Saved { to: PathBuf },
    Results { query: String, chats: Vec<Chat>, msgs: Vec<Message> },
    Contacts(Vec<Contact>),
    /// `Resolve`'s answer.
    Resolved { username: String, chat: Option<Chat> },
    /// Something went wrong that the person should hear about.
    Error(String),
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// A running backend: send it commands.
#[derive(Clone)]
pub struct Backend {
    to: tokio::sync::mpsc::UnboundedSender<Cmd>,
}

impl Backend {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.to.send(cmd);
    }

    /// Start the backend: the demo account when `OLO_TELEGRAM_DEMO` is
    /// set, else the account signed in (or the status that says why
    /// there's none). Events go to `sink`, on the backend's threads.
    pub fn start(sink: impl Fn(Event) + Send + Sync + 'static) -> Backend {
        let (to, from) = tokio::sync::mpsc::unbounded_channel();
        let sink: Sink = Arc::new(sink);
        if std::env::var_os("OLO_TELEGRAM_DEMO").is_some() {
            crate::demo::spawn(from, sink);
        } else {
            crate::net::spawn(from, sink);
        }
        Backend { to }
    }
}
