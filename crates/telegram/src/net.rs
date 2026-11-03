//! The real account: MTProto through grammers on a tokio runtime of the
//! backend's own. Reading is done with Telegram's own calls (raw TL), so
//! everything the official apps show is within reach (archived chats,
//! folders, topics, read states, drafts, reactions); sending, editing,
//! downloading and uploading go through grammers' client. Every user and
//! chat a reply names is remembered (their access hashes in the
//! session, their names here) so messages can be shown with who sent
//! them. The chat list and each chat's latest messages are cached on
//! disk, so the app shows them at once next time while Telegram is
//! asked again.

use olo_sync::Mutex;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use grammers_client::{Client, InvocationError, media::Attribute, message::InputMessage};
use grammers_mtsender::{ConnectionParams, SenderPool};
use grammers_session::{
    Session,
    types::{PeerId, PeerInfo, PeerKind, PeerRef},
};
use grammers_tl_types as tl;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::outbox::{Change, Outbox};
use crate::{
    account::{self, Store},
    backend::{Cmd, Event, Outgoing, Pic, Sink, Status},
    model::*,
};

/// What's known of a user or chat, from the replies that named them.
#[derive(Clone, Debug, Default)]
struct Who {
    name: String,
    username: Option<String>,
    photo: Option<i64>,
    kind: ChatKind,
    contact: bool,
    verified: bool,
    seen: Option<Seen>,
    members: Option<u32>,
}

struct Ctx {
    client: Client,
    sink: Sink,
    store: Arc<Store>,
    me: AtomicI64,
    who: Mutex<HashMap<ChatId, Who>>,
    chats: Mutex<HashMap<ChatId, Chat>>,
    /// One load of the chat list at a time (a pin made elsewhere asks for
    /// another while the first is under way).
    loading: tokio::sync::Mutex<()>,
    /// The chat list changed: it's written to disk (`cache_due` wakes the
    /// writer), a second after the person's own change or a whole list
    /// (`cache_soon`), else within half a minute (a message come in), and
    /// only once `chats` holds all of it (the cache read in, or a whole
    /// list loaded: `cache_ready`).
    cache_due: tokio::sync::Notify,
    cache_soon: tokio::sync::Notify,
    cache_ready: std::sync::atomic::AtomicBool,
    /// Each message's media as Telegram described it (to download it).
    media: Mutex<HashMap<(ChatId, MsgId), tl::enums::MessageMedia>>,
    fetching: Mutex<HashSet<Pic>>,
    /// Chats an update changed the state of (archived, read, marked,
    /// muted, pinned): read again from Telegram in a moment, in batches
    /// (`confirm`), so what it has now wins over an update replayed or
    /// come out of order.
    confirming: Mutex<HashSet<ChatId>>,
    confirm_due: tokio::sync::Notify,
    /// The person's changes not yet in Telegram (`outbox`), and what
    /// wakes the task sending them.
    outbox: Mutex<Outbox>,
    outbox_due: tokio::sync::Notify,
    /// How many there were when the sending began (for "3 of 7").
    outbox_total: AtomicI64,
    /// The chat list has been read whole from Telegram this run (a chat
    /// not in it is gone).
    list_whole: std::sync::atomic::AtomicBool,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn id_of(peer: &tl::enums::Peer) -> ChatId {
    PeerId::from(peer).bot_api_dialog_id_unchecked()
}

/// The folder `chat` is in, as Telegram's answer to moving it says (its
/// `updateFolderPeers`), if it says.
fn folder_in(r: &tl::enums::Updates, chat: ChatId) -> Option<i32> {
    let updates = match r {
        tl::enums::Updates::Updates(u) => &u.updates,
        tl::enums::Updates::Combined(u) => &u.updates,
        tl::enums::Updates::UpdateShort(u) => std::slice::from_ref(&u.update),
        _ => return None,
    };
    updates.iter().find_map(|u| match u {
        tl::enums::Update::FolderPeers(f) => f.folder_peers.iter().find_map(|p| {
            let tl::enums::FolderPeer::Peer(p) = p;
            (id_of(&p.peer) == chat).then_some(p.folder_id)
        }),
        _ => None,
    })
}

fn params() -> ConnectionParams {
    let lang = std::env::var("LANG").ok().and_then(|l| l.split(['_', '.']).next().map(str::to_owned)).filter(|l| l.len() == 2).unwrap_or_else(|| "en".into());
    ConnectionParams {
        device_model: "Olo".into(),
        system_version: "Linux".into(),
        app_version: format!("Olo Telegram {}", env!("CARGO_PKG_VERSION")),
        system_lang_code: lang.clone(),
        lang_code: lang,
        ..ConnectionParams::default()
    }
}

/// A client for `api` on `store`, its sender pool running; and the
/// updates that come in.
pub fn connect(store: Arc<Store>, api: &account::Api) -> (Client, tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>) {
    let pool = SenderPool::with_configuration(store, api.id, params());
    let client = Client::new(pool.handle);
    tokio::spawn(pool.runner.run());
    (client, pool.updates)
}

/// What an error from Telegram means for the person.
pub fn explain(e: &InvocationError) -> String {
    match e {
        InvocationError::Rpc(rpc) => match rpc.name.as_str() {
            "FLOOD_WAIT" => format!("Telegram asks us to wait {} seconds.", rpc.value.unwrap_or(0)),
            "PHONE_NUMBER_INVALID" => "That phone number isn’t valid.".into(),
            "PHONE_CODE_INVALID" => "That code isn’t right.".into(),
            "PHONE_CODE_EXPIRED" => "That code has expired; ask for another.".into(),
            "PASSWORD_HASH_INVALID" => "That password isn’t right.".into(),
            "API_ID_INVALID" => "Telegram doesn’t know that API ID and hash.".into(),
            "AUTH_KEY_UNREGISTERED" | "SESSION_REVOKED" => "This session was signed out (from another device?).".into(),
            "CHAT_WRITE_FORBIDDEN" => "You can’t write in this chat.".into(),
            "MESSAGE_NOT_MODIFIED" => "Nothing changed.".into(),
            name => format!("Telegram said {name}."),
        },
        InvocationError::Io(e) => format!("Telegram couldn’t be reached ({e})."),
        other => other.to_string(),
    }
}

pub fn spawn(from: UnboundedReceiver<Cmd>, sink: Sink) {
    let _ = std::thread::Builder::new().name("telegram".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("telegram-net").enable_all().build().expect("a runtime");
        rt.block_on(run(from, sink));
    });
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("a blocking task")
}

/// Why a change didn't get to Telegram.
#[derive(Debug)]
enum Failed {
    /// Not now (asked to wait: `flood`, its seconds; offline; a chat not
    /// known yet): again after the wait.
    Later(Duration, Option<u32>),
    /// Telegram won't take it (the chat is gone, not ours to change).
    Refused(String),
}

impl Failed {
    fn of(e: &InvocationError) -> Failed {
        match e {
            InvocationError::Rpc(r) if r.code == 420 => {
                let s = r.value.unwrap_or(5);
                Failed::Later(Duration::from_secs(u64::from(s)), Some(s))
            }
            InvocationError::Rpc(r) if (400..500).contains(&r.code) => Failed::Refused(explain(e)),
            _ => Failed::Later(Duration::from_secs(5), None),
        }
    }
}

/// Whether incoming message `m` makes one more unread in chat `c`: newer
/// than any message the row knows and than what's read. An update
/// replayed (caught up on) after the list was read counts nothing again.
fn counts_unread(c: &Chat, m: &Message) -> bool {
    !m.out && m.id > c.read_inbox && c.last.as_ref().is_none_or(|l| l.id < m.id)
}

/// Whether an update saying chat `c` was read up to `up_to` is news: one
/// no newer than what's known read (replayed) carries a count of those
/// left unread older than ours.
fn read_moves_on(c: &Chat, up_to: MsgId) -> bool {
    up_to > c.read_inbox
}

/// How far back starting again catches up on updates: further, the
/// chat list read whole is all that's needed.
const CATCH_UP_SECS: i64 = 6 * 3600;

async fn run(mut from: UnboundedReceiver<Cmd>, sink: Sink) {
    let api = match blocking(account::api).await {
        Ok(Some(api)) => api,
        Ok(None) => {
            sink(Event::Status(Status::NotSetUp));
            return idle(from).await;
        }
        Err(e) => {
            sink(Event::Status(Status::Offline(format!("The keyring couldn’t be read ({e})."))));
            return idle(from).await;
        }
    };
    let store = match blocking(Store::load).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            sink(Event::Status(Status::SignedOut));
            return idle(from).await;
        }
        Err(e) => {
            sink(Event::Status(Status::Offline(e)));
            return idle(from).await;
        }
    };
    if let Some(me) = store.me() {
        sink(Event::Me(me));
    }
    sink(Event::Status(Status::Connecting));
    let (client, updates) = connect(store.clone(), &api);
    let ctx = Arc::new(Ctx {
        client,
        sink: sink.clone(),
        store: store.clone(),
        me: AtomicI64::new(store.me().map_or(0, |m| m.id)),
        who: Mutex::new(HashMap::new()),
        chats: Mutex::new(HashMap::new()),
        loading: tokio::sync::Mutex::new(()),
        cache_due: tokio::sync::Notify::new(),
        cache_soon: tokio::sync::Notify::new(),
        cache_ready: std::sync::atomic::AtomicBool::new(false),
        media: Mutex::new(HashMap::new()),
        fetching: Mutex::new(HashSet::new()),
        confirming: Mutex::new(HashSet::new()),
        confirm_due: tokio::sync::Notify::new(),
        outbox: Mutex::new(blocking(Outbox::load).await),
        outbox_due: tokio::sync::Notify::new(),
        outbox_total: AtomicI64::new(0),
        list_whole: std::sync::atomic::AtomicBool::new(false),
    });
    ctx.emit_sync(None);
    // Chats' states as Telegram has them now, for those updates changed.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            loop {
                ctx.confirm_due.notified().await;
                // (The rest of the burst: a catch-up, an archive of many.)
                tokio::time::sleep(Duration::from_millis(300)).await;
                loop {
                    let ids: Vec<ChatId> = {
                        let mut c = ctx.confirming.lock().unwrap();
                        let ids: Vec<ChatId> = c.iter().copied().take(100).collect();
                        for id in &ids {
                            c.remove(id);
                        }
                        ids
                    };
                    if ids.is_empty() {
                        break;
                    }
                    ctx.confirm_now(&ids).await;
                }
            }
        });
    }
    // The chat list on disk follows it (a chat archived or not, a page
    // loaded), so starting again shows what was last known, even when the
    // last load never finished: it was written only after a whole list,
    // and a chat unarchived went back to the archive the cache had it in.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            loop {
                ctx.cache_due.notified().await;
                tokio::select! {
                    _ = ctx.cache_soon.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
                // (The rest of the same change.)
                tokio::time::sleep(Duration::from_secs(1)).await;
                if ctx.cache_ready.load(Ordering::Relaxed) {
                    let chats: Vec<Chat> = ctx.chats.lock().unwrap().values().cloned().collect();
                    blocking(move || Ctx::cache_chats(chats)).await;
                }
            }
        });
    }
    // Who we are, retried until Telegram answers.
    let mut wait = 1;
    loop {
        match ctx.client.get_me().await {
            Ok(user) => {
                let u = &user.raw;
                ctx.remember_users(std::slice::from_ref(u)).await;
                let id = PeerId::from(&tl::enums::Peer::User(tl::types::PeerUser { user_id: user.id().bare_id_unchecked() })).bot_api_dialog_id_unchecked();
                ctx.me.store(id, Ordering::Relaxed);
                let me = Me { id, name: user.full_name(), phone: user.phone().map(|p| format!("+{p}")), username: user.username().map(str::to_owned), photo: user.photo().map(|p| p.photo_id) };
                store.set_me(me.clone());
                sink(Event::Me(me));
                break;
            }
            Err(e) if matches!(&e, InvocationError::Rpc(r) if r.name == "AUTH_KEY_UNREGISTERED" || r.name == "SESSION_REVOKED" || r.name == "USER_DEACTIVATED") => {
                sink(Event::Status(Status::SignedOut));
                sink(Event::Error(explain(&e)));
                let _ = blocking(account::forget_session).await;
                return idle(from).await;
            }
            Err(e) => {
                sink(Event::Status(Status::Offline(explain(&e))));
                tokio::time::sleep(Duration::from_secs(wait)).await;
                wait = (wait * 2).min(60);
            }
        }
    }
    sink(Event::Status(Status::Updating));
    // The person's changes, sent as they're made, and what was left
    // unsent last time now.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            loop {
                ctx.send_outbox().await;
                ctx.outbox_due.notified().await;
            }
        });
    }
    // Save the session now and then (the keyring only when it changed).
    {
        let store = store.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let s = store.clone();
                if let Err(e) = blocking(move || s.save(false)).await {
                    tracing::warn!("telegram: the session couldn’t be saved: {e}");
                }
            }
        });
    }
    // Updates.
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            // What came while Olo's Telegram was closed, from where it
            // last got to; not from days back (the chat list is read
            // whole anyway, and replaying days of messages and moves
            // only lands old state on it).
            let age = now() - ctx.store.updates_date();
            // (Starting from now needs the account among the peers kept;
            // without it, catching up is still safe, only slower.)
            let catch_up = age < CATCH_UP_SECS || !ctx.store.knows_self();
            if !catch_up {
                tracing::info!("telegram: the updates were last caught up {} h ago: starting from now", age / 3600);
            }
            let cfg = grammers_mtsender::UpdatesConfiguration { catch_up, ..Default::default() };
            let mut stream = match ctx.client.stream_updates(updates, cfg).await {
                Ok(s) => s,
                Err(e) => {
                    (ctx.sink)(Event::Error(format!("Updates couldn’t start ({e}).")));
                    return;
                }
            };
            (ctx.sink)(Event::Status(Status::Online));
            loop {
                match stream.next_raw().await {
                    Ok((update, _, peers)) => {
                        for p in peers.iter_peers() {
                            ctx.remember_peer(p).await;
                        }
                        ctx.update(update).await;
                        // Where we've got to, into the session (on disk
                        // with its next save): grammers doesn't keep it
                        // itself, and without it every start replayed
                        // everything since signing in.
                        if let Err(e) = stream.sync_update_state().await {
                            tracing::warn!("telegram: the updates' state couldn't be kept: {e}");
                        }
                    }
                    Err(e) => {
                        (ctx.sink)(Event::Status(Status::Offline(explain(&e))));
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        (ctx.sink)(Event::Status(Status::Online));
                        // Back: the changes waiting go now.
                        ctx.outbox_due.notify_one();
                    }
                }
            }
        });
    }
    while let Some(cmd) = from.recv().await {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = ctx.handle(cmd).await {
                (ctx.sink)(Event::Error(e));
            }
            // What the person changed is on disk in a moment.
            ctx.cache_soon.notify_one();
        });
    }
    let _ = blocking(move || store.save(false)).await;
}

/// No account to talk to: answer what can be answered.
async fn idle(mut from: UnboundedReceiver<Cmd>) {
    while from.recv().await.is_some() {}
}

/// A picture's file name in the cache.
fn media_dir() -> PathBuf {
    account::dir().join("media")
}

fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect()
}

impl Ctx {
    fn me(&self) -> i64 {
        self.me.load(Ordering::Relaxed)
    }

    fn emit(&self, e: Event) {
        (self.sink)(e);
    }

    async fn remember_users(&self, users: &[tl::enums::User]) {
        let mut infos = Vec::new();
        for u in users {
            infos.push(PeerInfo::from(u));
            if let tl::enums::User::User(u) = u {
                let id = u.id;
                let name = [u.first_name.as_deref(), u.last_name.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                let name = if u.deleted { "Deleted Account".to_owned() } else if name.is_empty() { "Unknown".into() } else { name };
                let seen = u.status.as_ref().and_then(seen_of);
                let photo = match &u.photo {
                    Some(tl::enums::UserProfilePhoto::Photo(p)) => Some(p.photo_id),
                    _ => None,
                };
                let kind = if u.is_self { ChatKind::Saved } else if u.bot { ChatKind::Bot } else { ChatKind::User };
                let w = Who { name, username: u.username.clone(), photo, kind, contact: u.contact, verified: u.verified, seen, members: None };
                let mut who = self.who.lock().unwrap();
                // A "min" user carries less than we may know already.
                if u.min
                    && let Some(old) = who.get(&id)
                {
                    let mut w = w;
                    w.contact = old.contact;
                    w.seen = w.seen.or(old.seen.clone());
                    who.insert(id, w);
                } else {
                    who.insert(id, w);
                }
            }
        }
        let _ = self.store.cache_peers(infos).await;
    }

    async fn remember_chats(&self, chats: &[tl::enums::Chat]) {
        let mut infos = Vec::new();
        for c in chats {
            infos.push(PeerInfo::from(c));
            let (id, w) = match c {
                tl::enums::Chat::Chat(c) => {
                    let photo = match &c.photo {
                        tl::enums::ChatPhoto::Photo(p) => Some(p.photo_id),
                        _ => None,
                    };
                    (-c.id, Who { name: c.title.clone(), photo, kind: ChatKind::Group, members: Some(c.participants_count.max(0) as u32), ..Who::default() })
                }
                tl::enums::Chat::Forbidden(c) => (-c.id, Who { name: c.title.clone(), kind: ChatKind::Group, ..Who::default() }),
                tl::enums::Chat::Channel(c) => {
                    let photo = match &c.photo {
                        tl::enums::ChatPhoto::Photo(p) => Some(p.photo_id),
                        _ => None,
                    };
                    let kind = if c.forum { ChatKind::Forum } else if c.megagroup || c.gigagroup { ChatKind::Group } else { ChatKind::Channel };
                    let members = c.participants_count.map(|n| n.max(0) as u32);
                    (-1_000_000_000_000 - c.id, Who { name: c.title.clone(), username: c.username.clone(), photo, kind, verified: c.verified, members, ..Who::default() })
                }
                tl::enums::Chat::ChannelForbidden(c) => (-1_000_000_000_000 - c.id, Who { name: c.title.clone(), kind: if c.megagroup { ChatKind::Group } else { ChatKind::Channel }, ..Who::default() }),
                _ => continue,
            };
            let mut who = self.who.lock().unwrap();
            let members = w.members.or(who.get(&id).and_then(|o| o.members));
            who.insert(id, Who { members, ..w });
        }
        let _ = self.store.cache_peers(infos).await;
    }

    async fn remember_peer(&self, p: &grammers_client::peer::Peer) {
        use grammers_client::peer::Peer;
        match p {
            Peer::User(u) => self.remember_users(std::slice::from_ref(&u.raw)).await,
            Peer::Group(g) => self.remember_chats(std::slice::from_ref(&g.raw)).await,
            Peer::Channel(c) => self.remember_chats(&[tl::enums::Chat::Channel(c.raw.clone())]).await,
            _ => {}
        }
    }

    fn name_of(&self, id: ChatId) -> Option<String> {
        if id == self.me() {
            return Some("You".into());
        }
        self.who.lock().unwrap().get(&id).map(|w| w.name.clone())
    }

    async fn peer(&self, chat: ChatId) -> Result<PeerRef, String> {
        let id = PeerId::from_bot_api_dialog_id(chat).ok_or("Not a chat Telegram knows.")?;
        if chat == self.me() {
            return Ok(PeerRef { id: PeerId::self_user(), auth: Default::default() });
        }
        self.store.peer_ref(id).await.ok().flatten().ok_or_else(|| "This chat isn’t known yet; open the chat list first.".into())
    }

    fn input_peer(r: &PeerRef) -> tl::enums::InputPeer {
        r.into()
    }

    /// A raw message, as the app shows it (None for an empty one).
    fn convert(&self, raw: &tl::enums::Message) -> Option<Message> {
        let me = self.me();
        match raw {
            tl::enums::Message::Empty(_) => None,
            tl::enums::Message::Message(m) => {
                let chat = id_of(&m.peer_id);
                let from_id = m.from_id.as_ref().map(id_of).or(if m.out { Some(me) } else if chat > 0 { Some(chat) } else { None });
                let from = from_id.map(|id| {
                    let w = self.who.lock().unwrap().get(&id).cloned();
                    Sender { id, name: if id == me { "You".into() } else { w.as_ref().map_or_else(|| "Unknown".into(), |w| w.name.clone()) }, photo: w.and_then(|w| w.photo) }
                });
                let (reply_to, topic) = match &m.reply_to {
                    Some(tl::enums::MessageReplyHeader::Header(h)) => {
                        let topic = if h.forum_topic { h.reply_to_top_id.or(h.reply_to_msg_id) } else { None };
                        // In a forum, a reply only to the topic's first message is no reply.
                        let reply = h.reply_to_msg_id.filter(|&r| !(h.forum_topic && h.reply_to_top_id.is_none() && Some(r) == topic)).map(|id| ReplyTo {
                            id,
                            from: None,
                            text: h.quote_text.clone(),
                        });
                        (reply, topic)
                    }
                    _ => (None, None),
                };
                let forwarded = m.fwd_from.as_ref().map(|f| match f {
                    tl::enums::MessageFwdHeader::Header(h) => h.from_name.clone().or_else(|| h.from_id.as_ref().and_then(|p| self.name_of(id_of(p)))).or(h.post_author.clone()).unwrap_or_else(|| "Unknown".into()),
                });
                let media = m.media.as_ref().and_then(|md| {
                    self.media.lock().unwrap().insert((chat, m.id), md.clone());
                    media_of(md)
                });
                let reactions = match &m.reactions {
                    Some(tl::enums::MessageReactions::Reactions(r)) => r
                        .results
                        .iter()
                        .filter_map(|c| {
                            let tl::enums::ReactionCount::Count(c) = c;
                            let emoji = match &c.reaction {
                                tl::enums::Reaction::Emoji(e) => e.emoticon.clone(),
                                tl::enums::Reaction::CustomEmoji(_) => "⭐".into(),
                                _ => return None,
                            };
                            Some(Reaction { emoji, count: c.count.max(0) as u32, mine: c.chosen_order.is_some() })
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let replies = match &m.replies {
                    Some(tl::enums::MessageReplies::Replies(r)) => Some(r.replies.max(0) as u32),
                    None => None,
                };
                Some(Message {
                    id: m.id,
                    chat,
                    from,
                    date: m.date as i64,
                    edited: m.edit_date.filter(|_| !m.edit_hide).map(|d| d as i64),
                    out: m.out,
                    text: m.message.clone(),
                    entities: m.entities.as_deref().map(|e| entities_of(&m.message, e)).unwrap_or_default(),
                    reply_to,
                    forwarded,
                    media,
                    reactions,
                    views: m.views.map(|v| v.max(0) as u32),
                    replies,
                    topic,
                    service: None,
                    pinned: m.pinned,
                    mentioned: m.mentioned,
                    album: m.grouped_id,
                    via_bot: m.via_bot_id.and_then(|b| self.name_of(b)),
                    pending: None,
                })
            }
            tl::enums::Message::Service(s) => {
                let chat = id_of(&s.peer_id);
                let actor = s.from_id.as_ref().map(id_of).and_then(|id| self.name_of(id)).unwrap_or_else(|| "Someone".into());
                let topic = match &s.reply_to {
                    Some(tl::enums::MessageReplyHeader::Header(h)) if h.forum_topic => h.reply_to_top_id.or(h.reply_to_msg_id),
                    _ => None,
                };
                Some(Message { id: s.id, chat, date: s.date as i64, out: s.out, service: Some(self.describe(&actor, &s.action)), topic, ..Message::default() })
            }
        }
    }

    /// What a service message says.
    fn describe(&self, actor: &str, action: &tl::enums::MessageAction) -> String {
        use tl::enums::MessageAction as A;
        let names = |ids: &[i64]| ids.iter().filter_map(|&id| self.name_of(id)).collect::<Vec<_>>().join(", ");
        match action {
            A::ChatCreate(c) => format!("{actor} created the group “{}”", c.title),
            A::ChannelCreate(c) => format!("Channel “{}” created", c.title),
            A::ChatEditTitle(c) => format!("{actor} renamed the group to “{}”", c.title),
            A::ChatEditPhoto(_) => format!("{actor} changed the group’s picture"),
            A::ChatAddUser(a) => {
                let who = names(&a.users);
                if who.is_empty() || who == actor { format!("{actor} joined") } else { format!("{actor} added {who}") }
            }
            A::ChatDeleteUser(d) => {
                let who = self.name_of(d.user_id).unwrap_or_default();
                if who == actor { format!("{actor} left") } else { format!("{actor} removed {who}") }
            }
            A::ChatJoinedByLink(_) => format!("{actor} joined by link"),
            A::PhoneCall(c) => {
                let missed = matches!(c.reason, Some(tl::enums::PhoneCallDiscardReason::Missed));
                let kind = if c.video { "Video call" } else { "Call" };
                match c.duration {
                    Some(d) if !missed => format!("{kind} ({})", clock(d as f32)),
                    _ => format!("Missed {}", kind.to_lowercase()),
                }
            }
            A::TopicCreate(t) => format!("{actor} created the topic “{}”", t.title),
            A::TopicEdit(t) => match &t.title {
                Some(title) => format!("{actor} renamed the topic to “{title}”"),
                None => format!("{actor} changed the topic"),
            },
            A::ChatMigrateTo(_) | A::ChannelMigrateFrom(_) => "The group was upgraded to a supergroup".into(),
            A::SetMessagesTtl(t) if t.period == 0 => format!("{actor} turned off auto-delete"),
            A::SetMessagesTtl(_) => format!("{actor} turned on auto-delete"),
            A::GroupCall(c) => match c.duration {
                Some(d) => format!("Voice chat ended ({})", clock(d as f32)),
                None => format!("{actor} started a voice chat"),
            },
            A::CustomAction(c) => c.message.clone(),
            A::PinMessage => format!("{actor} pinned a message"),
            A::ScreenshotTaken => format!("{actor} took a screenshot"),
            A::ContactSignUp => format!("{actor} joined Telegram"),
            _ => format!("{actor} did something this app can’t show yet"),
        }
    }

    fn chat_of(&self, d: &tl::types::Dialog, last: Option<&Message>, archived: bool) -> Chat {
        let id = id_of(&d.peer);
        let w = self.who.lock().unwrap().get(&id).cloned().unwrap_or_default();
        let muted = match &d.notify_settings {
            tl::enums::PeerNotifySettings::Settings(s) => s.mute_until.is_some_and(|t| t as i64 > now()),
        };
        let draft = match &d.draft {
            Some(tl::enums::DraftMessage::Message(m)) if !m.message.is_empty() => Some((m.message.clone(), m.date as i64)),
            _ => None,
        };
        let order = last.map_or(0, |m| m.date).max(draft.as_ref().map_or(0, |d| d.1));
        let kind = if id == self.me() { ChatKind::Saved } else { w.kind };
        Chat {
            id,
            kind,
            title: if kind == ChatKind::Saved { "Saved Messages".into() } else { w.name },
            username: w.username,
            photo: w.photo,
            unread: d.unread_count.max(0) as u32,
            mentions: d.unread_mentions_count.max(0) as u32,
            marked: d.unread_mark,
            pinned: d.pinned,
            archived: archived || d.folder_id == Some(1),
            muted,
            verified: w.verified,
            contact: w.contact,
            // We've written if they've read something of ours, or the
            // latest is ours.
            unknown: crate::model::stranger(id, kind, w.contact, w.verified, d.read_outbox_max_id > 0 || last.is_some_and(|m| m.out)),
            last: last.map(Preview::of),
            draft: draft.map(|d| d.0),
            order,
            read_outbox: d.read_outbox_max_id,
            read_inbox: d.read_inbox_max_id,
            members: w.members,
            seen: w.seen,
            pin_order: 0,
        }
    }

    /// One page of dialogs: the chats and the offset for the next page.
    async fn dialogs(&self, folder: Option<i32>, offset: (i32, i32, tl::enums::InputPeer)) -> Result<(Vec<Chat>, Option<(i32, i32, tl::enums::InputPeer)>), InvocationError> {
        let r = self
            .client
            .invoke(&tl::functions::messages::GetDialogs { exclude_pinned: false, folder_id: folder, offset_date: offset.0, offset_id: offset.1, offset_peer: offset.2, limit: 100, hash: 0 })
            .await?;
        let (dialogs, messages, chats, users, total) = match r {
            tl::enums::messages::Dialogs::Dialogs(d) => (d.dialogs, d.messages, d.chats, d.users, None),
            tl::enums::messages::Dialogs::Slice(d) => (d.dialogs, d.messages, d.chats, d.users, Some(d.count)),
            tl::enums::messages::Dialogs::NotModified(_) => return Ok((Vec::new(), None)),
        };
        self.remember_users(&users).await;
        self.remember_chats(&chats).await;
        let msgs: HashMap<(ChatId, MsgId), Message> = messages.iter().filter_map(|m| self.convert(m)).map(|m| ((m.chat, m.id), m)).collect();
        let mut out = Vec::new();
        let mut pin = 0;
        let mut last_offset = None;
        for d in &dialogs {
            let tl::enums::Dialog::Dialog(d) = d else { continue };
            let id = id_of(&d.peer);
            let last = msgs.get(&(id, d.top_message));
            let mut c = self.chat_of(d, last, folder == Some(1));
            if c.pinned {
                c.pin_order = pin;
                pin += 1;
            }
            if let Some(m) = last {
                let peer = self.peer(id).await.ok();
                last_offset = peer.map(|p| (m.date as i32, m.id, Self::input_peer(&p)));
            }
            out.push(c);
        }
        let more = match total {
            Some(n) => dialogs.len() == 100 && (out.len() as i32) < n,
            None => false,
        };
        Ok((out, if more { last_offset } else { None }))
    }

    async fn folders(&self) -> Result<Vec<Folder>, InvocationError> {
        let tl::enums::messages::DialogFilters::Filters(f) = self.client.invoke(&tl::functions::messages::GetDialogFilters {}).await?;
        let ids = |peers: &[tl::enums::InputPeer]| peers.iter().map(|p| PeerRef::from(p).id.bot_api_dialog_id_unchecked()).collect::<Vec<_>>();
        Ok(f.filters
            .iter()
            .filter_map(|f| match f {
                tl::enums::DialogFilter::Filter(f) => {
                    let tl::enums::TextWithEntities::Entities(t) = &f.title;
                    Some(Folder {
                        id: f.id,
                        title: t.text.clone(),
                        emoticon: f.emoticon.clone(),
                        contacts: f.contacts,
                        non_contacts: f.non_contacts,
                        groups: f.groups,
                        channels: f.broadcasts,
                        bots: f.bots,
                        exclude_muted: f.exclude_muted,
                        exclude_read: f.exclude_read,
                        exclude_archived: f.exclude_archived,
                        include: ids(&f.include_peers),
                        exclude: ids(&f.exclude_peers),
                        pinned: ids(&f.pinned_peers),
                    })
                }
                tl::enums::DialogFilter::Chatlist(f) => {
                    let tl::enums::TextWithEntities::Entities(t) = &f.title;
                    Some(Folder { id: f.id, title: t.text.clone(), emoticon: f.emoticon.clone(), include: ids(&f.include_peers), pinned: ids(&f.pinned_peers), ..Folder::default() })
                }
                _ => None,
            })
            .collect())
    }

    /// The messages in a `messages.Messages` reply, oldest first.
    async fn messages_of(&self, r: tl::enums::messages::Messages) -> (Vec<Message>, Option<i32>) {
        let (msgs, users, chats, count) = match r {
            tl::enums::messages::Messages::Messages(m) => (m.messages, m.users, m.chats, None),
            tl::enums::messages::Messages::Slice(m) => (m.messages, m.users, m.chats, Some(m.count)),
            tl::enums::messages::Messages::ChannelMessages(m) => (m.messages, m.users, m.chats, Some(m.count)),
            tl::enums::messages::Messages::NotModified(_) => return (Vec::new(), None),
        };
        self.remember_users(&users).await;
        self.remember_chats(&chats).await;
        let mut out: Vec<Message> = msgs.iter().filter_map(|m| self.convert(m)).collect();
        out.sort_by_key(|m| m.id);
        self.fill_replies(&mut out);
        (out, count)
    }

    /// Replies' quotes, from the messages at hand.
    fn fill_replies(&self, msgs: &mut [Message]) {
        let by_id: HashMap<MsgId, (Option<String>, String)> = msgs.iter().map(|m| (m.id, (m.from.as_ref().map(|f| f.name.clone()), m.summary()))).collect();
        for m in msgs.iter_mut() {
            if let Some(r) = &mut m.reply_to
                && let Some((from, text)) = by_id.get(&r.id)
            {
                r.from = from.clone();
                if r.text.is_none() {
                    r.text = Some(text.clone());
                }
            }
        }
    }

    /// Quotes for replies to messages not in the page, asked for.
    async fn fetch_replies(&self, chat: ChatId, msgs: &mut [Message]) {
        let missing: Vec<MsgId> = msgs.iter().filter_map(|m| m.reply_to.as_ref()).filter(|r| r.from.is_none()).map(|r| r.id).collect::<HashSet<_>>().into_iter().collect();
        if missing.is_empty() {
            return;
        }
        let Ok(peer) = self.peer(chat).await else { return };
        let ids: Vec<tl::enums::InputMessage> = missing.iter().map(|&id| tl::enums::InputMessage::Id(tl::types::InputMessageId { id })).collect();
        let r = if peer.id.kind() == PeerKind::Channel {
            self.client.invoke(&tl::functions::channels::GetMessages { channel: (&peer).into(), id: ids }).await
        } else {
            self.client.invoke(&tl::functions::messages::GetMessages { id: ids }).await
        };
        let Ok(r) = r else { return };
        let (got, _) = self.messages_of(r).await;
        let by_id: HashMap<MsgId, &Message> = got.iter().map(|m| (m.id, m)).collect();
        for m in msgs.iter_mut() {
            if let Some(r) = &mut m.reply_to
                && let Some(q) = by_id.get(&r.id)
            {
                r.from = q.from.as_ref().map(|f| f.name.clone());
                if r.text.is_none() {
                    r.text = Some(q.summary());
                }
            }
        }
    }

    async fn history(&self, chat: ChatId, topic: Option<MsgId>, before: Option<MsgId>, around: bool) -> Result<(Vec<Message>, bool), String> {
        let peer = self.peer(chat).await?;
        let input = Self::input_peer(&peer);
        const PAGE: i32 = 40;
        let (offset_id, add_offset) = match (before, around) {
            (Some(id), true) => (id, -PAGE / 2),
            (Some(id), false) => (id, 0),
            (None, _) => (0, 0),
        };
        let r = match topic {
            Some(t) => self
                .client
                .invoke(&tl::functions::messages::GetReplies { peer: input, msg_id: t, offset_id, offset_date: 0, add_offset, limit: PAGE, max_id: 0, min_id: 0, hash: 0 })
                .await,
            None => self.client.invoke(&tl::functions::messages::GetHistory { peer: input, offset_id, offset_date: 0, add_offset, limit: PAGE, max_id: 0, min_id: 0, hash: 0 }).await,
        }
        .map_err(|e| explain(&e))?;
        let (mut msgs, _) = self.messages_of(r).await;
        // A topic's own first message counts as in it.
        if let Some(t) = topic {
            for m in &mut msgs {
                if m.id == t {
                    m.topic = Some(t);
                }
            }
        }
        self.fetch_replies(chat, &mut msgs).await;
        let more = msgs.len() as i32 >= PAGE / 2 && msgs.first().is_some_and(|m| m.id > 1);
        Ok((msgs, more))
    }

    async fn topics(&self, chat: ChatId) -> Result<Vec<Topic>, String> {
        let peer = self.peer(chat).await?;
        let r = self
            .client
            .invoke(&tl::functions::messages::GetForumTopics { peer: Self::input_peer(&peer), q: None, offset_date: 0, offset_id: 0, offset_topic: 0, limit: 100 })
            .await
            .map_err(|e| explain(&e))?;
        let tl::enums::messages::ForumTopics::Topics(t) = r;
        self.remember_users(&t.users).await;
        self.remember_chats(&t.chats).await;
        let last: HashMap<MsgId, Message> = t.messages.iter().filter_map(|m| self.convert(m)).map(|m| (m.id, m)).collect();
        Ok(t.topics
            .iter()
            .filter_map(|t| match t {
                tl::enums::ForumTopic::Topic(t) => Some(Topic {
                    id: t.id,
                    title: t.title.clone(),
                    emoji: None,
                    color: t.icon_color as u32,
                    unread: t.unread_count.max(0) as u32,
                    date: last.get(&t.top_message).map_or(t.date as i64, |m| m.date),
                    closed: t.closed,
                    pinned: t.pinned,
                    last: last.get(&t.top_message).map(Preview::of),
                }),
                _ => None,
            })
            .collect())
    }

    fn cached_chats() -> Option<Vec<Chat>> {
        serde_json::from_slice(&std::fs::read(account::dir().join("chats.json")).ok()?).ok()
    }

    fn cache_chats(chats: Vec<Chat>) {
        if let Ok(d) = account::private_dir() {
            let _ = account::private_write(&d.join("chats.json"), &serde_json::to_vec(&chats).unwrap_or_default());
        }
    }

    fn cached_messages(chat: ChatId) -> Option<Vec<Message>> {
        serde_json::from_slice(&std::fs::read(account::dir().join("chats").join(format!("{chat}.json"))).ok()?).ok()
    }

    fn cache_messages(chat: ChatId, msgs: &[Message]) {
        if let Ok(d) = account::private_dir() {
            let d = d.join("chats");
            let _ = std::fs::create_dir_all(&d);
            let keep = &msgs[msgs.len().saturating_sub(40)..];
            let _ = account::private_write(&d.join(format!("{chat}.json")), &serde_json::to_vec(keep).unwrap_or_default());
        }
    }

    fn known_chat(&self, id: ChatId) -> Option<Chat> {
        self.chats.lock().unwrap().get(&id).cloned()
    }

    fn put_chat(&self, mut c: Chat) {
        // What the person changed and Telegram doesn't have yet shows.
        self.outbox.lock().unwrap().apply_to(&mut c);
        self.chats.lock().unwrap().insert(c.id, c.clone());
        self.cache_due.notify_one();
        self.emit(Event::Chat(c));
    }

    /// A new or edited message's effect on its chat's row.
    fn touch_chat(&self, m: &Message, new: bool) {
        let mut c = match self.known_chat(m.chat) {
            Some(c) => c,
            None => {
                let w = self.who.lock().unwrap().get(&m.chat).cloned().unwrap_or_default();
                let unknown = crate::model::stranger(m.chat, w.kind, w.contact, w.verified, m.out);
                Chat { id: m.chat, kind: w.kind, title: w.name, username: w.username, photo: w.photo, contact: w.contact, verified: w.verified, unknown, members: w.members, seen: w.seen, ..Chat::default() }
            }
        };
        // Writing back makes them known.
        if new && m.out {
            c.unknown = false;
        }
        let unread = new && counts_unread(&c, m);
        if c.last.as_ref().is_none_or(|l| l.id <= m.id) {
            c.last = Some(Preview::of(m));
            c.order = c.order.max(m.date);
        }
        if unread {
            c.unread += 1;
            if m.mentioned {
                c.mentions += 1;
            }
        }
        if new && m.out {
            c.draft = None;
            c.read_inbox = c.read_inbox.max(m.id);
        }
        self.put_chat(c);
    }

    async fn update(&self, update: tl::enums::Update) {
        use tl::enums::Update as U;
        match update {
            U::NewMessage(tl::types::UpdateNewMessage { message, .. }) | U::NewChannelMessage(tl::types::UpdateNewChannelMessage { message, .. }) => {
                if let Some(mut m) = self.convert(&message) {
                    self.fetch_replies(m.chat, std::slice::from_mut(&mut m)).await;
                    self.touch_chat(&m, true);
                    self.emit(Event::New(m));
                }
            }
            U::EditMessage(tl::types::UpdateEditMessage { message, .. }) | U::EditChannelMessage(tl::types::UpdateEditChannelMessage { message, .. }) => {
                if let Some(mut m) = self.convert(&message) {
                    self.fetch_replies(m.chat, std::slice::from_mut(&mut m)).await;
                    self.touch_chat(&m, false);
                    self.emit(Event::Edited(m));
                }
            }
            U::DeleteMessages(d) => self.emit(Event::Deleted { chat: None, ids: d.messages }),
            U::DeleteChannelMessages(d) => self.emit(Event::Deleted { chat: Some(-1_000_000_000_000 - d.channel_id), ids: d.messages }),
            U::ReadHistoryInbox(r) => self.read(id_of(&r.peer), false, r.max_id, Some(r.still_unread_count)),
            U::ReadChannelInbox(r) => self.read(-1_000_000_000_000 - r.channel_id, false, r.max_id, Some(r.still_unread_count)),
            U::ReadHistoryOutbox(r) => self.read(id_of(&r.peer), true, r.max_id, None),
            U::ReadChannelOutbox(r) => self.read(-1_000_000_000_000 - r.channel_id, true, r.max_id, None),
            U::UserTyping(t) => {
                let who = self.name_of(t.user_id).unwrap_or_default();
                self.emit(Event::Doing { chat: t.user_id, topic: t.top_msg_id, who, doing: doing_of(&t.action) });
            }
            U::ChatUserTyping(t) => {
                let who = self.name_of(id_of(&t.from_id)).unwrap_or_default();
                self.emit(Event::Doing { chat: -t.chat_id, topic: None, who, doing: doing_of(&t.action) });
            }
            U::ChannelUserTyping(t) => {
                let who = self.name_of(id_of(&t.from_id)).unwrap_or_default();
                self.emit(Event::Doing { chat: -1_000_000_000_000 - t.channel_id, topic: t.top_msg_id, who, doing: doing_of(&t.action) });
            }
            U::MessageReactions(r) => {
                // Fetch the message again, to show it with its reactions.
                let chat = id_of(&r.peer);
                if let Ok((msgs, _)) = self.by_ids(chat, &[r.msg_id]).await {
                    for m in msgs {
                        self.emit(Event::Edited(m));
                    }
                }
            }
            U::DraftMessage(d) => {
                let chat = id_of(&d.peer);
                if d.top_msg_id.is_none()
                    && let Some(mut c) = self.known_chat(chat)
                {
                    c.draft = match d.draft {
                        tl::enums::DraftMessage::Message(m) if !m.message.is_empty() => Some(m.message),
                        _ => None,
                    };
                    self.put_chat(c);
                }
            }
            U::DialogPinned(p) => {
                if let tl::enums::DialogPeer::Peer(dp) = p.peer {
                    let id = id_of(&dp.peer);
                    if let Some(mut c) = self.known_chat(id) {
                        c.pinned = p.pinned;
                        self.put_chat(c);
                    }
                    self.confirm(id);
                }
            }
            U::DialogUnreadMark(u) => {
                if let tl::enums::DialogPeer::Peer(dp) = u.peer {
                    let id = id_of(&dp.peer);
                    if let Some(mut c) = self.known_chat(id) {
                        c.marked = u.unread;
                        self.put_chat(c);
                    }
                    self.confirm(id);
                }
            }
            U::FolderPeers(f) => {
                for p in f.folder_peers {
                    let tl::enums::FolderPeer::Peer(p) = p;
                    let id = id_of(&p.peer);
                    if let Some(mut c) = self.known_chat(id) {
                        c.archived = p.folder_id == 1;
                        self.put_chat(c);
                    }
                    self.confirm(id);
                }
            }
            U::NotifySettings(n) => {
                if let tl::enums::NotifyPeer::Peer(p) = n.peer {
                    let id = id_of(&p.peer);
                    if let Some(mut c) = self.known_chat(id) {
                        let tl::enums::PeerNotifySettings::Settings(s) = n.notify_settings;
                        c.muted = s.mute_until.is_some_and(|t| t as i64 > now());
                        self.put_chat(c);
                    }
                    self.confirm(id);
                }
            }
            U::UserStatus(s) => {
                let seen = seen_of(&s.status);
                if let Some(w) = self.who.lock().unwrap().get_mut(&s.user_id) {
                    w.seen = seen.clone();
                }
                if let Some(mut c) = self.known_chat(s.user_id) {
                    c.seen = seen;
                    self.put_chat(c);
                }
            }
            U::DialogFilter(_) | U::DialogFilters | U::DialogFilterOrder(_) => {
                if let Ok(f) = self.folders().await {
                    self.emit(Event::Folders(f));
                }
            }
            U::PinnedDialogs(_) => {
                let _ = self.load_chats(false).await;
            }
            _ => {}
        }
    }

    fn read(&self, chat: ChatId, outbox: bool, up_to: MsgId, still_unread: Option<i32>) {
        if !outbox {
            // (How many are left unread is as of the update; read again.)
            self.confirm(chat);
        }
        if let Some(mut c) = self.known_chat(chat) {
            if outbox {
                c.read_outbox = c.read_outbox.max(up_to);
            } else if !read_moves_on(&c, up_to) {
                return;
            } else {
                c.read_inbox = c.read_inbox.max(up_to);
                c.unread = still_unread.map_or(0, |n| n.max(0) as u32);
                if c.unread == 0 {
                    c.mentions = 0;
                }
            }
            self.put_chat(c);
        }
        self.emit(Event::Read { chat, outbox, up_to });
    }

    async fn by_ids(&self, chat: ChatId, ids: &[MsgId]) -> Result<(Vec<Message>, bool), String> {
        let peer = self.peer(chat).await?;
        let input: Vec<tl::enums::InputMessage> = ids.iter().map(|&id| tl::enums::InputMessage::Id(tl::types::InputMessageId { id })).collect();
        let r = if peer.id.kind() == PeerKind::Channel {
            self.client.invoke(&tl::functions::channels::GetMessages { channel: (&peer).into(), id: input }).await
        } else {
            self.client.invoke(&tl::functions::messages::GetMessages { id: input }).await
        }
        .map_err(|e| explain(&e))?;
        let (mut msgs, _) = self.messages_of(r).await;
        self.fetch_replies(chat, &mut msgs).await;
        Ok((msgs, false))
    }

    /// The whole chat list (the main list, then the archive), in pages.
    async fn load_chats(&self, cached_first: bool) -> Result<(), String> {
        let _one = self.loading.lock().await;
        if cached_first && let Some(mut c) = blocking(Self::cached_chats).await {
            {
                let o = self.outbox.lock().unwrap();
                for chat in &mut c {
                    o.apply_to(chat);
                }
            }
            {
                let mut known = self.chats.lock().unwrap();
                for chat in &c {
                    known.entry(chat.id).or_insert_with(|| chat.clone());
                }
            }
            self.cache_ready.store(true, Ordering::Relaxed);
            self.emit(Event::Chats { chats: c, done: false, cached: true });
        }
        if let Ok(f) = self.folders().await {
            self.emit(Event::Folders(f));
        }
        let mut live = HashSet::new();
        let (mut main, mut archived, mut pages) = (0, 0, 0);
        // The main list (folder 0), then the archive (1): each chat's
        // place as Telegram has it. (No folder asks for every chat at
        // once, archived ones in among the rest.)
        for folder in [Some(0), Some(1)] {
            let mut offset = (0, 0, tl::enums::InputPeer::Empty);
            loop {
                let (mut page, next) = self.dialogs_retrying(folder, offset).await.map_err(|e| {
                    tracing::warn!("telegram: the chat list stopped after {pages} pages: {e}");
                    explain(&e)
                })?;
                pages += 1;
                {
                    let o = self.outbox.lock().unwrap();
                    for c in &mut page {
                        o.apply_to(c);
                    }
                }
                {
                    let mut known = self.chats.lock().unwrap();
                    for c in &page {
                        live.insert(c.id);
                        *(if c.archived { &mut archived } else { &mut main }) += 1;
                        known.insert(c.id, c.clone());
                    }
                }
                self.cache_due.notify_one();
                let done = next.is_none() && folder == Some(1);
                self.emit(Event::Chats { chats: page, done, cached: false });
                match next {
                    Some(n) => offset = n,
                    None => break,
                }
            }
        }
        // What Telegram no longer gives (left, deleted, an id it no longer
        // goes by) goes, as from the list (`Store::whole`).
        self.chats.lock().unwrap().retain(|id, _| live.contains(id));
        self.cache_ready.store(true, Ordering::Relaxed);
        self.list_whole.store(true, Ordering::Relaxed);
        // Changes waiting for a chat not known till now go.
        self.outbox_due.notify_one();
        self.cache_due.notify_one();
        self.cache_soon.notify_one();
        tracing::info!("telegram: the chat list: {main} in the main list, {archived} archived, in {pages} pages");
        self.emit(Event::Status(Status::Online));
        Ok(())
    }

    /// A page of the chat list, asked again when it fails rather than
    /// leaving the rest of the list as the cache had it: after the wait
    /// Telegram asks for (FLOOD_WAIT, however long: shown as Waiting), else
    /// 2, 4, 8… seconds; six tries at most.
    async fn dialogs_retrying(&self, folder: Option<i32>, offset: (i32, i32, tl::enums::InputPeer)) -> Result<(Vec<Chat>, Option<(i32, i32, tl::enums::InputPeer)>), InvocationError> {
        let mut tries = 0u32;
        loop {
            match self.dialogs(folder, offset.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) if tries < 5 => {
                    tries += 1;
                    let flood = match &e {
                        InvocationError::Rpc(r) if r.code == 420 => r.value,
                        _ => None,
                    };
                    let wait = flood.map_or(1u64 << tries, u64::from);
                    tracing::warn!("telegram: a page of the chat list (folder {folder:?}) failed ({e}): again in {wait} s");
                    if let Some(s) = flood {
                        self.emit(Event::Status(Status::Waiting(s)));
                    }
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    if flood.is_some() {
                        self.emit(Event::Status(Status::Updating));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The folder Telegram has a chat in now (0 the main list, 1 the
    /// archive), asked.
    /// A change of the person's: on disk, shown, and on its way.
    fn queue(&self, change: Change) {
        let chat = change.chat();
        {
            let mut o = self.outbox.lock().unwrap();
            if o.is_empty() {
                self.outbox_total.store(0, Ordering::Relaxed);
            }
            o.push(change);
            o.save();
        }
        self.outbox_total.fetch_add(1, Ordering::Relaxed);
        if let Some(c) = self.known_chat(chat) {
            self.put_chat(c);
        }
        self.emit_sync(None);
        self.outbox_due.notify_one();
    }

    /// How many changes wait to be sent (and, if Telegram asked, for how
    /// long).
    fn emit_sync(&self, waiting: Option<u32>) {
        let pending = self.outbox.lock().unwrap().len();
        let total = (self.outbox_total.load(Ordering::Relaxed).max(0) as usize).max(pending);
        self.emit(Event::Sync { pending, total, waiting });
    }

    /// Send what's waiting until none is: archive moves together, a
    /// hundred to a request, the rest one by one. A change Telegram
    /// refuses is dropped (said, and the chat read again), as is one for a
    /// chat gone from the list; one it can't take now (asked to wait,
    /// offline, a chat not known yet) is tried again after a wait, longer
    /// each time up to a minute, for as long as it waits (and at once when
    /// the connection comes back or another change is made).
    async fn send_outbox(&self) {
        let mut backoff = 0u32;
        loop {
            let waiting: Vec<Change> = self.outbox.lock().unwrap().changes().to_vec();
            if waiting.is_empty() {
                self.emit_sync(None);
                return;
            }
            let (moves, rest): (Vec<Change>, Vec<Change>) = waiting.into_iter().partition(|c| matches!(c, Change::Archive { .. }));
            let mut batches: Vec<Vec<Change>> = moves.chunks(100).map(<[Change]>::to_vec).collect();
            batches.extend(rest.into_iter().map(|c| vec![c]));
            let mut later: Option<(Duration, Option<u32>)> = None;
            let mut progress = false;
            for batch in batches {
                match self.send_changes(&batch).await {
                    Ok(left) => {
                        let done: Vec<Change> = batch.iter().filter(|c| !left.contains(c)).cloned().collect();
                        progress |= !done.is_empty();
                        self.sent(&done);
                        if !left.is_empty() {
                            later = later.or(Some((Duration::from_secs(5), None)));
                        }
                    }
                    Err(Failed::Refused(why)) => {
                        tracing::warn!("telegram: {batch:?} refused: {why}");
                        self.emit(Event::Error(format!("Telegram didn’t take a change ({why}).")));
                        self.sent(&batch);
                    }
                    Err(Failed::Later(wait, flood)) => {
                        later = Some((wait, flood));
                        // (Asked to wait, or offline: the rest would fare
                        // the same.)
                        break;
                    }
                }
            }
            let Some((wait, flood)) = later else { continue };
            backoff = if progress { 0 } else { backoff + 1 };
            let wait = wait.max(Duration::from_secs((5u64 << backoff.min(4)).min(60)));
            tracing::info!("telegram: {} changes to send, again in {} s", self.outbox.lock().unwrap().len(), wait.as_secs());
            self.emit_sync(flood);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.outbox_due.notified() => {}
            }
        }
    }

    /// Those changes are in Telegram: off the disk, and the chats read
    /// again from it.
    fn sent(&self, done: &[Change]) {
        {
            let mut o = self.outbox.lock().unwrap();
            o.sent(done);
            o.save();
        }
        for c in done {
            self.confirm(c.chat());
        }
        self.emit_sync(None);
    }

    /// Send `batch`; the changes left for later (their chats not known
    /// yet). A change for a chat gone from the list is done with.
    async fn send_changes(&self, batch: &[Change]) -> Result<Vec<Change>, Failed> {
        let mut left = Vec::new();
        let mut resolved = Vec::new();
        for c in batch {
            match self.peer(c.chat()).await {
                Ok(p) => resolved.push((c.clone(), p)),
                Err(_) if self.list_whole.load(Ordering::Relaxed) && self.known_chat(c.chat()).is_none() => {
                    tracing::info!("telegram: {c:?} dropped: the chat is gone");
                }
                Err(e) => {
                    tracing::info!("telegram: {c:?} waits: {e}");
                    left.push(c.clone());
                }
            }
        }
        match resolved.as_slice() {
            [] => {}
            [(Change::Archive { .. }, _), ..] => {
                let peers = resolved
                    .iter()
                    .filter_map(|(c, p)| match *c {
                        Change::Archive { on, .. } => Some(tl::enums::InputFolderPeer::Peer(tl::types::InputFolderPeer { peer: Self::input_peer(p), folder_id: i32::from(on) })),
                        _ => None,
                    })
                    .collect();
                let r = self.client.invoke(&tl::functions::folders::EditPeerFolders { folder_peers: peers }).await.map_err(|e| Failed::of(&e))?;
                for (c, _) in &resolved {
                    if let Change::Archive { chat, on } = *c {
                        tracing::info!("telegram: chat {chat} into folder {}: Telegram says {:?}", i32::from(on), folder_in(&r, chat));
                    }
                }
            }
            [(Change::Pin { on, .. }, p)] => {
                let dp = tl::enums::InputDialogPeer::Peer(tl::types::InputDialogPeer { peer: Self::input_peer(p) });
                self.client.invoke(&tl::functions::messages::ToggleDialogPin { pinned: *on, peer: dp }).await.map_err(|e| Failed::of(&e))?;
            }
            [(Change::Mute { on, .. }, p)] => {
                let settings = tl::enums::InputPeerNotifySettings::Settings(tl::types::InputPeerNotifySettings {
                    show_previews: None,
                    silent: None,
                    mute_until: Some(if *on { i32::MAX } else { 0 }),
                    sound: None,
                    stories_muted: None,
                    stories_hide_sender: None,
                    stories_sound: None,
                });
                let np = tl::enums::InputNotifyPeer::Peer(tl::types::InputNotifyPeer { peer: Self::input_peer(p) });
                self.client.invoke(&tl::functions::account::UpdateNotifySettings { peer: np, settings }).await.map_err(|e| Failed::of(&e))?;
            }
            [(Change::Read { up_to, .. }, p)] => {
                if p.id.kind() == PeerKind::Channel {
                    self.client.invoke(&tl::functions::channels::ReadHistory { channel: p.into(), max_id: *up_to }).await.map_err(|e| Failed::of(&e))?;
                } else {
                    self.client.invoke(&tl::functions::messages::ReadHistory { peer: Self::input_peer(p), max_id: *up_to }).await.map_err(|e| Failed::of(&e))?;
                }
            }
            [(Change::MarkUnread { on, .. }, p)] => {
                let dp = tl::enums::InputDialogPeer::Peer(tl::types::InputDialogPeer { peer: Self::input_peer(p) });
                self.client.invoke(&tl::functions::messages::MarkDialogUnread { unread: *on, parent_peer: None, peer: dp }).await.map_err(|e| Failed::of(&e))?;
            }
            _ => {}
        }
        Ok(left)
    }

    /// Read chat `id`'s state from Telegram again in a moment.
    fn confirm(&self, id: ChatId) {
        self.confirming.lock().unwrap().insert(id);
        self.confirm_due.notify_one();
    }

    /// The state of chats `ids` (100 at most) as Telegram has it now:
    /// archived, unread, marked, muted, pinned, read up to where.
    async fn confirm_now(&self, ids: &[ChatId]) {
        let mut peers = Vec::new();
        for id in ids {
            if let Ok(p) = self.peer(*id).await {
                peers.push(tl::enums::InputDialogPeer::Peer(tl::types::InputDialogPeer { peer: Self::input_peer(&p) }));
            }
        }
        if peers.is_empty() {
            return;
        }
        let d = match self.client.invoke(&tl::functions::messages::GetPeerDialogs { peers }).await {
            Ok(tl::enums::messages::PeerDialogs::Dialogs(d)) => d,
            Err(e) => {
                tracing::warn!("telegram: {} chats' state couldn't be read again: {e}", ids.len());
                return;
            }
        };
        for d in &d.dialogs {
            let tl::enums::Dialog::Dialog(d) = d else { continue };
            let Some(mut c) = self.known_chat(id_of(&d.peer)) else { continue };
            let now = self.chat_of(d, None, false);
            let was = c.clone();
            c.archived = now.archived;
            c.unread = now.unread;
            c.mentions = now.mentions;
            c.marked = now.marked;
            c.muted = now.muted;
            c.pinned = now.pinned;
            c.read_inbox = now.read_inbox;
            c.read_outbox = c.read_outbox.max(now.read_outbox);
            if c != was {
                self.put_chat(c);
            }
        }
    }

    async fn send(&self, out: Outgoing) -> Result<(), String> {
        let peer = self.peer(out.chat).await?;
        let entities: Vec<tl::enums::MessageEntity> = out.entities.iter().filter_map(|e| entity_to_raw(&out.text, e)).collect();
        let reply = out.reply_to.or(out.topic);
        let mut sent = Vec::new();
        if let Some((path, duration, wave)) = &out.voice {
            let up = self.client.upload_file(path).await.map_err(|e| format!("The voice message couldn’t be sent ({e})."))?;
            let msg = InputMessage::new()
                .text(out.text.clone())
                .fmt_entities(entities.clone())
                .reply_to(reply)
                .document(up)
                .mime_type("audio/ogg")
                .attribute(Attribute::Voice { duration: Duration::from_secs_f32(*duration), waveform: Some(pack_waveform(wave)) });
            sent.push(self.client.send_message(peer, msg).await.map_err(|e| explain(&e))?);
        } else if out.files.is_empty() {
            let msg = InputMessage::new().text(out.text.clone()).fmt_entities(entities).reply_to(reply).clear_draft(true);
            sent.push(self.client.send_message(peer, msg).await.map_err(|e| explain(&e))?);
        } else {
            let n = out.files.len();
            for (i, f) in out.files.iter().enumerate() {
                (self.sink)(Event::Progress { pic: None, local: Some(out.local), done: i as f32 / n as f32 });
                let up = self.client.upload_file(f).await.map_err(|e| format!("{} couldn’t be sent ({e}).", f.display()))?;
                let photo = !out.as_files && is_photo(f);
                // The caption goes with the last file.
                let text = if i + 1 == n { out.text.clone() } else { String::new() };
                let ents = if i + 1 == n { entities.clone() } else { Vec::new() };
                let mut msg = InputMessage::new().text(text).fmt_entities(ents).reply_to(reply);
                msg = if photo { msg.photo(up) } else { msg.document(up) };
                sent.push(self.client.send_message(peer, msg).await.map_err(|e| explain(&e))?);
            }
        }
        for (i, s) in sent.iter().enumerate() {
            if let Some(mut m) = self.convert(&s.raw) {
                if m.chat == 0 {
                    m.chat = out.chat;
                }
                m.chat = out.chat;
                m.out = true;
                m.topic = m.topic.or(out.topic);
                self.fetch_replies(out.chat, std::slice::from_mut(&mut m)).await;
                self.touch_chat(&m, true);
                if i + 1 == sent.len() {
                    self.emit(Event::Sent { local: out.local, msg: m });
                } else {
                    self.emit(Event::New(m));
                }
            }
        }
        Ok(())
    }

    /// Where a picture or file goes in the cache, and how to download it.
    fn locate(&self, pic: &Pic) -> Option<(PathBuf, Loc)> {
        let dir = media_dir();
        match pic {
            Pic::Avatar(chat) => {
                let photo = if *chat == self.me() { self.store.me().and_then(|m| m.photo) } else { self.who.lock().unwrap().get(chat).and_then(|w| w.photo) }?;
                let id = PeerId::from_bot_api_dialog_id(*chat)?;
                let info = self.store.peer_info(id)?;
                let auth = info.auth();
                let peer = PeerRef { id, auth: auth.unwrap_or_default() };
                let loc = tl::enums::InputFileLocation::InputPeerPhotoFileLocation(tl::types::InputPeerPhotoFileLocation { big: false, peer: (&peer).into(), photo_id: photo });
                Some((dir.join(format!("avatar-{chat}-{photo}.jpg")), Loc { loc, size: None }))
            }
            Pic::Thumb(chat, id) | Pic::File(chat, id) => {
                let media = self.media.lock().unwrap().get(&(*chat, *id)).cloned()?;
                let whole = matches!(pic, Pic::File(..));
                match media {
                    tl::enums::MessageMedia::Photo(p) => {
                        let tl::enums::Photo::Photo(photo) = p.photo? else { return None };
                        let size = best_size(&photo.sizes, if whole { 4096 } else { 800 })?;
                        let loc = tl::enums::InputFileLocation::InputPhotoFileLocation(tl::types::InputPhotoFileLocation {
                            id: photo.id,
                            access_hash: photo.access_hash,
                            file_reference: photo.file_reference.clone(),
                            thumb_size: size.0,
                        });
                        Some((dir.join(format!("{chat}-{id}-{}.jpg", if whole { "full" } else { "thumb" })), Loc { loc, size: Some(size.1) }))
                    }
                    tl::enums::MessageMedia::Document(d) => {
                        let tl::enums::Document::Document(doc) = d.document? else { return None };
                        let sticker = doc.attributes.iter().any(|a| matches!(a, tl::enums::DocumentAttribute::Sticker(_)));
                        let name = doc.attributes.iter().find_map(|a| if let tl::enums::DocumentAttribute::Filename(f) = a { Some(f.file_name.clone()) } else { None });
                        // A static sticker is its own picture; anything else, its thumbnail.
                        let static_sticker = sticker && doc.mime_type == "image/webp";
                        if whole || static_sticker {
                            let loc = tl::enums::InputFileLocation::InputDocumentFileLocation(tl::types::InputDocumentFileLocation {
                                id: doc.id,
                                access_hash: doc.access_hash,
                                file_reference: doc.file_reference.clone(),
                                thumb_size: String::new(),
                            });
                            let ext = name.as_deref().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_owned()).or_else(|| ext_for(&doc.mime_type).map(str::to_owned)).unwrap_or_else(|| "bin".into());
                            let file = match name {
                                Some(n) => format!("{chat}-{id}-{}", safe_name(&n)),
                                None => format!("{chat}-{id}.{ext}"),
                            };
                            Some((dir.join(file), Loc { loc, size: Some(doc.size as usize) }))
                        } else {
                            let size = best_size(doc.thumbs.as_deref().unwrap_or(&[]), 800)?;
                            let loc = tl::enums::InputFileLocation::InputDocumentFileLocation(tl::types::InputDocumentFileLocation {
                                id: doc.id,
                                access_hash: doc.access_hash,
                                file_reference: doc.file_reference.clone(),
                                thumb_size: size.0,
                            });
                            Some((dir.join(format!("{chat}-{id}-thumb.jpg")), Loc { loc, size: Some(size.1) }))
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    async fn fetch(&self, pic: Pic) -> Result<(), String> {
        if !self.fetching.lock().unwrap().insert(pic.clone()) {
            return Ok(());
        }
        let r = self.fetch_now(&pic).await;
        self.fetching.lock().unwrap().remove(&pic);
        match r {
            Ok(path) => self.emit(Event::Fetched { pic, path }),
            Err(why) => self.emit(Event::FetchFailed { pic, why }),
        }
        Ok(())
    }

    async fn fetch_now(&self, pic: &Pic) -> Result<PathBuf, String> {
        let (path, loc) = self.locate(pic).ok_or("There’s nothing to download.")?;
        if path.exists() {
            return Ok(path);
        }
        let _ = account::private_dir();
        tokio::fs::create_dir_all(media_dir()).await.map_err(|e| e.to_string())?;
        let part = path.with_extension("part");
        let total = loc.size;
        let mut it = self.client.iter_download(&loc);
        let mut file = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
        let mut got = 0usize;
        let mut last = std::time::Instant::now();
        loop {
            match it.next().await {
                Ok(Some(chunk)) => {
                    use tokio::io::AsyncWriteExt;
                    file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                    got += chunk.len();
                    if let Some(t) = total
                        && last.elapsed() > Duration::from_millis(100)
                    {
                        last = std::time::Instant::now();
                        self.emit(Event::Progress { pic: Some(pic.clone()), local: None, done: got as f32 / t.max(1) as f32 });
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(explain(&e));
                }
            }
        }
        drop(file);
        tokio::fs::rename(&part, &path).await.map_err(|e| e.to_string())?;
        Ok(path)
    }

    async fn handle(&self, cmd: Cmd) -> Result<(), String> {
        match cmd {
            Cmd::Chats => self.load_chats(true).await?,
            Cmd::Open { chat, topic } => {
                if topic.is_none()
                    && let Some(msgs) = blocking(move || Self::cached_messages(chat)).await
                {
                    let more = msgs.first().is_some_and(|m| m.id > 1);
                    self.emit(Event::Messages { chat, topic, msgs, more, before: None });
                }
                let (msgs, more) = self.history(chat, topic, None, false).await?;
                if topic.is_none() {
                    let keep = msgs.clone();
                    blocking(move || Self::cache_messages(chat, &keep)).await;
                }
                self.emit(Event::Messages { chat, topic, msgs, more, before: None });
                if topic.is_none() && self.known_chat(chat).is_some_and(|c| c.kind == ChatKind::Forum) {
                    let topics = self.topics(chat).await?;
                    self.emit(Event::Topics { chat, topics });
                }
            }
            Cmd::Older { chat, topic, before } => {
                let (msgs, more) = self.history(chat, topic, Some(before), false).await?;
                self.emit(Event::Messages { chat, topic, msgs, more, before: Some(before) });
            }
            Cmd::Around { chat, id } => {
                let (msgs, more) = self.history(chat, None, Some(id + 1), true).await?;
                self.emit(Event::Around { chat, id, msgs, more });
            }
            Cmd::Send(out) => {
                let (local, chat) = (out.local, out.chat);
                if let Err(why) = self.send(out).await {
                    self.emit(Event::SendFailed { local, chat, why });
                }
            }
            Cmd::Edit { chat, id, text, entities } => {
                let peer = self.peer(chat).await?;
                let ents: Vec<tl::enums::MessageEntity> = entities.iter().filter_map(|e| entity_to_raw(&text, e)).collect();
                self.client.edit_message(peer, id, InputMessage::new().text(text).fmt_entities(ents)).await.map_err(|e| explain(&e))?;
                let (msgs, _) = self.by_ids(chat, &[id]).await?;
                for m in msgs {
                    self.touch_chat(&m, false);
                    self.emit(Event::Edited(m));
                }
            }
            Cmd::Delete { chat, ids, everyone } => {
                let peer = self.peer(chat).await?;
                if peer.id.kind() == PeerKind::Channel || everyone {
                    self.client.delete_messages(peer, &ids).await.map_err(|e| explain(&e))?;
                } else {
                    self.client.invoke(&tl::functions::messages::DeleteMessages { revoke: false, id: ids.clone() }).await.map_err(|e| explain(&e))?;
                }
                self.emit(Event::Deleted { chat: Some(chat), ids });
            }
            Cmd::Forward { from, ids, to } => {
                let (src, dst) = (self.peer(from).await?, self.peer(to).await?);
                let got = self.client.forward_messages(dst, &ids, src).await.map_err(|e| explain(&e))?;
                for m in got.into_iter().flatten() {
                    if let Some(m) = self.convert(&m.raw) {
                        self.touch_chat(&m, true);
                        self.emit(Event::New(m));
                    }
                }
            }
            Cmd::React { chat, id, emoji } => {
                let peer = self.peer(chat).await?;
                let r = match emoji {
                    Some(e) => grammers_client::message::InputReactions::emoticon(e),
                    None => grammers_client::message::InputReactions::remove(),
                };
                self.client.send_reactions(peer, id, r).await.map_err(|e| explain(&e))?;
                let (msgs, _) = self.by_ids(chat, &[id]).await?;
                for m in msgs {
                    self.emit(Event::Edited(m));
                }
            }
            // The person's changes to their chats: kept until Telegram has
            // them (`outbox`), shown at once.
            Cmd::Read { chat, up_to } => {
                if self.known_chat(chat).is_some_and(|c| c.marked) {
                    self.queue(Change::MarkUnread { chat, on: false });
                }
                self.queue(Change::Read { chat, up_to });
            }
            Cmd::MarkUnread { chat, on } => self.queue(Change::MarkUnread { chat, on }),
            Cmd::Typing { chat, topic, doing } => {
                let peer = self.peer(chat).await?;
                let action = match doing {
                    Doing::Typing => tl::enums::SendMessageAction::SendMessageTypingAction,
                    Doing::RecordingVoice => tl::enums::SendMessageAction::SendMessageRecordAudioAction,
                    Doing::SendingFile | Doing::SendingPhoto => tl::enums::SendMessageAction::SendMessageUploadDocumentAction(tl::types::SendMessageUploadDocumentAction { progress: 0 }),
                    _ => tl::enums::SendMessageAction::SendMessageCancelAction,
                };
                self.client.invoke(&tl::functions::messages::SetTyping { peer: Self::input_peer(&peer), top_msg_id: topic, action }).await.map_err(|e| explain(&e))?;
            }
            Cmd::Draft { chat, text } => {
                let peer = self.peer(chat).await?;
                self.client
                    .invoke(&tl::functions::messages::SaveDraft {
                        no_webpage: false,
                        invert_media: false,
                        reply_to: None,
                        peer: Self::input_peer(&peer),
                        message: text.clone(),
                        entities: None,
                        media: None,
                        effect: None,
                        suggested_post: None,
                        rich_message: None,
                    })
                    .await
                    .map_err(|e| explain(&e))?;
                if let Some(mut c) = self.known_chat(chat) {
                    c.draft = Some(text).filter(|t| !t.trim().is_empty());
                    self.put_chat(c);
                }
            }
            Cmd::PinChat { chat, on } => self.queue(Change::Pin { chat, on }),
            Cmd::Archive { chat, on } => self.queue(Change::Archive { chat, on }),
            Cmd::DeleteChat { chat } => {
                let peer = self.peer(chat).await?;
                self.client.delete_dialog(peer).await.map_err(|e| explain(&e))?;
                self.chats.lock().unwrap().remove(&chat);
                self.cache_due.notify_one();
                self.emit(Event::ChatGone(chat));
            }
            Cmd::Mute { chat, on } => self.queue(Change::Mute { chat, on }),
            Cmd::PinMessage { chat, id, on } => {
                let peer = self.peer(chat).await?;
                if on {
                    self.client.pin_message(peer, id).await.map_err(|e| explain(&e))?;
                } else {
                    self.client.unpin_message(peer, id).await.map_err(|e| explain(&e))?;
                }
                let (msgs, _) = self.by_ids(chat, &[id]).await?;
                for m in msgs {
                    self.emit(Event::Edited(m));
                }
            }
            Cmd::Fetch(pic) => self.fetch(pic).await?,
            Cmd::SaveFile { chat, id, to } => {
                let path = self.fetch_now(&Pic::File(chat, id)).await?;
                let dest = to.clone();
                blocking(move || std::fs::copy(&path, &dest)).await.map_err(|e| format!("Couldn’t save {} ({e}).", to.display()))?;
                self.emit(Event::Saved { to });
            }
            Cmd::Search { query } => {
                let found = self.client.invoke(&tl::functions::contacts::Search { broadcasts: false, bots: false, q: query.clone(), limit: 20 }).await.map_err(|e| explain(&e))?;
                let tl::enums::contacts::Found::Found(found) = found;
                self.remember_users(&found.users).await;
                self.remember_chats(&found.chats).await;
                let chats: Vec<Chat> = found
                    .my_results
                    .iter()
                    .chain(&found.results)
                    .map(id_of)
                    .map(|id| {
                        self.known_chat(id).unwrap_or_else(|| {
                            let w = self.who.lock().unwrap().get(&id).cloned().unwrap_or_default();
                            Chat { id, kind: w.kind, title: w.name, username: w.username, photo: w.photo, contact: w.contact, verified: w.verified, members: w.members, seen: w.seen, ..Chat::default() }
                        })
                    })
                    .collect();
                let r = self
                    .client
                    .invoke(&tl::functions::messages::SearchGlobal {
                        broadcasts_only: false,
                        groups_only: false,
                        users_only: false,
                        folder_id: None,
                        community: None,
                        q: query.clone(),
                        filter: tl::enums::MessagesFilter::InputMessagesFilterEmpty,
                        min_date: 0,
                        max_date: 0,
                        offset_rate: 0,
                        offset_peer: tl::enums::InputPeer::Empty,
                        offset_id: 0,
                        limit: 40,
                    })
                    .await
                    .map_err(|e| explain(&e))?;
                let (mut msgs, _) = self.messages_of(r).await;
                msgs.sort_by_key(|m| std::cmp::Reverse(m.date));
                self.emit(Event::Results { query, chats, msgs });
            }
            Cmd::Contacts => {
                let r = self.client.invoke(&tl::functions::contacts::GetContacts { hash: 0 }).await.map_err(|e| explain(&e))?;
                if let tl::enums::contacts::Contacts::Contacts(c) = r {
                    self.remember_users(&c.users).await;
                    let mut people: Vec<Contact> = c
                        .users
                        .iter()
                        .filter_map(|u| match u {
                            tl::enums::User::User(u) => {
                                let w = self.who.lock().unwrap().get(&u.id).cloned().unwrap_or_default();
                                Some(Contact { id: u.id, name: w.name, phone: u.phone.as_ref().map(|p| format!("+{p}")), username: u.username.clone(), photo: w.photo, seen: w.seen })
                            }
                            _ => None,
                        })
                        .collect();
                    people.sort_by_key(|p| p.name.to_lowercase());
                    self.emit(Event::Contacts(people));
                }
            }
            Cmd::Resolve { username } => {
                let r = self
                    .client
                    .invoke(&tl::functions::contacts::ResolveUsername { username: username.trim_start_matches('@').to_owned(), referer: None })
                    .await;
                let chat = match r {
                    Ok(tl::enums::contacts::ResolvedPeer::Peer(p)) => {
                        self.remember_users(&p.users).await;
                        self.remember_chats(&p.chats).await;
                        let id = id_of(&p.peer);
                        Some(self.known_chat(id).unwrap_or_else(|| {
                            let w = self.who.lock().unwrap().get(&id).cloned().unwrap_or_default();
                            Chat { id, kind: w.kind, title: w.name, username: w.username, photo: w.photo, contact: w.contact, verified: w.verified, members: w.members, seen: w.seen, ..Chat::default() }
                        }))
                    }
                    Err(_) => None,
                };
                self.emit(Event::Resolved { username, chat });
            }
            Cmd::SignOut => {
                let _ = self.client.sign_out().await;
                blocking(account::forget_session).await?;
                self.emit(Event::Status(Status::SignedOut));
            }
        }
        Ok(())
    }
}

/// Where a file is, for grammers' downloader.
struct Loc {
    loc: tl::enums::InputFileLocation,
    size: Option<usize>,
}

impl grammers_client::media::Downloadable for Loc {
    fn to_raw_input_location(&self) -> Option<tl::enums::InputFileLocation> {
        Some(self.loc.clone())
    }

    fn size(&self) -> Option<usize> {
        self.size
    }
}

/// The largest picture size no wider or taller than `most` (its type
/// and bytes), else the smallest there is.
fn best_size(sizes: &[tl::enums::PhotoSize], most: i32) -> Option<(String, usize)> {
    let mut fits: Option<(i32, String, usize)> = None;
    let mut smallest: Option<(i32, String, usize)> = None;
    for s in sizes {
        let (t, w, h, bytes) = match s {
            tl::enums::PhotoSize::Size(s) => (s.r#type.clone(), s.w, s.h, s.size.max(0) as usize),
            tl::enums::PhotoSize::Progressive(s) => (s.r#type.clone(), s.w, s.h, s.sizes.iter().max().copied().unwrap_or(0).max(0) as usize),
            _ => continue,
        };
        let side = w.max(h);
        if side <= most && fits.as_ref().is_none_or(|f| f.0 < side) {
            fits = Some((side, t.clone(), bytes));
        }
        if smallest.as_ref().is_none_or(|f| f.0 > side) {
            smallest = Some((side, t, bytes));
        }
    }
    fits.or(smallest).map(|f| (f.1, f.2))
}

fn ext_for(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "application/pdf" => "pdf",
        "application/x-tgsticker" => "tgs",
        _ => return None,
    })
}

fn is_photo(path: &std::path::Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp")
}

fn seen_of(s: &tl::enums::UserStatus) -> Option<Seen> {
    Some(match s {
        tl::enums::UserStatus::Online(_) => Seen::Online,
        tl::enums::UserStatus::Offline(o) => Seen::At(o.was_online as i64),
        tl::enums::UserStatus::Recently(_) => Seen::Recently,
        tl::enums::UserStatus::LastWeek(_) => Seen::LastWeek,
        tl::enums::UserStatus::LastMonth(_) => Seen::LastMonth,
        _ => Seen::LongAgo,
    })
}

fn doing_of(a: &tl::enums::SendMessageAction) -> Doing {
    use tl::enums::SendMessageAction as A;
    match a {
        A::SendMessageTypingAction => Doing::Typing,
        A::SendMessageRecordAudioAction => Doing::RecordingVoice,
        A::SendMessageRecordVideoAction | A::SendMessageRecordRoundAction => Doing::RecordingVideo,
        A::SendMessageUploadPhotoAction(_) => Doing::SendingPhoto,
        A::SendMessageUploadDocumentAction(_) | A::SendMessageUploadVideoAction(_) | A::SendMessageUploadAudioAction(_) => Doing::SendingFile,
        A::SendMessageChooseStickerAction => Doing::ChoosingSticker,
        A::SendMessageCancelAction => Doing::Stopped,
        _ => Doing::Typing,
    }
}

fn entities_of(text: &str, raw: &[tl::enums::MessageEntity]) -> Vec<Entity> {
    use tl::enums::MessageEntity as E;
    let mut out: Vec<Entity> = raw
        .iter()
        .filter_map(|e| {
            let (o, l, kind) = match e {
                E::Bold(e) => (e.offset, e.length, EntityKind::Bold),
                E::Italic(e) => (e.offset, e.length, EntityKind::Italic),
                E::Underline(e) => (e.offset, e.length, EntityKind::Underline),
                E::Strike(e) => (e.offset, e.length, EntityKind::Strike),
                E::Spoiler(e) => (e.offset, e.length, EntityKind::Spoiler),
                E::Code(e) => (e.offset, e.length, EntityKind::Code),
                E::Pre(e) => (e.offset, e.length, EntityKind::Pre(e.language.clone())),
                E::Url(e) => (e.offset, e.length, EntityKind::Url),
                E::TextUrl(e) => (e.offset, e.length, EntityKind::TextUrl(e.url.clone())),
                E::Mention(e) => (e.offset, e.length, EntityKind::Mention),
                E::MentionName(e) => (e.offset, e.length, EntityKind::MentionName(e.user_id)),
                E::Hashtag(e) => (e.offset, e.length, EntityKind::Hashtag),
                E::Cashtag(e) => (e.offset, e.length, EntityKind::Cashtag),
                E::BotCommand(e) => (e.offset, e.length, EntityKind::BotCommand),
                E::Email(e) => (e.offset, e.length, EntityKind::Email),
                E::Phone(e) => (e.offset, e.length, EntityKind::Phone),
                E::Blockquote(e) => (e.offset, e.length, EntityKind::Quote),
                E::CustomEmoji(e) => (e.offset, e.length, EntityKind::CustomEmoji(e.document_id)),
                _ => return None,
            };
            let (start, end) = utf16_to_bytes(text, o.max(0) as usize, l.max(0) as usize);
            (end > start).then_some(Entity { start, end, kind })
        })
        .collect();
    out.sort_by_key(|e| e.start);
    out
}

fn entity_to_raw(text: &str, e: &Entity) -> Option<tl::enums::MessageEntity> {
    use tl::enums::MessageEntity as E;
    let (offset, length) = bytes_to_utf16(text, e.start, e.end);
    if length == 0 {
        return None;
    }
    Some(match &e.kind {
        EntityKind::Bold => E::Bold(tl::types::MessageEntityBold { offset, length }),
        EntityKind::Italic => E::Italic(tl::types::MessageEntityItalic { offset, length }),
        EntityKind::Underline => E::Underline(tl::types::MessageEntityUnderline { offset, length }),
        EntityKind::Strike => E::Strike(tl::types::MessageEntityStrike { offset, length }),
        EntityKind::Spoiler => E::Spoiler(tl::types::MessageEntitySpoiler { offset, length }),
        EntityKind::Code => E::Code(tl::types::MessageEntityCode { offset, length }),
        EntityKind::Pre(lang) => E::Pre(tl::types::MessageEntityPre { offset, length, language: lang.clone() }),
        EntityKind::TextUrl(url) => E::TextUrl(tl::types::MessageEntityTextUrl { offset, length, url: url.clone() }),
        EntityKind::Quote => E::Blockquote(tl::types::MessageEntityBlockquote { collapsed: false, offset, length }),
        _ => return None,
    })
}

fn text_of(t: &tl::enums::TextWithEntities) -> String {
    let tl::enums::TextWithEntities::Entities(t) = t;
    t.text.clone()
}

fn media_of(m: &tl::enums::MessageMedia) -> Option<Media> {
    use tl::enums::MessageMedia as M;
    Some(match m {
        M::Photo(p) => {
            let Some(tl::enums::Photo::Photo(photo)) = &p.photo else { return Some(Media::Other("Photo (expired)".into())) };
            let (mut w, mut h) = (0, 0);
            let mut thumb = None;
            for s in &photo.sizes {
                match s {
                    tl::enums::PhotoSize::Size(s) if s.w.max(s.h) > w.max(h) => (w, h) = (s.w, s.h),
                    tl::enums::PhotoSize::Progressive(s) if s.w.max(s.h) > w.max(h) => (w, h) = (s.w, s.h),
                    tl::enums::PhotoSize::PhotoStrippedSize(s) => thumb = Some(s.bytes.clone()),
                    _ => {}
                }
            }
            Media::Photo { w: w.max(0) as u32, h: h.max(0) as u32, thumb }
        }
        M::Document(d) => {
            let Some(tl::enums::Document::Document(doc)) = &d.document else { return Some(Media::Other("File (expired)".into())) };
            let name = doc.attributes.iter().find_map(|a| if let tl::enums::DocumentAttribute::Filename(f) = a { Some(f.file_name.clone()) } else { None });
            let size = doc.size.max(0) as u64;
            let mut out = None;
            for a in &doc.attributes {
                match a {
                    tl::enums::DocumentAttribute::Sticker(s) => {
                        let (w, h) = doc.attributes.iter().find_map(|a| match a {
                            tl::enums::DocumentAttribute::ImageSize(i) => Some((i.w, i.h)),
                            tl::enums::DocumentAttribute::Video(v) => Some((v.w, v.h)),
                            _ => None,
                        }).unwrap_or((512, 512));
                        out = Some(Media::Sticker { emoji: s.alt.clone(), animated: doc.mime_type != "image/webp", w: w.max(0) as u32, h: h.max(0) as u32 });
                        break;
                    }
                    tl::enums::DocumentAttribute::Audio(a) if a.voice || d.voice => {
                        out = Some(Media::Voice { duration: a.duration as f32, waveform: a.waveform.as_deref().map(unpack_waveform).unwrap_or_default(), size });
                    }
                    tl::enums::DocumentAttribute::Audio(a) if out.is_none() => {
                        out = Some(Media::Audio { title: a.title.clone(), performer: a.performer.clone(), duration: a.duration as f32, name: name.clone(), size });
                    }
                    tl::enums::DocumentAttribute::Video(v) if out.is_none() || matches!(out, Some(Media::Audio { .. })) => {
                        let gif = doc.attributes.iter().any(|a| matches!(a, tl::enums::DocumentAttribute::Animated));
                        out = Some(Media::Video { w: v.w.max(0) as u32, h: v.h.max(0) as u32, duration: v.duration as f32, round: v.round_message, gif, name: name.clone(), size });
                    }
                    _ => {}
                }
            }
            out.unwrap_or(Media::File { name: name.unwrap_or_else(|| format!("file.{}", ext_for(&doc.mime_type).unwrap_or("bin"))), size, mime: doc.mime_type.clone() })
        }
        M::Contact(c) => Media::Contact { name: format!("{} {}", c.first_name, c.last_name).trim().to_owned(), phone: c.phone_number.clone() },
        M::Geo(g) => match &g.geo {
            tl::enums::GeoPoint::Point(p) => Media::Location { lat: p.lat, lon: p.long, title: None, address: None },
            _ => Media::Other("Location".into()),
        },
        M::GeoLive(g) => match &g.geo {
            tl::enums::GeoPoint::Point(p) => Media::Location { lat: p.lat, lon: p.long, title: Some("Live location".into()), address: None },
            _ => Media::Other("Live location".into()),
        },
        M::Venue(v) => match &v.geo {
            tl::enums::GeoPoint::Point(p) => Media::Location { lat: p.lat, lon: p.long, title: Some(v.title.clone()), address: Some(v.address.clone()) },
            _ => Media::Other(v.title.clone()),
        },
        M::Poll(p) => {
            let tl::enums::Poll::Poll(poll) = &p.poll;
            let tl::enums::PollResults::Results(res) = &p.results;
            let votes: HashMap<Vec<u8>, u32> = res
                .results
                .iter()
                .flatten()
                .map(|v| {
                    let tl::enums::PollAnswerVoters::Voters(v) = v;
                    (v.option.clone(), v.voters.unwrap_or(0).max(0) as u32)
                })
                .collect();
            let answers = poll
                .answers
                .iter()
                .filter_map(|a| match a {
                    tl::enums::PollAnswer::Answer(a) => Some((text_of(&a.text), votes.get(&a.option).copied().unwrap_or(0))),
                    _ => None,
                })
                .collect();
            Media::Poll { question: text_of(&poll.question), answers, voters: res.total_voters.unwrap_or(0).max(0) as u32, closed: poll.closed, quiz: poll.quiz }
        }
        M::WebPage(w) => match &w.webpage {
            tl::enums::WebPage::Page(p) => Media::WebPage { url: p.url.clone(), site: p.site_name.clone(), title: p.title.clone(), description: p.description.clone(), photo: p.photo.is_some() },
            _ => return None,
        },
        M::Dice(d) => Media::Dice { emoji: d.emoticon.clone(), value: d.value },
        M::Game(_) => Media::Other("Game".into()),
        M::Invoice(_) => Media::Other("Invoice".into()),
        M::Story(_) => Media::Other("Story".into()),
        M::Giveaway(_) | M::GiveawayResults(_) => Media::Other("Giveaway".into()),
        M::PaidMedia(_) => Media::Other("Paid media".into()),
        M::ToDo(_) => Media::Other("Checklist".into()),
        _ => return None,
    })
}

/// Telegram's waveform: 5-bit samples packed little-endian.
fn unpack_waveform(packed: &[u8]) -> Vec<u8> {
    let n = packed.len() * 8 / 5;
    (0..n)
        .map(|i| {
            let bit = i * 5;
            let (byte, shift) = (bit / 8, bit % 8);
            let lo = packed[byte] as u16;
            let hi = packed.get(byte + 1).copied().unwrap_or(0) as u16;
            (((hi << 8 | lo) >> shift) & 31) as u8
        })
        .collect()
}

fn pack_waveform(samples: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; (samples.len() * 5).div_ceil(8)];
    for (i, &s) in samples.iter().enumerate() {
        let bit = i * 5;
        let (byte, shift) = (bit / 8, bit % 8);
        let v = ((s.min(31)) as u16) << shift;
        out[byte] |= v as u8;
        if byte + 1 < out.len() {
            out[byte + 1] |= (v >> 8) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moving a chat: Telegram's answer names the folder it's in now, for
    /// that chat only; an answer without it says nothing (and the chat is
    /// asked about again).
    #[test]
    fn folder_from_the_answer() {
        let user = |id| tl::enums::Peer::User(tl::types::PeerUser { user_id: id });
        let moved = |id, folder_id| tl::enums::FolderPeer::Peer(tl::types::FolderPeer { peer: user(id), folder_id });
        let answer = |updates| tl::enums::Updates::Updates(tl::types::Updates { updates, users: Vec::new(), chats: Vec::new(), date: 0, seq: 0 });
        let r = answer(vec![tl::enums::Update::FolderPeers(tl::types::UpdateFolderPeers { folder_peers: vec![moved(5, 1), moved(7, 0)], pts: 1, pts_count: 1 })]);
        assert_eq!(folder_in(&r, id_of(&user(7))), Some(0));
        assert_eq!(folder_in(&r, id_of(&user(5))), Some(1));
        assert_eq!(folder_in(&r, id_of(&user(9))), None);
        assert_eq!(folder_in(&answer(Vec::new()), id_of(&user(7))), None);
        assert_eq!(folder_in(&tl::enums::Updates::TooLong, id_of(&user(7))), None);
    }

    #[test]
    fn waveforms_round_trip() {
        let w: Vec<u8> = (0..64).map(|i| (i * 7 % 32) as u8).collect();
        let packed = pack_waveform(&w);
        assert_eq!(packed.len(), 40);
        assert_eq!(unpack_waveform(&packed), w);
    }

    #[test]
    fn entities_both_ways() {
        let text = "a😀 bold";
        let raw = vec![tl::enums::MessageEntity::Bold(tl::types::MessageEntityBold { offset: 4, length: 4 })];
        let e = entities_of(text, &raw);
        assert_eq!(e, [Entity { start: 6, end: 10, kind: EntityKind::Bold }]);
        let back = entity_to_raw(text, &e[0]).unwrap();
        assert_eq!(format!("{back:?}"), format!("{:?}", raw[0]));
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    /// Updates caught up on after the chat list was read change nothing
    /// they already account for: Justin's 35 unread came from old
    /// messages counted again on every start.
    #[test]
    fn replayed_updates_change_nothing() {
        let msg = |id, out| Message { id, chat: 5, out, ..Message::default() };
        let chat = Chat { id: 5, read_inbox: 40, last: Some(Preview::of(&msg(42, false))), unread: 2, ..Chat::default() };
        // Already in the list (and so in its count), or read: not again.
        assert!(!counts_unread(&chat, &msg(42, false)));
        assert!(!counts_unread(&chat, &msg(30, false)));
        // New, from them: one more; ours: none.
        assert!(counts_unread(&chat, &msg(43, false)));
        assert!(!counts_unread(&chat, &msg(43, true)));
        // A read older than, or as old as, what's known read is old news.
        assert!(!read_moves_on(&chat, 35) && !read_moves_on(&chat, 40));
        assert!(read_moves_on(&chat, 42));
    }
}
