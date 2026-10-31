//! Telegram for Olo (`olo-telegram`): the chat list on the left, the
//! conversation on the right. The account is reached through `olo-tg`
//! (MTProto in process); signing in and the API ID are in Preferences ▸
//! Integrations ▸ Telegram. New messages notify through Olo's
//! notifications, answerable from the banner. `OLO_TELEGRAM_DEMO=1`
//! shows a pretend account (tests, screenshots).

mod bubble;
mod conv;
mod list;
mod pics;
mod store;
mod when;

use std::{collections::HashMap, path::PathBuf};

use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key, Rect, pos2, vec2},
    file_panel::{FilePanel, PanelResult},
    instance,
    notify::{Note, NoteEvent, NoteId, Notifier},
    style,
    widgets::{self, Button, EmptyState},
};
use olo_tg::{Backend, Chat, ChatId, ChatKind, Cmd, Contact, Doing, Event, Media, Me, Message, MsgId, Outgoing, Pending, Pic, Status};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.telegram";
/// Telegram's blue, for notifications.
const ACCENT: [u8; 3] = [0x2A, 0xAB, 0xEE];

/// What's kept over a reload or between runs.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    chat: Option<ChatId>,
    topic: Option<MsgId>,
    folder: Option<i32>,
    list_width: Option<f32>,
}

enum Panel {
    Attach,
    SaveAs(ChatId, MsgId),
}

enum Alert {
    Delete { chat: ChatId, ids: Vec<MsgId>, mine: bool },
    SignOut,
}

struct Forward {
    window: WindowId,
    from: ChatId,
    ids: Vec<MsgId>,
    query: String,
}

struct Tg {
    backend: Backend,
    store: store::Store,
    pics: pics::Pics,
    status: Status,
    me: Option<Me>,
    main: Option<WindowId>,
    view: list::View,
    found: list::Found,
    conv: Option<conv::Conv>,
    list_width: f32,
    h24: bool,
    notes: Notifier,
    /// Each chat's notification, updated as more come.
    chat_notes: HashMap<ChatId, (Option<NoteId>, Vec<String>)>,
    note_chats: HashMap<NoteId, (ChatId, MsgId)>,
    panel: Option<(WindowId, FilePanel, Panel)>,
    alert: Option<(WindowId, Alert)>,
    forward: Option<Forward>,
    contacts: Option<(WindowId, Vec<Contact>, String)>,
    info: Option<(WindowId, ChatId)>,
    player: Option<(ChatId, MsgId, olo_preview::audio::Player)>,
    /// Play or open this once it's downloaded.
    when_fetched: Option<(Pic, bool)>,
    next_local: u64,
    focused: bool,
    /// What's typed in each chat's field, kept when switching.
    drafts: HashMap<ChatId, String>,
    banner: Option<(String, f64)>,
    searched: (String, f64),
    asked_chats: bool,
    restored: Saved,
    /// The unread count Deskbar was last told.
    badge: u32,
    /// The main window's context (textures are made in it).
    ctx: Option<egui::Context>,
    focus_search: bool,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl Tg {
    fn redraw(&self, cx: &mut Cx) {
        if let Some(w) = self.main {
            cx.redraw(w);
        }
    }

    fn open_chat(&mut self, cx: &mut Cx, chat: ChatId, topic: Option<MsgId>) {
        // Keep what was being written in the chat we leave.
        if let Some(c) = self.conv.take() {
            let old = self.store.chats.get(&c.chat).and_then(|x| x.draft.clone()).unwrap_or_default();
            if c.text.trim() != old.trim() {
                self.backend.send(Cmd::Draft { chat: c.chat, text: c.text.clone() });
            }
            self.drafts.insert(c.chat, c.text);
        }
        let text = self.drafts.remove(&chat).or_else(|| self.store.chats.get(&chat).and_then(|c| c.draft.clone())).unwrap_or_default();
        let mut c = conv::Conv::new(chat, topic, text);
        if let Some(ch) = self.store.chats.get(&chat)
            && ch.unread > 0
        {
            c.unread_from = Some(ch.read_inbox + 1);
        }
        let t = self.store.thread(chat, topic);
        if !t.loading {
            t.loading = true;
            self.backend.send(Cmd::Open { chat, topic });
        }
        self.conv = Some(c);
        if let Some(w) = self.main {
            let title = self.store.chats.get(&chat).map_or_else(|| "Telegram".into(), |c| c.title.clone());
            cx.set_title(w, &title);
        }
        // Its notification has been seen to.
        if let Some((Some(id), _)) = self.chat_notes.remove(&chat) {
            self.notes.close(id);
        }
        self.redraw(cx);
    }

    fn send(&mut self, cx: &mut Cx) {
        let Some(c) = &mut self.conv else { return };
        let text = c.text.trim_end().to_owned();
        if let Some(id) = c.edit.take() {
            self.backend.send(Cmd::Edit { chat: c.chat, id, text, entities: Vec::new() });
            c.text.clear();
            self.redraw(cx);
            return;
        }
        if text.trim().is_empty() && c.files.is_empty() {
            return;
        }
        self.next_local += 1;
        let local = self.next_local;
        let files = std::mem::take(&mut c.files);
        let reply = c.reply.take();
        let media = files.first().map(|f| {
            let name = f.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            Media::File { name, size: std::fs::metadata(f).map_or(0, |m| m.len()), mime: String::new() }
        });
        let reply_to = reply.and_then(|r| {
            let m = self.store.threads.get(&(c.chat, None))?.msgs.get(&r)?;
            Some(olo_tg::model::ReplyTo { id: r, from: m.from.as_ref().map(|f| f.name.clone()), text: Some(m.summary()) })
        });
        let pending = Message {
            id: i32::MAX,
            chat: c.chat,
            from: self.me.as_ref().map(|m| olo_tg::model::Sender { id: m.id, name: "You".into(), photo: None }),
            date: when::now(),
            out: true,
            text: text.clone(),
            reply_to,
            media,
            topic: c.topic,
            pending: Some((local, Pending::Sending)),
            ..Message::default()
        };
        self.backend.send(Cmd::Send(Outgoing { chat: c.chat, topic: c.topic, text, entities: Vec::new(), reply_to: reply, files, as_files: false, voice: None, local }));
        c.text.clear();
        c.at_bottom = true;
        self.drafts.remove(&c.chat);
        self.store.pending(pending);
        self.redraw(cx);
    }

    fn notify(&mut self, m: &Message) {
        let Some(chat) = self.store.chats.get(&m.chat) else { return };
        if m.out || chat.muted || m.service.is_some() {
            return;
        }
        // Not while the person is looking at it.
        if self.focused && self.conv.as_ref().is_some_and(|c| c.chat == m.chat) {
            return;
        }
        let from = m.from.as_ref().map_or_else(|| chat.title.clone(), |f| f.name.clone());
        let (id, lines) = self.chat_notes.entry(m.chat).or_insert_with(|| (None, Vec::new()));
        lines.push(m.summary());
        let keep = lines.len().saturating_sub(4);
        lines.drain(..keep);
        let mut note = Note::new(if chat.kind.many() { chat.title.clone() } else { from.clone() })
            .body(lines.join("\n"))
            .accent(ACCENT)
            .style(olo_protocol::NoteStyle::Message)
            .kind(olo_kit::notify::NoteKind::Important)
            .action("default", "Show")
            .reply(format!("Reply to {}", chat.title));
        if chat.kind.many() {
            note = note.subtitle(from);
        }
        let shown = self.notes.show(*id, note);
        *id = Some(shown);
        self.note_chats.insert(shown, (m.chat, m.id));
    }

    fn event(&mut self, cx: &mut Cx, e: Event) {
        match e {
            Event::Status(s) => {
                if matches!(s, Status::Online | Status::Updating | Status::Connecting) && !self.asked_chats {
                    self.asked_chats = true;
                }
                self.status = s;
            }
            Event::Me(m) => self.me = Some(m),
            Event::Chats { chats, done, cached } => {
                for c in chats {
                    self.store.put_chat(c);
                }
                self.store.sort();
                if done && !cached {
                    self.store.complete = true;
                }
                // The chat open before a reload, once it's known.
                if self.conv.is_none()
                    && let Some(chat) = self.restored.chat.take()
                {
                    if self.store.chats.contains_key(&chat) {
                        let topic = self.restored.topic.take();
                        self.open_chat(cx, chat, topic);
                    } else if !done {
                        self.restored.chat = Some(chat);
                    }
                }
            }
            Event::Chat(c) => {
                if self.store.chats.get(&c.id).is_some_and(|o| o.photo != c.photo) {
                    self.pics.forget(&Pic::Avatar(c.id));
                }
                // Read ticks are drawn from the chat: re-lay its threads.
                if let Some(o) = self.store.chats.get(&c.id)
                    && o.read_outbox != c.read_outbox
                {
                    for ((ch, _), t) in self.store.threads.iter_mut() {
                        if *ch == c.id {
                            t.version += 1;
                        }
                    }
                }
                self.store.put_chat(c);
                self.store.sort();
            }
            Event::ChatGone(id) => {
                self.store.chats.remove(&id);
                self.store.sort();
            }
            Event::Folders(f) => self.store.folders = f,
            Event::Messages { chat, topic, msgs, more, before } => self.store.page(chat, topic, msgs, more, before.is_none()),
            Event::Around { chat, id, msgs, more } => {
                self.store.page(chat, None, msgs, more, false);
                if let Some(c) = &mut self.conv
                    && c.chat == chat
                {
                    c.jump = Some(id);
                }
            }
            Event::New(m) => {
                self.notify(&m);
                self.store.message(m);
            }
            Event::Sent { local, msg } => self.store.sent(local, Some(msg)),
            Event::SendFailed { local, why, .. } => {
                self.store.sent(local, None);
                self.banner = Some((why, now() + 8.0));
            }
            Event::Edited(m) => self.store.message(m),
            Event::Deleted { chat, ids } => self.store.deleted(chat, &ids),
            Event::Read { .. } => {}
            Event::Doing { chat, topic, who, doing } => {
                let list = self.store.typing.entry((chat, topic)).or_default();
                list.retain(|t| t.who != who);
                if doing != Doing::Stopped {
                    list.push(store::Typing { who, doing, until: now() + 6.0 });
                }
            }
            Event::Topics { chat, topics } => {
                self.store.topics.insert(chat, topics);
            }
            Event::Fetched { pic, path } => {
                self.pics.fetched(pic.clone(), path.clone());
                if let Some((want, play)) = self.when_fetched.clone()
                    && want == pic
                {
                    self.when_fetched = None;
                    if play {
                        self.play(&pic, &path);
                    } else {
                        let _ = olo_kit::open::open(&olo_kit::open::Target::Path(path));
                    }
                }
            }
            Event::Progress { pic: Some(pic), done, .. } => {
                self.pics.progress.insert(pic, done);
            }
            Event::Progress { .. } => {}
            Event::FetchFailed { pic, why } => {
                if matches!(pic, Pic::File(..)) {
                    self.banner = Some((why, now() + 8.0));
                }
                self.pics.fetch_failed(pic);
            }
            Event::Saved { to } => {
                self.notes.show(None, Note::new("Saved").body(to.display().to_string()).accent(ACCENT));
            }
            Event::Results { query, chats, msgs } => self.found = list::Found { query, chats, msgs },
            Event::Contacts(list) => {
                if let Some((_, l, _)) = &mut self.contacts {
                    *l = list;
                }
            }
            Event::Resolved { username, chat } => match chat {
                Some(c) => {
                    let id = c.id;
                    if !self.store.chats.contains_key(&id) {
                        self.store.put_chat(c);
                        self.store.sort();
                    }
                    self.open_chat(cx, id, None);
                }
                None => self.banner = Some((format!("No one on Telegram is called {username}."), now() + 6.0)),
            },
            Event::Error(e) => self.banner = Some((e, now() + 8.0)),
        }
        let unread = self.store.total_unread();
        if unread != self.badge {
            self.badge = unread;
            self.notes.badge(unread);
        }
        self.redraw(cx);
    }

    /// Talk to the account afresh (the old backend ends once dropped).
    fn reconnect(&mut self, cx: &mut Cx) {
        let proxy = cx.proxy();
        self.backend = Backend::start(move |e| proxy.send(e));
        self.backend.send(Cmd::Chats);
        self.store = store::Store::default();
        self.conv = None;
        self.me = None;
        self.status = Status::Connecting;
        self.pics = pics::Pics::new(cx.proxy());
        self.redraw(cx);
    }

    fn play(&mut self, pic: &Pic, path: &std::path::Path) {
        let Pic::File(chat, id) = pic else { return };
        self.player = Some((*chat, *id, olo_preview::audio::Player::start(&olo_preview::input::Input::file(path))));
    }

    fn hit(&mut self, cx: &mut Cx, id: MsgId, hit: bubble::Hit) {
        let Some(c) = &mut self.conv else { return };
        let chat = c.chat;
        match hit {
            bubble::Hit::Reply(r) => {
                let have = self.store.threads.get(&(chat, None)).is_some_and(|t| t.msgs.contains_key(&r));
                if have {
                    c.jump = Some(r);
                } else {
                    self.backend.send(Cmd::Around { chat, id: r });
                }
            }
            bubble::Hit::Reaction(e) => {
                let mine = self.store.threads.get(&(chat, c.topic)).and_then(|t| t.msgs.get(&id)).is_some_and(|m| m.reactions.iter().any(|r| r.mine && r.emoji == e));
                self.backend.send(Cmd::React { chat, id, emoji: if mine { None } else { Some(e) } });
            }
            bubble::Hit::Link(url) => {
                if let Some(user) = url.strip_prefix("https://t.me/").filter(|u| !u.contains('/')) {
                    self.backend.send(Cmd::Resolve { username: user.to_owned() });
                } else {
                    let _ = olo_kit::open::open(&olo_kit::open::Target::Url(url));
                }
            }
            bubble::Hit::Mention(name) => self.backend.send(Cmd::Resolve { username: name }),
            bubble::Hit::MentionId(user) | bubble::Hit::Sender(user) => {
                if self.store.chats.contains_key(&user) {
                    self.open_chat(cx, user, None);
                }
            }
            bubble::Hit::Hashtag(tag) => {
                self.view.query = tag.clone();
                self.backend.send(Cmd::Search { query: tag });
            }
            bubble::Hit::Spoiler => {
                c.revealed.insert(id);
            }
            bubble::Hit::Media => self.open_media(chat, id),
        }
    }

    /// A message's picture, file or voice: open or play it.
    fn open_media(&mut self, chat: ChatId, id: MsgId) {
        let Some(m) = self.store.threads.get(&(chat, None)).and_then(|t| t.msgs.get(&id)).or_else(|| self.conv.as_ref().and_then(|c| self.store.threads.get(&(chat, c.topic))?.msgs.get(&id))) else { return };
        let pic = Pic::File(chat, id);
        let play = matches!(m.media, Some(Media::Voice { .. } | Media::Audio { .. }));
        match &m.media {
            Some(Media::Location { lat, lon, .. }) => {
                let _ = olo_kit::open::open(&olo_kit::open::Target::Url(format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}")));
                return;
            }
            Some(Media::WebPage { url, .. }) => {
                let _ = olo_kit::open::open(&olo_kit::open::Target::Url(url.clone()));
                return;
            }
            Some(Media::Photo { .. } | Media::Video { .. } | Media::File { .. } | Media::Audio { .. } | Media::Voice { .. } | Media::Sticker { .. }) => {}
            _ => return,
        }
        if play && self.player.as_ref().is_some_and(|p| p.0 == chat && p.1 == id) {
            if let Some((_, _, p)) = &self.player {
                p.toggle();
            }
            return;
        }
        match self.pics.file(&pic).map(PathBuf::from) {
            Some(path) if play => self.play(&pic, &path),
            Some(path) => {
                let _ = olo_kit::open::open(&olo_kit::open::Target::Path(path));
            }
            None => {
                self.when_fetched = Some((pic.clone(), play));
                self.pics.fetch_file(pic, &self.backend);
            }
        }
    }

    fn message_command(&mut self, cx: &mut Cx, id: MsgId, cmd: &str) {
        let Some(c) = &mut self.conv else { return };
        let chat = c.chat;
        let Some(m) = self.store.threads.get(&(chat, c.topic)).and_then(|t| t.msgs.get(&id)).cloned() else { return };
        match cmd {
            "reply" if m.service.is_none() && m.pending.is_none() => {
                c.reply = Some(id);
                c.edit = None;
                c.focus_field = true;
            }
            "edit" if m.out => {
                c.edit = Some(id);
                c.reply = None;
                c.text = m.text.clone();
                c.focus_field = true;
            }
            "copy" => cx.copy(m.text.clone()),
            "forward" => self.open_forward(cx, chat, vec![id]),
            "pin" => self.backend.send(Cmd::PinMessage { chat, id, on: !m.pinned }),
            "open" => self.open_media(chat, id),
            "save" => {
                let name = match &m.media {
                    Some(Media::File { name, .. }) => name.clone(),
                    Some(Media::Photo { .. }) => format!("Photo {id}.jpg"),
                    Some(Media::Video { name, .. }) | Some(Media::Audio { name, .. }) => name.clone().unwrap_or_else(|| format!("Telegram {id}")),
                    Some(Media::Voice { .. }) => format!("Voice message {id}.ogg"),
                    _ => format!("Telegram {id}"),
                };
                let dir = olo_kit::user_dir("DOWNLOAD").unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
                let w = cx.open_window(WindowOptions::new("Save As", olo_kit::file_panel::PANEL_SIZE.0, olo_kit::file_panel::PANEL_SIZE.1).dialog().parent_opt(self.main));
                self.panel = Some((w, FilePanel::save(dir, name), Panel::SaveAs(chat, id)));
            }
            "delete" => {
                let mine = m.out;
                let text = if mine && self.store.chats.get(&chat).is_some_and(|c| c.kind != ChatKind::Channel) {
                    "Delete this message for everyone in the chat?"
                } else {
                    "Delete this message for you?"
                };
                let w = cx.open_window(WindowOptions::alert_buttons("Delete Message", text, &["Cancel", "Delete"]).parent_opt(self.main));
                self.alert = Some((w, Alert::Delete { chat, ids: vec![id], mine }));
            }
            c2 if c2.starts_with("react:") => {
                let e = c2.trim_start_matches("react:").to_owned();
                let mine = m.reactions.iter().any(|r| r.mine && r.emoji == e);
                self.backend.send(Cmd::React { chat, id, emoji: if mine { None } else { Some(e) } });
            }
            _ => {}
        }
        self.redraw(cx);
    }

    fn chat_command(&mut self, cx: &mut Cx, chat: ChatId, cmd: &str) {
        let Some(c) = self.store.chats.get(&chat).cloned() else { return };
        match cmd {
            "chat:pin" => self.backend.send(Cmd::PinChat { chat, on: !c.pinned }),
            "chat:mute" => self.backend.send(Cmd::Mute { chat, on: !c.muted }),
            "chat:read" => {
                if c.unread > 0 || c.marked {
                    let up_to = c.last.as_ref().map_or(0, |l| l.id);
                    self.backend.send(Cmd::Read { chat, up_to });
                } else {
                    self.backend.send(Cmd::MarkUnread { chat, on: true });
                }
            }
            "chat:archive" => self.backend.send(Cmd::Archive { chat, on: !c.archived }),
            _ => {}
        }
        self.redraw(cx);
    }

    fn open_forward(&mut self, cx: &mut Cx, from: ChatId, ids: Vec<MsgId>) {
        if let Some(f) = &self.forward {
            cx.close_window(f.window);
        }
        let window = cx.open_window(WindowOptions::new("Forward To", 360, 480).min_size(300, 320).dialog().parent_opt(self.main));
        self.forward = Some(Forward { window, from, ids, query: String::new() });
    }

    fn open_contacts(&mut self, cx: &mut Cx) {
        if let Some((w, _, _)) = &self.contacts {
            cx.activate(*w);
            return;
        }
        let w = cx.open_window(WindowOptions::new("Contacts", 360, 520).min_size(300, 320));
        self.contacts = Some((w, Vec::new(), String::new()));
        self.backend.send(Cmd::Contacts);
    }

    fn open_info(&mut self, cx: &mut Cx, chat: ChatId) {
        if let Some((w, _)) = self.info.take() {
            cx.close_window(w);
        }
        let title = self.store.chats.get(&chat).map_or_else(|| "Info".into(), |c| c.title.clone());
        let w = cx.open_window(WindowOptions::new(&title, 340, 420).min_size(300, 360).max_size(300, 1000).parent_opt(self.main));
        self.info = Some((w, chat));
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        self.ctx = Some(ui.ctx().clone());
        if std::mem::take(&mut self.focus_search) {
            olo_kit::search::SearchField::focus(ui.ctx(), "chat-search");
        }
        if self.conv.is_some() {
            olo_kit::dnd::open_files(ui, "attach");
        }
        let full = ui.available_rect_before_wrap();
        let p = style::palette(ui.ctx());
        ui.painter().rect_filled(full, 0.0, style::c(p.panel_background));
        self.focused = ui.input(|i| i.focused);
        match &self.status {
            Status::NotSetUp | Status::SignedOut => {
                let (title, detail) = if self.status == Status::NotSetUp {
                    ("Telegram isn’t set up yet", "Connect your Telegram account in Preferences ▸ Integrations: an API ID from my.telegram.org, then sign in with a code or a QR code.")
                } else {
                    ("Not signed in to Telegram", "Sign in to your Telegram account in Preferences ▸ Integrations, with a code sent to your phone or by scanning a QR code.")
                };
                let r = ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| ui.add(EmptyState::new(title).icon("telegram").detail(detail).action("Open Integrations…"))).inner;
                if r.clicked() {
                    let _ = olo_kit::spawn_detached("olo-preferences integrations");
                }
                return;
            }
            _ => {}
        }
        // The list, a grip, the conversation.
        // On the 2-point grid, so the panes' edges are on device pixels at 1.5×.
        let lw = (self.list_width.clamp(list::MIN_WIDTH, (full.width() - 320.0).max(list::MIN_WIDTH)) / 2.0).round() * 2.0;
        let left = Rect::from_min_size(full.min, vec2(lw, full.height()));
        let right = Rect::from_min_max(pos2(style::snap_coord(ui.ctx(), left.right() + style::hairline(ui.ctx())), full.top()), full.max);
        let grip = Rect::from_min_max(pos2(left.right() - 2.0, full.top()), pos2(left.right() + 3.0, full.bottom()));
        let gr = ui.interact(grip, ui.make_persistent_id("list-grip"), egui::Sense::drag());
        if gr.hovered() || gr.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if gr.dragged() {
            self.list_width = (lw + gr.drag_delta().x).max(list::MIN_WIDTH);
        }
        gr.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Splitter, true, "Chat List Width"));
        // Ask Telegram while typing a search (after a pause).
        let t = ui.input(|i| i.time);
        let q = self.view.query.trim().to_owned();
        if !q.is_empty() && q != self.found.query && q != self.searched.0 {
            if t - self.searched.1 > 0.4 {
                self.searched = (q.clone(), t);
                self.backend.send(Cmd::Search { query: q });
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
            }
        }
        let selected = self.conv.as_ref().map(|c| c.chat);
        let act = list::show(
            ui,
            left,
            list::ListUi { store: &self.store, pics: &mut self.pics, backend: &self.backend, view: &mut self.view, selected, found: &self.found, h24: self.h24 },
        );
        style::vline(ui.painter(), left.right(), full.top(), full.bottom(), style::c(p.shadow));
        match act {
            Some(list::Act::Open(id)) => self.open_chat(cx, id, None),
            Some(list::Act::OpenAt(chat, id)) => {
                self.open_chat(cx, chat, None);
                if self.store.threads.get(&(chat, None)).is_some_and(|t| t.msgs.contains_key(&id)) {
                    if let Some(c) = &mut self.conv {
                        c.jump = Some(id);
                    }
                } else {
                    self.backend.send(Cmd::Around { chat, id });
                }
            }
            Some(list::Act::Command(id, cmd)) => self.chat_command(cx, id, &cmd),
            Some(list::Act::Search(q)) => {
                if self.searched.1 == 0.0 {
                    self.searched.1 = t;
                }
                let _ = q;
            }
            None => {}
        }
        // The conversation.
        let playing = self.player.as_ref().map(|(c, m, p)| {
            let st = &p.state;
            let pos = st.position_ms.load(std::sync::atomic::Ordering::Relaxed) as f32;
            let dur = st.duration_ms.load(std::sync::atomic::Ordering::Relaxed).max(1) as f32;
            (*c, *m, (pos / dur).clamp(0.0, 1.0))
        });
        if self.player.as_ref().is_some_and(|(_, _, p)| p.state.finished.load(std::sync::atomic::Ordering::Relaxed)) {
            self.player = None;
        }
        let me = self.me.as_ref().map_or(0, |m| m.id);
        let acts = match &mut self.conv {
            Some(c) => conv::show(ui, right, c, conv::ConvUi { store: &self.store, pics: &mut self.pics, backend: &self.backend, h24: self.h24, me, playing }),
            None => {
                ui.painter().rect_filled(right, 0.0, bubble::Look::of(&p).ground);
                ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                    let detail = match &self.status {
                        Status::Connecting => "Connecting to Telegram…".to_owned(),
                        Status::Updating => "Catching up…".to_owned(),
                        Status::Offline(why) => why.clone(),
                        Status::Waiting(s) => format!("Telegram asks us to wait {s} seconds."),
                        _ => "Choose a chat from the list to start messaging.".to_owned(),
                    };
                    ui.add(EmptyState::new("No chat selected").icon("telegram").detail(&detail));
                });
                Vec::new()
            }
        };
        for a in acts {
            match a {
                conv::Act::Send => self.send(cx),
                conv::Act::Older => {
                    if let Some(c) = &self.conv
                        && let Some(t) = self.store.threads.get_mut(&(c.chat, c.topic))
                        && let Some(before) = t.oldest()
                    {
                        t.loading = true;
                        self.backend.send(Cmd::Older { chat: c.chat, topic: c.topic, before });
                    }
                }
                conv::Act::Read(id) => {
                    if let Some(c) = &self.conv
                        && self.focused
                        && let Some(ch) = self.store.chats.get_mut(&c.chat)
                        && (id > ch.read_inbox || ch.marked)
                        && (ch.unread > 0 || ch.marked)
                    {
                        ch.read_inbox = id;
                        self.backend.send(Cmd::Read { chat: c.chat, up_to: id });
                    }
                }
                conv::Act::Typing => {
                    if let Some(c) = &mut self.conv {
                        let t = ui.input(|i| i.time);
                        if t - c.typing_sent > 5.0 {
                            c.typing_sent = t;
                            self.backend.send(Cmd::Typing { chat: c.chat, topic: c.topic, doing: Doing::Typing });
                        }
                    }
                }
                conv::Act::Message(id, cmd) => self.message_command(cx, id, &cmd),
                conv::Act::Hit(id, h) => self.hit(cx, id, h),
                conv::Act::Attach => {
                    let dir = olo_kit::user_dir("PICTURES").unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
                    let w = cx.open_window(WindowOptions::new("Attach Files", olo_kit::file_panel::PANEL_SIZE.0, olo_kit::file_panel::PANEL_SIZE.1).dialog().parent_opt(self.main));
                    self.panel = Some((w, FilePanel::open(dir).multiple(), Panel::Attach));
                }
                conv::Act::Info => {
                    if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                        self.open_info(cx, chat);
                    }
                }
                conv::Act::Topic(t) => {
                    if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                        self.open_chat(cx, chat, t);
                    }
                }
                conv::Act::CancelReply => {
                    if let Some(c) = &mut self.conv {
                        if c.edit.take().is_some() {
                            c.text.clear();
                        }
                        c.reply = None;
                    }
                }
                conv::Act::Unstage(i) => {
                    if let Some(c) = &mut self.conv
                        && i < c.files.len()
                    {
                        c.files.remove(i);
                    }
                }
                conv::Act::Bottom => {
                    if let Some(c) = &mut self.conv {
                        c.at_bottom = true;
                    }
                }
            }
        }
        // A problem to tell, across the conversation's top.
        if let Some((text, until)) = self.banner.clone() {
            if now() > until {
                self.banner = None;
            } else {
                let r = Rect::from_min_size(pos2(right.left() + 12.0, right.top() + conv::HEADER + 8.0), vec2(right.width() - 24.0, 30.0));
                ui.painter().rect_filled(style::snap(ui.ctx(), r), 8.0, style::c(p.failure).gamma_multiply(0.92));
                style::label(ui.painter(), r.left() + 12.0, r.center().y, egui::Align::LEFT, &text, style::plain_font(), egui::Color32::WHITE);
                style::describe_text(ui.ctx(), egui::accesskit::Role::Alert, &text, r, None);
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            }
        }
        if self.store.typing.values().any(|l| !l.is_empty()) {
            let t = now();
            for l in self.store.typing.values_mut() {
                l.retain(|x| x.until > t);
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    fn forward_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        let Some(f) = &mut self.forward else { return };
        let mut chosen = None;
        let mut cancel = false;
        widgets::window_content(ui, |ui| {
            ui.add(olo_kit::search::SearchField::new("forward-search", &mut f.query).hint("Search chats"));
            ui.add_space(olo_kit::metrics::TIGHT);
            let q = f.query.to_lowercase();
            let ids: Vec<&Chat> = self.store.order.iter().filter_map(|id| self.store.chats.get(id)).filter(|c| q.is_empty() || c.title.to_lowercase().contains(&q)).collect();
            let h = ui.available_height() - olo_kit::metrics::CONTROL_HEIGHT - olo_kit::metrics::SPACING;
            ui.allocate_ui(vec2(ui.available_width(), h), |ui| {
                widgets::ScrollView::new("forward-list").show_rows(ui, 40.0, ids.len(), |ui, range| {
                    for i in range {
                        let c = ids[i];
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 0.0, style::c(style::palette(ui.ctx()).list_item_text).gamma_multiply(0.06));
                        }
                        let pic = Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(30.0, 30.0));
                        let tex = if c.photo.is_some() { self.pics.get(&Pic::Avatar(c.id), &self.backend).cloned() } else { None };
                        bubble::avatar(ui.painter(), pic, c.id, &c.title, tex.as_ref(), style::palette(ui.ctx()).mode == olo_kit::theme::ThemeMode::Dark);
                        style::label(ui.painter(), pic.right() + 10.0, r.center().y, egui::Align::LEFT, &c.title, style::plain_font(), style::c(style::palette(ui.ctx()).list_item_text));
                        style::describe_item(ui.ctx(), &c.title, r, false);
                        if resp.clicked() {
                            chosen = Some(c.id);
                        }
                    }
                });
            });
            widgets::button_row(ui, |ui| {
                if ui.add(Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
            });
        });
        if let Some(to) = chosen {
            self.backend.send(Cmd::Forward { from: f.from, ids: f.ids.clone(), to });
            cx.close_window(f.window);
            self.forward = None;
            self.open_chat(cx, to, None);
        } else if cancel {
            cx.close_window(f.window);
            self.forward = None;
        }
    }

    fn contacts_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        let Some((w, list, query)) = &mut self.contacts else { return };
        let window = *w;
        let mut open = None;
        widgets::window_content(ui, |ui| {
            ui.add(olo_kit::search::SearchField::new("contacts-search", query).hint("Search contacts"));
            ui.add_space(olo_kit::metrics::TIGHT);
            let q = query.to_lowercase();
            let shown: Vec<&Contact> = list.iter().filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || c.username.as_ref().is_some_and(|u| u.contains(&q))).collect();
            if list.is_empty() {
                ui.add(EmptyState::new("Loading contacts…").icon("people"));
                return;
            }
            widgets::ScrollView::new("contacts-list").show_rows(ui, 46.0, shown.len(), |ui, range| {
                let p = style::palette(ui.ctx());
                for i in range {
                    let c = shown[i];
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), egui::Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 0.0, style::c(p.list_item_text).gamma_multiply(0.06));
                    }
                    let pic = Rect::from_center_size(pos2(r.left() + 24.0, r.center().y), vec2(34.0, 34.0));
                    let tex = if c.photo.is_some() { self.pics.get(&Pic::Avatar(c.id), &self.backend).cloned() } else { None };
                    bubble::avatar(ui.painter(), pic, c.id, &c.name, tex.as_ref(), p.mode == olo_kit::theme::ThemeMode::Dark);
                    style::label(ui.painter(), pic.right() + 10.0, r.top() + 16.0, egui::Align::LEFT, &c.name, style::bold_font(), style::c(p.list_item_text));
                    let detail = c.seen.as_ref().map(|s| s.describe(when::now())).or_else(|| c.phone.clone()).unwrap_or_default();
                    style::label(ui.painter(), pic.right() + 10.0, r.top() + 31.0, egui::Align::LEFT, &detail, style::small_font(), style::disabled_text(&p));
                    style::describe_item(ui.ctx(), &c.name, r, false);
                    if resp.clicked() {
                        open = Some(c.clone());
                    }
                }
            });
        });
        if let Some(c) = open {
            if !self.store.chats.contains_key(&c.id) {
                self.store.put_chat(Chat { id: c.id, kind: ChatKind::User, title: c.name.clone(), username: c.username.clone(), photo: c.photo, contact: true, seen: c.seen.clone(), ..Chat::default() });
                self.store.sort();
            }
            self.open_chat(cx, c.id, None);
            if let Some(m) = self.main {
                cx.activate(m);
            }
            let _ = window;
        }
    }

    fn info_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        let Some((_, chat)) = self.info else { return };
        let Some(c) = self.store.chats.get(&chat).cloned() else { return };
        let p = style::palette(ui.ctx());
        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, style::c(p.panel_background));
        let pic = Rect::from_center_size(pos2(full.center().x, full.top() + 70.0), vec2(96.0, 96.0));
        let tex = if c.photo.is_some() { self.pics.get(&Pic::Avatar(c.id), &self.backend).cloned() } else { None };
        bubble::avatar(ui.painter(), pic, c.id, &c.title, tex.as_ref(), p.mode == olo_kit::theme::ThemeMode::Dark);
        let ink = style::c(p.panel_text);
        style::label(ui.painter(), full.center().x, pic.bottom() + 20.0, egui::Align::Center, &c.title, egui::FontId::new(16.0, style::bold()), ink);
        let sub = match c.kind {
            ChatKind::User | ChatKind::Bot => c.seen.as_ref().map(|s| s.describe(when::now())).unwrap_or_default(),
            ChatKind::Channel => c.members.map(|n| format!("{n} subscribers")).unwrap_or_default(),
            ChatKind::Group | ChatKind::Forum => c.members.map(|n| format!("{n} members")).unwrap_or_default(),
            ChatKind::Saved => "Your notes, files and forwarded messages".into(),
        };
        style::label(ui.painter(), full.center().x, pic.bottom() + 40.0, egui::Align::Center, &sub, style::plain_font(), style::disabled_text(&p));
        let body = Rect::from_min_max(pos2(full.left(), pic.bottom() + 60.0), full.max);
        let mut cmd = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink(olo_kit::metrics::WINDOW_INSET)), |ui| {
            if let Some(u) = &c.username {
                widgets::BBox::new("Username").show(ui, |ui| {
                    ui.add(widgets::StringView::new(&format!("@{u}")));
                });
            }
            ui.add_space(olo_kit::metrics::SPACING);
            let mut muted = c.muted;
            if ui.add(widgets::CheckBox::new(&mut muted, "Mute notifications")).clicked() {
                cmd = Some("chat:mute");
            }
            let mut pinned = c.pinned;
            if ui.add(widgets::CheckBox::new(&mut pinned, "Pin in chat list")).clicked() {
                cmd = Some("chat:pin");
            }
            let mut archived = c.archived;
            if ui.add(widgets::CheckBox::new(&mut archived, "Archived")).clicked() {
                cmd = Some("chat:archive");
            }
            if let Some(u) = &c.username {
                ui.add_space(olo_kit::metrics::SPACING);
                if ui.add(Button::new("Copy Link")).clicked() {
                    cx.copy(format!("https://t.me/{u}"));
                }
            }
        });
        if let Some(cmd) = cmd {
            self.chat_command(cx, chat, cmd);
        }
    }
}

impl App for Tg {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        if Some(window) == self.main {
            return self.main_ui(cx, ui);
        }
        if self.forward.as_ref().is_some_and(|f| f.window == window) {
            return self.forward_ui(cx, ui);
        }
        if self.contacts.as_ref().is_some_and(|c| c.0 == window) {
            return self.contacts_ui(cx, ui);
        }
        if self.info.is_some_and(|i| i.0 == window) {
            return self.info_ui(cx, ui);
        }
        if let Some((w, panel, _)) = &mut self.panel
            && *w == window
        {
            let r = widgets::window_content(ui, |ui| panel.ui(ui));
            let done = match r {
                Some(PanelResult::Chosen(path)) => Some(vec![path]),
                Some(PanelResult::Several(paths)) => Some(paths),
                Some(PanelResult::Cancelled) => Some(Vec::new()),
                _ => None,
            };
            if let Some(paths) = done {
                cx.close_window(window);
                if let Some((_, _, purpose)) = self.panel.take() {
                    match purpose {
                        Panel::Attach => {
                            if let Some(c) = &mut self.conv {
                                c.files.extend(paths);
                                c.focus_field = true;
                            }
                        }
                        Panel::SaveAs(chat, id) => {
                            if let Some(to) = paths.into_iter().next() {
                                self.backend.send(Cmd::SaveFile { chat, id, to });
                            }
                        }
                    }
                }
                self.redraw(cx);
            }
            return;
        }
        if let Some((w, alert)) = &self.alert
            && *w == window
        {
            let (icon, message, buttons): (&str, String, &[&str]) = match alert {
                Alert::Delete { mine, .. } => ("warning", if *mine { "Delete this message for everyone in the chat?".into() } else { "Delete this message for you?".into() }, &["Cancel", "Delete"]),
                Alert::SignOut => ("warning", "Sign out of Telegram on this computer? Your chats stay on Telegram.".into(), &["Cancel", "Sign Out"]),
            };
            if let Some(choice) = widgets::alert(ui, icon, &message, buttons) {
                cx.close_window(window);
                if let Some((_, alert)) = self.alert.take()
                    && choice == 1
                {
                    match alert {
                        Alert::Delete { chat, ids, mine } => self.backend.send(Cmd::Delete { chat, ids, everyone: mine }),
                        Alert::SignOut => self.backend.send(Cmd::SignOut),
                    }
                }
            }
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if Some(window) != self.main {
            return None;
        }
        let ready = matches!(self.status, Status::Online | Status::Updating | Status::Connecting | Status::Offline(_) | Status::Waiting(_));
        let chat = self.conv.as_ref().and_then(|c| self.store.chats.get(&c.chat));
        let file = Menu::new("File")
            .item("Contacts", "contacts", Some(Key::K))
            .enabled(ready)
            .separator()
            .item("Close", "close", Some(Key::W))
            .item("Quit", "quit", Some(Key::Q));
        let edit = Menu::new("Edit").item("Find Chat", "find", Some(Key::F)).enabled(ready);
        let mut chat_menu = Menu::new("Chat")
            .item("Attach Files…", "attach", Some(Key::O))
            .enabled(chat.is_some())
            .item("Chat Info", "info", Some(Key::I))
            .enabled(chat.is_some())
            .separator();
        if let Some(c) = chat {
            chat_menu = chat_menu
                .item(if c.pinned { "Unpin" } else { "Pin" }, "chat:pin", None)
                .item(if c.muted { "Unmute" } else { "Mute" }, "chat:mute", None)
                .item(if c.unread > 0 || c.marked { "Mark as Read" } else { "Mark as Unread" }, "chat:read", None)
                .item(if c.archived { "Unarchive" } else { "Archive" }, "chat:archive", None);
        }
        let mut view = Menu::new("View").item("All Chats", "folder:all", Some(Key::Num0)).checked(self.view.folder.is_none() && !self.view.archive);
        for (i, f) in self.store.folders.iter().enumerate().take(8) {
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8][i];
            view = view.item(&f.title, format!("folder:{}", i + 1), Some(key)).checked(self.view.folder == Some(f.id));
        }
        view = view
            .item("Archived Chats", "archive", Some(Key::Num9))
            .checked(self.view.archive)
            .separator()
            .item("Previous Chat", "prev", Some(Key::ArrowUp))
            .item("Next Chat", "next", Some(Key::ArrowDown));
        let account = Menu::new("Account")
            .item(&self.me.as_ref().map_or_else(|| "Not Signed In".into(), |m| m.name.clone()), "me", None)
            .enabled(false)
            .item("Integrations…", "integrations", None)
            .separator()
            .item("Sign Out…", "signout", None)
            .enabled(self.me.is_some());
        Some(vec![file, edit, chat_menu, view, account])
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        match command {
            "quit" => cx.quit(),
            "close" => {
                if Some(window) == self.main {
                    cx.quit();
                } else {
                    cx.close_window(window);
                }
            }
            "contacts" => self.open_contacts(cx),
            "find" => self.focus_search = true,
            "attach" => {
                let dir = olo_kit::user_dir("PICTURES").unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
                let w = cx.open_window(WindowOptions::new("Attach Files", olo_kit::file_panel::PANEL_SIZE.0, olo_kit::file_panel::PANEL_SIZE.1).dialog().parent_opt(self.main));
                self.panel = Some((w, FilePanel::open(dir).multiple(), Panel::Attach));
            }
            "info" => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.open_info(cx, chat);
                }
            }
            c if c.starts_with("chat:") => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.chat_command(cx, chat, c);
                }
            }
            "folder:all" => {
                self.view.folder = None;
                self.view.archive = false;
            }
            "archive" => {
                self.view.archive = true;
                self.view.folder = None;
            }
            c if c.starts_with("folder:") => {
                // The nth folder, as the View menu lists them.
                let n: usize = c.trim_start_matches("folder:").parse().unwrap_or(0);
                self.view.folder = n.checked_sub(1).and_then(|i| self.store.folders.get(i)).map(|f| f.id);
                self.view.archive = false;
            }
            "prev" | "next" => {
                let ids = list::shown(&self.store, &self.view);
                let at = self.conv.as_ref().and_then(|c| ids.iter().position(|&i| i == c.chat));
                let to = match (at, command) {
                    (Some(i), "prev") => i.checked_sub(1),
                    (Some(i), _) => (i + 1 < ids.len()).then_some(i + 1),
                    (None, _) => (!ids.is_empty()).then_some(0),
                };
                if let Some(i) = to {
                    self.open_chat(cx, ids[i], None);
                }
            }
            "integrations" => {
                let _ = olo_kit::spawn_detached("olo-preferences integrations");
            }
            "signout" => {
                let w = cx.open_window(WindowOptions::alert_buttons("Sign Out", "Sign out of Telegram on this computer? Your chats stay on Telegram.", &["Cancel", "Sign Out"]).parent_opt(self.main));
                self.alert = Some((w, Alert::SignOut));
            }
            _ => {}
        }
        self.redraw(cx);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if Some(window) == self.main {
            cx.quit();
        } else {
            cx.close_window(window);
            if self.forward.as_ref().is_some_and(|f| f.window == window) {
                self.forward = None;
            }
            if self.contacts.as_ref().is_some_and(|c| c.0 == window) {
                self.contacts = None;
            }
            if self.info.is_some_and(|i| i.0 == window) {
                self.info = None;
            }
            if self.panel.as_ref().is_some_and(|p| p.0 == window) {
                self.panel = None;
            }
            if self.alert.as_ref().is_some_and(|a| a.0 == window) {
                self.alert = None;
            }
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Event>() {
            Ok(e) => return self.event(cx, *e),
            Err(m) => m,
        };
        let message = match message.downcast::<pics::Decoded>() {
            Ok(d) => {
                if let Some(ctx) = &self.ctx {
                    self.pics.decoded(ctx, *d);
                }
                return self.redraw(cx);
            }
            Err(m) => m,
        };
        let message = match message.downcast::<NoteEvent>() {
            Ok(e) => {
                match *e {
                    NoteEvent::Action { id, token, .. } => {
                        if let Some(&(chat, _)) = self.note_chats.get(&id) {
                            self.open_chat(cx, chat, None);
                            if let Some(w) = self.main {
                                cx.activate_with(w, token);
                            }
                        }
                    }
                    NoteEvent::Replied { id, text } => {
                        if let Some(&(chat, msg)) = self.note_chats.get(&id) {
                            self.next_local += 1;
                            let many = self.store.chats.get(&chat).is_some_and(|c| c.kind.many());
                            self.backend.send(Cmd::Send(Outgoing { chat, text, reply_to: many.then_some(msg), local: self.next_local, ..Outgoing::default() }));
                            self.backend.send(Cmd::Read { chat, up_to: msg });
                            self.chat_notes.remove(&chat);
                        }
                    }
                    NoteEvent::Closed { id } => {
                        if let Some((chat, _)) = self.note_chats.remove(&id) {
                            self.chat_notes.retain(|c, (n, _)| !(*c == chat && *n == Some(id)));
                        }
                    }
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<instance::Launch>() {
            Ok(launch) => {
                if launch.args.iter().any(|a| a == "--reconnect") {
                    // Preferences signed in or out: start again with the account as it is.
                    self.reconnect(cx);
                } else if let Some(w) = self.main {
                    cx.activate(w);
                }
                return;
            }
            Err(m) => m,
        };
        if let Ok(h24) = message.downcast::<Clock>() {
            self.h24 = h24.0;
            self.redraw(cx);
        }
    }

    fn dropped(&mut self, cx: &mut Cx, _window: WindowId, drop: olo_kit::dnd::Drop) {
        if let Some(c) = &mut self.conv {
            c.files.extend(drop.paths());
            c.focus_field = true;
        }
        self.redraw(cx);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let s = Saved { chat: self.conv.as_ref().map(|c| c.chat), topic: self.conv.as_ref().and_then(|c| c.topic), folder: self.view.folder, list_width: Some(self.list_width) };
        serde_json::to_string(&s).ok()
    }
}

/// The Deskbar clock's style (24-hour or not).
struct Clock(bool);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        let backend = Backend::start(move |e| proxy.send(e));
        backend.send(Cmd::Chats);
        let clock = cx.proxy();
        olo_kit::settings::watch(move |s| clock.send(Clock(s.clock.twenty_four_hour)));
        let restored: Saved = cx.restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let main = cx.open_window(WindowOptions::new("Telegram", 980, 660).min_size(620, 420));
        Tg {
            backend,
            store: store::Store::default(),
            pics: pics::Pics::new(cx.proxy()),
            status: Status::Connecting,
            me: None,
            main: Some(main),
            view: list::View { folder: restored.folder, ..list::View::default() },
            found: list::Found::default(),
            conv: None,
            list_width: restored.list_width.unwrap_or(list::WIDTH),
            h24: false,
            notes: Notifier::new(cx.proxy(), "Telegram", "olo-telegram"),
            chat_notes: HashMap::new(),
            note_chats: HashMap::new(),
            panel: None,
            alert: None,
            forward: None,
            contacts: None,
            info: None,
            player: None,
            when_fetched: None,
            next_local: 0,
            focused: true,
            drafts: HashMap::new(),
            banner: None,
            searched: (String::new(), 0.0),
            asked_chats: false,
            restored,
            badge: 0,
            ctx: None,
            focus_search: false,
        }
    });
}
