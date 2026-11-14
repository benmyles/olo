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
mod stickers;
mod store;
mod voice;
mod when;

use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, ClipboardItem, CursorStyle, Cx, Element, ExternalPaths, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, LocalWindow, Menu, MenuBar,
    MouseMoveEvent, Painter, ParentElement as _, Pixels, Point, ScrollHandle, SharedString, Styled as _, View, ViewRef, WindowCx, WindowOptions, canvas, clipboard, div,
    due::{Due, Notice},
    file_panel::{self, FilePanel},
    instance::{self, Launch},
    metrics as m,
    notify::{Note, NoteEvent, NoteId, NoteKind, Notifier},
    point, px,
    search::{SearchEvent, SearchField, search_field},
    size,
    style::{self, Align, c},
    theme, white,
    widgets::{BBox, Button, CheckBox, EmptyState, ScrollView, SidePane, StatusBar, StringView, TextField, TextFieldEvent, button_row, open_alert, text_field},
};
use olo_tg::{Backend, Chat, ChatId, ChatKind, Cmd, Contact, Doing, Event, Me, Media, Message, MsgId, Outgoing, Pending, Pic, Status};
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
    /// The chat list put away (View ▸ Hide Chat List).
    #[serde(default)]
    list_hidden: bool,
}

/// What a file panel's choice is for.
type PanelDone = Box<dyn FnOnce(&mut Tg, Vec<PathBuf>, &mut Cx<Tg>)>;

/// What a file panel chose (nothing: cancelled).
struct Chosen(Vec<PathBuf>);

/// The Deskbar clock's style (24-hour or not).
struct Clock(bool);

pub struct Tg {
    backend: Backend,
    store: store::Store,
    pics: pics::Pics,
    /// Animated stickers playing.
    stickers: stickers::Stickers,
    status: Status,
    me: Option<Me>,
    view: list::View,
    found: list::Found,
    conv: Option<conv::Conv>,
    /// The chat list beside the conversation: its width, or put away.
    side: SidePane,
    h24: bool,
    notes: Notifier,
    /// Each chat's notification, updated as more come.
    chat_notes: HashMap<ChatId, (Option<NoteId>, Vec<String>)>,
    note_chats: HashMap<NoteId, (ChatId, MsgId)>,
    /// A file panel or an alert up (one at a time).
    /// What a file panel that's open does with what it's given.
    panel: Option<PanelDone>,
    alert: Option<LocalWindow<olo_kit::widgets::Alert>>,
    forward: Option<LocalWindow<Picker>>,
    contacts: Option<(LocalWindow<Picker>, Vec<Contact>)>,
    info: Option<(LocalWindow<Info>, ChatId)>,
    player: Option<(ChatId, MsgId, olo_preview::audio::Player)>,
    /// Play or open this once it's downloaded.
    when_fetched: Option<(Pic, bool)>,
    next_local: u64,
    /// A voice message being recorded, and the last one's token.
    pub voice: Option<Recording>,
    voice_token: u64,
    /// Voice messages ended before the microphone opened (sent or not),
    /// and those finishing to be sent: where to.
    voice_ended: HashMap<u64, bool>,
    voice_to: HashMap<u64, (ChatId, Option<MsgId>, Option<MsgId>)>,
    focused: bool,
    /// What's typed in each chat's field, kept when switching.
    drafts: HashMap<ChatId, String>,
    /// The messages of chats left lately, laid out: going back is instant.
    kept: Vec<(ChatId, conv::Layouts)>,
    banner: Notice,
    /// The search last sent to Telegram, and the pause before the next.
    searched: String,
    searching: Option<olo_kit::TimerId>,
    restored: Saved,
    /// The unread count Deskbar was last told.
    badge: u32,
    search: Child<SearchField>,
    pub field: Child<TextField>,
    /// The area under the pointer (lit).
    hot: Option<SharedString>,
    list_scroll: ScrollHandle,
    /// The list's rows as last built, the one under the pointer, and the
    /// rows' top as last painted (window coordinates).
    list_rows: Vec<list::Row>,
    /// Chats chosen together in the list (⌘-click, ⇧-click, Select All
    /// Chats), for what's done to them all; and the one a ⇧-click runs
    /// from.
    chosen: Vec<ChatId>,
    chosen_from: Option<ChatId>,
    list_hover: Option<usize>,
    list_top: Rc<Cell<f32>>,
    /// A redraw due (a flash going out, typing gone quiet, a voice
    /// message's progress).
    wake: Due,
    title: String,
    /// Where the window's panes are.
    list_rect: Bounds<Pixels>,
    conv_rect: Bounds<Pixels>,
    composer_rect: Bounds<Pixels>,
    /// The person's changes on their way to Telegram: (waiting, of how
    /// many, Telegram's asked wait), from `Event::Sync`.
    sync: (usize, usize, Option<u32>),
    /// Quitting, once they're sent or by then.
    quitting: Option<Instant>,
}

/// A voice message being recorded (`voice.rs`), in the chat open.
pub struct Recording {
    pub token: u64,
    chat: ChatId,
    topic: Option<MsgId>,
    pub since: Instant,
    /// Held down: let go on the button it's sent, away from it thrown
    /// away. Not held (a click started it, or Chat ▸ Record Voice
    /// Message): Send or Cancel ends it.
    pub held: bool,
    rec: Option<voice::Recorder>,
    /// The last levels (0–1), newest last, and the seconds so far.
    pub levels: VecDeque<f32>,
    pub seconds: f32,
    typing_sent: f64,
}

/// Levels the composer keeps for its bars.
pub const LEVELS: usize = 120;
/// Let go sooner than this, the press was a click: it goes on recording.
const HOLD: Duration = Duration::from_millis(300);

/// How long quitting waits for the changes still on their way (they're
/// kept and sent next time if it gives up).
const QUIT_WAIT: Duration = Duration::from_secs(30);
/// The line under the chat list while changes are on their way: its
/// label, the bar, and room round them.
const SYNC_H: f32 = 44.0;

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// How far a voice message or song has played.
pub fn played(p: &olo_preview::audio::Player) -> f32 {
    let st = &p.state;
    let pos = st.position_ms.load(std::sync::atomic::Ordering::Relaxed) as f32;
    let dur = st.duration_ms.load(std::sync::atomic::Ordering::Relaxed).max(1) as f32;
    (pos / dur).clamp(0.0, 1.0)
}

/// An area of the window the pointer can use, at `b` (window
/// coordinates), lit while it's over it.
pub fn hit(tg: &ViewRef<Tg>, key: &str, b: Bounds<Pixels>) -> Element {
    let key = SharedString::from(key.to_owned());
    let (t, k) = (*tg, key.clone());
    div().id(key).absolute().left(b.origin.x).top(b.origin.y).w(b.size.width).h(b.size.height).cursor(CursorStyle::Arrow).on_hover(move |on: &bool, cx| {
        let (k, on) = (k.clone(), *on);
        t.update(cx, move |tg, cx| {
            if on {
                tg.hot = Some(k);
            } else if tg.hot.as_ref() == Some(&k) {
                tg.hot = None;
            } else {
                return;
            }
            cx.invalidate();
        })
    })
}

/// Tells agents of `node` when any of it is in view (a scrolled list's
/// rows painted beyond its edges aren't).
pub fn seen(g: &mut Painter, node: olo_kit::agent::Node) {
    if g.content_mask().bounds.intersects(&node.bounds) {
        olo_kit::agent::record(g, node);
    }
}

/// A click with the left button (or the keyboard's): the kit says a
/// click for any button, and a right click here opens a menu instead.
pub trait LeftClick {
    fn on_left_click(self, f: impl Fn(&olo_kit::ClickEvent, &mut WindowCx) + 'static) -> Self;
}

impl LeftClick for Element {
    fn on_left_click(self, f: impl Fn(&olo_kit::ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click(move |e, cx| {
            if e.standard_click() {
                f(e, cx)
            }
        })
    }
}

/// A pane where it goes: what draws in the window's coordinates,
/// clipped to its place.
fn pane(content: Element, r: Bounds<Pixels>, window: olo_kit::Size<Pixels>) -> Element {
    div()
        .absolute()
        .left(r.origin.x)
        .top(r.origin.y)
        .w(r.size.width)
        .h(r.size.height)
        .overflow_hidden()
        .child(div().absolute().left(-r.origin.x).top(-r.origin.y).w(window.width).h(window.height).child(content))
}

fn open_url(url: String) {
    let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url));
}

fn open_path(path: PathBuf) {
    let _ = olo_xdg::open::open(&olo_xdg::open::Target::Path(path));
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

impl Tg {
    /// Redraws in `d` (or sooner, if one is due sooner already).
    /// Quit: at once, unless changes are on their way to Telegram; then
    /// once they're sent, or `QUIT_WAIT` on (they're kept on disk and go
    /// next time), showing how they go.
    fn quit(&mut self, cx: &mut Cx<Self>) {
        if self.sync.0 == 0 || self.quitting.is_some_and(|q| Instant::now() >= q) {
            return olo_kit::quit();
        }
        if self.quitting.is_none() {
            self.quitting = Some(Instant::now() + QUIT_WAIT);
            cx.after_do(QUIT_WAIT, |_, _| olo_kit::quit());
        }
        cx.invalidate();
    }

    fn wake_in(&mut self, d: Duration, cx: &mut Cx<Self>) {
        let mut wake = std::mem::take(&mut self.wake);
        wake.by(Instant::now() + d, cx);
        self.wake = wake;
    }

    /// The window's title, when it changes.
    fn set_title(&mut self, title: String, cx: &mut Cx<Self>) {
        if title == self.title {
            return;
        }
        self.title = title.clone();
        cx.set_window_title(title);
    }

    fn open_chat(&mut self, chat: ChatId, topic: Option<MsgId>, cx: &mut Cx<Self>) {
        // A voice message being recorded is for the chat it was begun in.
        if self.voice.as_ref().is_some_and(|v| v.chat != chat || v.topic != topic) {
            self.voice_end(false, cx);
        }
        // Keep what was being written in the chat we leave.
        let text = self.field.read(cx).text().to_owned();
        if let Some(mut c) = self.conv.take() {
            let old = self.store.chats.get(&c.chat).and_then(|x| x.draft.clone()).unwrap_or_default();
            if text.trim() != old.trim() {
                self.backend.send(Cmd::Draft { chat: c.chat, text: text.clone() });
            }
            self.drafts.insert(c.chat, text);
            self.kept.retain(|k| k.0 != c.chat);
            self.kept.push((c.chat, std::mem::take(&mut c.layouts)));
            if self.kept.len() > 12 {
                self.kept.remove(0);
            }
        }
        let text = self.drafts.remove(&chat).or_else(|| self.store.chats.get(&chat).and_then(|c| c.draft.clone())).unwrap_or_default();
        self.field.update(cx, move |f, cx| {
            f.set_text(text, cx);
            f.caret_to_end(cx);
        });
        let mut c = conv::Conv::new(chat, topic);
        if let Some(i) = self.kept.iter().position(|k| k.0 == chat) {
            c.layouts = self.kept.remove(i).1;
        }
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
        let title = self.store.chats.get(&chat).map_or_else(|| "Telegram".into(), |c| c.title.clone());
        self.set_title(title, cx);
        // Its notification has been seen to.
        if let Some((Some(id), _)) = self.chat_notes.remove(&chat) {
            self.notes.close(id);
        }
        cx.invalidate();
    }

    fn send(&mut self, cx: &mut Cx<Self>) {
        let text = self.field.read(cx).text().trim_end().to_owned();
        let Some(c) = &mut self.conv else { return };
        if let Some(id) = c.edit.take() {
            self.backend.send(Cmd::Edit { chat: c.chat, id, text, entities: Vec::new() });
            self.field.update(cx, move |f, cx| f.set_text("", cx));
            cx.invalidate();
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
        c.at_bottom = true;
        self.drafts.remove(&c.chat);
        self.store.pending(pending);
        self.field.update(cx, move |f, cx| f.set_text("", cx));
        cx.invalidate();
    }

    /// After `wait`, the frame due for an animated sticker (`stickers.rs`).
    pub fn sticker_timer(&mut self, (key, wait): (stickers::Key, Duration), cx: &mut Cx<Self>) {
        cx.after_do(wait, move |tg, cx| {
            tg.stickers.ask(key, Instant::now());
            for due in tg.stickers.take_due() {
                tg.sticker_timer(due, cx);
            }
        });
    }

    /// Redraw the composer alone (a voice message's level, ten a second).
    fn redraw_composer(&self, cx: &mut Cx<Self>) {
        cx.invalidate();
    }

    /// Start recording a voice message in the chat open: `held` while
    /// the button is held down, else until Send or Cancel.
    pub fn voice_start(&mut self, held: bool, cx: &mut Cx<Self>) {
        let Some(c) = &self.conv else { return };
        if self.voice.is_some() || c.edit.is_some() {
            return;
        }
        self.voice_token += 1;
        let token = self.voice_token;
        // (What's playing would be recorded too.)
        self.player = None;
        self.backend.send(Cmd::Typing { chat: c.chat, topic: c.topic, doing: Doing::RecordingVoice });
        self.voice = Some(Recording { token, chat: c.chat, topic: c.topic, since: Instant::now(), held, rec: None, levels: VecDeque::new(), seconds: 0.0, typing_sent: now() });
        // The field keeps the keyboard (pressing the button took it), for
        // Escape.
        if let Some(c) = &mut self.conv {
            c.focus_field = true;
        }
        voice::start(token, olo_kit::proxy());
        cx.invalidate();
    }

    /// The button let go, over it or not.
    pub fn voice_release(&mut self, over: bool, cx: &mut Cx<Self>) {
        let Some(v) = &mut self.voice else { return };
        if !v.held {
            return;
        }
        if v.since.elapsed() < HOLD {
            // A click: it goes on till Send or Cancel.
            v.held = false;
            return self.redraw_composer(cx);
        }
        self.voice_end(over, cx);
    }

    /// End the recording: sent, or thrown away.
    pub fn voice_end(&mut self, send: bool, cx: &mut Cx<Self>) {
        let Some(mut v) = self.voice.take() else { return };
        if let Some(c) = &mut self.conv {
            c.focus_field = true;
        }
        if send {
            let reply = self.conv.as_mut().filter(|c| c.chat == v.chat).and_then(|c| c.reply.take());
            self.voice_to.insert(v.token, (v.chat, v.topic, reply));
        } else {
            self.backend.send(Cmd::Typing { chat: v.chat, topic: v.topic, doing: Doing::Stopped });
        }
        match v.rec.take() {
            Some(rec) if send => rec.finish(v.token, olo_kit::proxy()),
            Some(rec) => rec.cancel(),
            // Still opening the microphone: done once it's open.
            None => _ = self.voice_ended.insert(v.token, send),
        }
        cx.invalidate();
    }

    fn voice_msg(&mut self, msg: voice::Msg, cx: &mut Cx<Self>) {
        match msg {
            voice::Msg::Started(token, Ok(rec)) => {
                if let Some(v) = self.voice.as_mut().filter(|v| v.token == token) {
                    v.rec = Some(rec);
                } else if let Some(send) = self.voice_ended.remove(&token) {
                    if send { rec.finish(token, olo_kit::proxy()) } else { rec.cancel() }
                } else {
                    rec.cancel();
                }
            }
            voice::Msg::Started(token, Err(why)) => {
                self.voice_ended.remove(&token);
                self.voice_to.remove(&token);
                if let Some(v) = self.voice.take_if(|v| v.token == token) {
                    self.backend.send(Cmd::Typing { chat: v.chat, topic: v.topic, doing: Doing::Stopped });
                    self.banner.say(format!("The microphone can’t be recorded ({why})."), Duration::from_secs(8));
                    cx.invalidate();
                }
            }
            voice::Msg::Level(token, level, seconds) => {
                let Some(v) = self.voice.as_mut().filter(|v| v.token == token) else { return };
                v.levels.push_back(level);
                while v.levels.len() > LEVELS {
                    v.levels.pop_front();
                }
                v.seconds = seconds;
                // Telegram's "recording a voice message", kept up.
                if now() - v.typing_sent > 5.0 {
                    v.typing_sent = now();
                    self.backend.send(Cmd::Typing { chat: v.chat, topic: v.topic, doing: Doing::RecordingVoice });
                }
                if seconds >= voice::LONGEST {
                    return self.voice_end(true, cx);
                }
                self.redraw_composer(cx);
            }
            voice::Msg::Done(token, result) => {
                let Some((chat, topic, reply)) = self.voice_to.remove(&token) else { return };
                let note = match result {
                    Ok(note) if note.seconds >= voice::SHORTEST => note,
                    Ok(note) => {
                        let _ = std::fs::remove_file(&note.path);
                        self.backend.send(Cmd::Typing { chat, topic, doing: Doing::Stopped });
                        self.banner.say("Hold the microphone button down while you speak, or click it to record until you click Send.".to_owned(), Duration::from_secs(6));
                        return cx.invalidate();
                    }
                    Err(why) => {
                        self.banner.say(format!("The voice message couldn’t be recorded ({why})."), Duration::from_secs(8));
                        return cx.invalidate();
                    }
                };
                self.next_local += 1;
                let local = self.next_local;
                let size = std::fs::metadata(&note.path).map_or(0, |m| m.len());
                let reply_to = reply.and_then(|r| {
                    let m = self.store.threads.get(&(chat, None))?.msgs.get(&r)?;
                    Some(olo_tg::model::ReplyTo { id: r, from: m.from.as_ref().map(|f| f.name.clone()), text: Some(m.summary()) })
                });
                let pending = Message {
                    id: i32::MAX,
                    chat,
                    from: self.me.as_ref().map(|m| olo_tg::model::Sender { id: m.id, name: "You".into(), photo: None }),
                    date: when::now(),
                    out: true,
                    reply_to,
                    media: Some(Media::Voice { duration: note.seconds, waveform: note.waveform.clone(), size }),
                    topic,
                    pending: Some((local, Pending::Sending)),
                    ..Message::default()
                };
                let voice = Some((note.path, note.seconds, note.waveform));
                self.backend.send(Cmd::Send(Outgoing { chat, topic, reply_to: reply, voice, local, ..Outgoing::default() }));
                if let Some(c) = self.conv.as_mut().filter(|c| c.chat == chat) {
                    c.at_bottom = true;
                }
                self.store.pending(pending);
                cx.invalidate();
            }
        }
    }

    fn notify_new(&mut self, m: &Message) {
        let Some(chat) = self.store.chats.get(&m.chat) else { return };
        // (Unknown senders are almost always spam: they wait, unannounced.)
        if m.out || chat.muted || chat.unknown || m.service.is_some() {
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
            .kind(NoteKind::Important)
            .action("default", "Show")
            .reply(format!("Reply to {}", chat.title));
        if chat.kind.many() {
            note = note.subtitle(from);
        }
        let shown = self.notes.show(*id, note);
        *id = Some(shown);
        self.note_chats.insert(shown, (m.chat, m.id));
    }

    fn event(&mut self, e: Event, cx: &mut Cx<Self>) {
        match e {
            Event::Status(s) => self.status = s,
            Event::Me(m) => self.me = Some(m),
            Event::Chats { chats, done, cached } => {
                for c in chats {
                    if !cached {
                        self.store.live.insert(c.id);
                    }
                    self.store.put_chat(c);
                }
                if done && !cached {
                    self.store.whole();
                }
                self.store.sort();
                // The chat open before a reload, once it's known.
                if self.conv.is_none()
                    && let Some(chat) = self.restored.chat.take()
                {
                    if self.store.chats.contains_key(&chat) {
                        let topic = self.restored.topic.take();
                        self.open_chat(chat, topic, cx);
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
                let retitle = self.conv.as_ref().is_some_and(|cv| cv.chat == c.id).then(|| c.title.clone());
                // (Live from Telegram: kept when the list is whole.)
                self.store.live.insert(c.id);
                self.store.put_chat(c);
                self.store.sort();
                if let Some(t) = retitle {
                    self.set_title(t, cx);
                }
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
                self.notify_new(&m);
                self.store.message(m);
            }
            Event::Sent { local, msg } => self.store.sent(local, Some(msg)),
            Event::SendFailed { local, why, .. } => {
                self.store.sent(local, None);
                self.banner.say(why, Duration::from_secs(8));
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
                        open_path(path);
                    }
                }
            }
            Event::Progress { pic: Some(pic), done, .. } => {
                self.pics.progress.insert(pic, done);
            }
            Event::Progress { .. } => {}
            Event::FetchFailed { pic, why } => {
                if matches!(pic, Pic::File(..)) {
                    self.banner.say(why, Duration::from_secs(8));
                }
                self.pics.fetch_failed(pic);
            }
            Event::Saved { to } => {
                self.notes.show(None, Note::new("Saved").body(to.display().to_string()).accent(ACCENT));
            }
            Event::Results { query, chats, msgs } => self.found = list::Found { query, chats, msgs },
            Event::Contacts(list) => {
                if let Some((_, l)) = &mut self.contacts {
                    *l = list;
                }
                self.feed(cx);
            }
            Event::Resolved { username, chat } => match chat {
                Some(c) => {
                    let id = c.id;
                    if !self.store.chats.contains_key(&id) {
                        self.store.put_chat(c);
                        self.store.sort();
                    }
                    self.open_chat(id, None, cx);
                }
                None => self.banner.say(format!("No one on Telegram is called {username}."), Duration::from_secs(6)),
            },
            Event::Error(e) => self.banner.say(e, Duration::from_secs(8)),
            Event::Sync { pending, total, waiting } => {
                self.sync = (pending, total, waiting);
                if pending == 0 && self.quitting.is_some() {
                    olo_kit::quit();
                }
            }
        }
        let unread = self.store.total_unread();
        if unread != self.badge {
            self.badge = unread;
            self.notes.badge(unread);
        }
        cx.invalidate();
    }

    /// Talk to the account afresh (the old backend ends once dropped).
    fn reconnect(&mut self, cx: &mut Cx<Self>) {
        let proxy = olo_kit::proxy();
        self.backend = Backend::start(move |e| proxy.send(e));
        self.backend.send(Cmd::Chats);
        self.store = store::Store::default();
        self.conv = None;
        self.kept.clear();
        self.me = None;
        self.status = Status::Connecting;
        self.pics = pics::Pics::new(olo_kit::proxy());
        cx.invalidate();
    }

    fn play(&mut self, pic: &Pic, path: &std::path::Path) {
        let Pic::File(chat, id) = pic else { return };
        self.player = Some((*chat, *id, olo_preview::audio::Player::start(&olo_preview::input::Input::file(path))));
    }

    fn hit(&mut self, id: MsgId, hit: bubble::Hit, cx: &mut Cx<Self>) {
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
                    open_url(url);
                }
            }
            bubble::Hit::Mention(name) => self.backend.send(Cmd::Resolve { username: name }),
            bubble::Hit::MentionId(user) | bubble::Hit::Sender(user) => {
                if self.store.chats.contains_key(&user) {
                    self.open_chat(user, None, cx);
                }
            }
            bubble::Hit::Hashtag(tag) => {
                {
                    let tag = tag.clone();
                    self.search.update(cx, |s, cx| s.set_text(tag, cx));
                }
                self.view.query = tag.clone();
                self.searched = tag.clone();
                self.backend.send(Cmd::Search { query: tag });
            }
            bubble::Hit::Spoiler => {
                c.revealed.insert(id);
            }
            bubble::Hit::Media => self.open_media(chat, id),
        }
        cx.invalidate();
    }

    /// A message's picture, file or voice: open or play it.
    fn open_media(&mut self, chat: ChatId, id: MsgId) {
        let Some(m) = self.store.threads.get(&(chat, None)).and_then(|t| t.msgs.get(&id)).or_else(|| self.conv.as_ref().and_then(|c| self.store.threads.get(&(chat, c.topic))?.msgs.get(&id))) else {
            return;
        };
        let pic = Pic::File(chat, id);
        let play = matches!(m.media, Some(Media::Voice { .. } | Media::Audio { .. }));
        match &m.media {
            Some(Media::Location { lat, lon, .. }) => return open_url(format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}")),
            Some(Media::WebPage { url, .. }) => return open_url(url.clone()),
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
            Some(path) => open_path(path),
            None => {
                self.when_fetched = Some((pic.clone(), play));
                self.pics.fetch_file(pic, &self.backend);
            }
        }
    }

    fn message_command(&mut self, id: MsgId, cmd: &str, cx: &mut Cx<Self>) {
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
                c.focus_field = true;
                self.field.update(cx, move |f, cx| {
                    f.set_text(m.text.clone(), cx);
                    f.caret_to_end(cx);
                });
            }
            "copy" => clipboard::write(ClipboardItem::new_string(m.text.clone())),
            "forward" => self.open_forward(chat, vec![id], cx),
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
                let dir = olo_xdg::user_dir("DOWNLOAD").unwrap_or_else(home);
                self.open_panel(
                    "Save As",
                    FilePanel::save(dir, name),
                    move |tg, paths, _| {
                        if let Some(to) = paths.into_iter().next() {
                            tg.backend.send(Cmd::SaveFile { chat, id, to });
                        }
                    },
                    cx,
                );
            }
            "delete" => {
                let mine = m.out;
                let everyone = mine && self.store.chats.get(&chat).is_some_and(|c| c.kind != ChatKind::Channel);
                let text = if everyone { "Delete this message for everyone in the chat?" } else { "Delete this message for you?" };
                self.ask("Delete Message", text, "Delete", vec![Cmd::Delete { chat, ids: vec![id], everyone: mine }], cx);
            }
            c2 if c2.starts_with("react:") => {
                let e = c2.trim_start_matches("react:").to_owned();
                let mine = m.reactions.iter().any(|r| r.mine && r.emoji == e);
                self.backend.send(Cmd::React { chat, id, emoji: if mine { None } else { Some(e) } });
            }
            _ => {}
        }
        cx.invalidate();
    }

    /// The chats chosen together that are still there, in the list's
    /// order.
    fn chosen_chats(&self) -> Vec<Chat> {
        self.store.order.iter().filter(|id| self.chosen.contains(id)).filter_map(|id| self.store.chats.get(id).cloned()).collect()
    }

    /// `cmd` (`chats:…`) done to every chat chosen together: archived or
    /// not, read, muted or not, deleted (asked first).
    fn chats_command(&mut self, cmd: &str, cx: &mut Cx<Self>) {
        let chats = self.chosen_chats();
        if self.chats_do(chats, cmd, cx) {
            self.chosen.clear();
            cx.invalidate();
        }
    }

    /// The chats of `group` (all of them, as its view shows them).
    pub fn group_chats(&self, group: list::Group) -> Vec<Chat> {
        list::shown(&self.store, &group.view()).iter().filter_map(|id| self.store.chats.get(id).cloned()).collect()
    }

    /// `group`'s menu (a right-click on its row or chip): what's done to
    /// all its chats, titled `title` (a submenu's), its commands after
    /// `prefix`.
    pub fn group_menu(&self, group: list::Group, title: &str, prefix: &str) -> Menu {
        let chats = self.group_chats(group);
        let mut menu = Menu::new(title).item("Mark All as Read", format!("{prefix}chats:read")).enabled(chats.iter().any(|c| c.unread > 0 || c.marked));
        if chats.iter().any(|c| !c.muted) {
            menu = menu.item("Mute All", format!("{prefix}chats:mute"));
        }
        if chats.iter().any(|c| c.muted) {
            menu = menu.item("Unmute All", format!("{prefix}chats:unmute"));
        }
        match group {
            list::Group::Archive => menu.separator().item("Unarchive All…", format!("{prefix}group:unarchive")).enabled(!chats.is_empty()),
            list::Group::Unknown => menu.separator().item("Archive All", format!("{prefix}chats:archive")).enabled(!chats.is_empty()),
            list::Group::Folder(_) => menu,
        }
    }

    /// `cmd` from `group`'s menu, done to all its chats (Unarchive All
    /// asks first, saying how many).
    pub fn group_command(&mut self, group: list::Group, cmd: &str, cx: &mut Cx<Self>) {
        let chats = self.group_chats(group);
        if cmd == "group:unarchive" {
            let n = chats.len();
            let message = match n {
                1 => "Move the archived chat back to your chat list?".to_owned(),
                2 => "Move both archived chats back to your chat list?".to_owned(),
                n => format!("Move all {n} archived chats back to your chat list?"),
            };
            let cmds = chats.iter().map(|c| Cmd::Archive { chat: c.id, on: false }).collect();
            self.ask("Unarchive All Chats", message, "Unarchive", cmds, cx);
            return;
        }
        eprintln!("telegram: {cmd} on {group:?}: {} chats", chats.len());
        self.chats_do(chats, cmd, cx);
        cx.invalidate();
    }

    /// `cmd` (`chats:…`) done to `chats`: false if it wasn't (none, or
    /// it asks first).
    fn chats_do(&mut self, chats: Vec<Chat>, cmd: &str, cx: &mut Cx<Self>) -> bool {
        if chats.is_empty() {
            return false;
        }
        let send = |tg: &mut Tg, f: &dyn Fn(&Chat) -> Option<Cmd>| {
            for c in &chats {
                if let Some(cmd) = f(c) {
                    tg.backend.send(cmd);
                }
            }
        };
        match cmd {
            "chats:archive" => send(self, &|c| (!c.archived).then_some(Cmd::Archive { chat: c.id, on: true })),
            "chats:unarchive" => send(self, &|c| c.archived.then_some(Cmd::Archive { chat: c.id, on: false })),
            "chats:read" => send(self, &|c| (c.unread > 0 || c.marked).then(|| Cmd::Read { chat: c.id, up_to: c.last.as_ref().map_or(0, |l| l.id) })),
            "chats:mute" => send(self, &|c| (!c.muted).then_some(Cmd::Mute { chat: c.id, on: true })),
            "chats:unmute" => send(self, &|c| c.muted.then_some(Cmd::Mute { chat: c.id, on: false })),
            "chats:delete" => {
                let n = chats.len();
                let leaves = chats.iter().any(|c| matches!(c.kind, ChatKind::Group | ChatKind::Forum | ChatKind::Channel));
                let message = format!("Delete {n} chats? Their messages go from this account{}. This can’t be undone.", if leaves { ", and you leave the groups and channels among them" } else { "" });
                let cmds = chats.iter().map(|c| Cmd::DeleteChat { chat: c.id }).collect();
                self.ask("Delete Chats", message, "Delete", cmds, cx);
                return false;
            }
            _ => return false,
        }
        true
    }

    /// The menu for the chats chosen together (the list's, the Chat
    /// menu's).
    fn chats_menu(&self, mut menu: Menu) -> Menu {
        let chats = self.chosen_chats();
        let n = chats.len();
        if chats.iter().any(|c| !c.archived) {
            menu = menu.item(format!("Archive {n} Chats"), "chats:archive");
        }
        if chats.iter().any(|c| c.archived) {
            menu = menu.item(format!("Unarchive {n} Chats"), "chats:unarchive");
        }
        menu = menu.item("Mark as Read", "chats:read").enabled(chats.iter().any(|c| c.unread > 0 || c.marked));
        if chats.iter().any(|c| !c.muted) {
            menu = menu.item("Mute", "chats:mute");
        }
        if chats.iter().any(|c| c.muted) {
            menu = menu.item("Unmute", "chats:unmute");
        }
        menu.separator().item(format!("Delete {n} Chats…"), "chats:delete")
    }

    fn chat_command(&mut self, chat: ChatId, cmd: &str, cx: &mut Cx<Self>) {
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
        cx.invalidate();
    }

    /// An alert asking before `cmd` goes (deleting, signing out).
    fn ask(&mut self, title: &'static str, message: impl Into<String>, yes: &'static str, cmds: Vec<Cmd>, cx: &mut Cx<Self>) {
        let message: String = message.into();
        if let Some(w) = self.alert.take() {
            w.close(cx);
        }
        let me = cx.view_ref();
        let cmds = std::cell::RefCell::new(cmds);
        self.alert = open_alert(cx, title, "warning", &message, &["Cancel", yes], move |choice, cx| {
            let (yes, cmds) = (*choice == Some(1), cmds.take());
            me.update(cx, move |tg, _| {
                tg.alert = None;
                if yes {
                    for cmd in cmds {
                        tg.backend.send(cmd);
                    }
                }
            })
        });
    }

    /// A file panel; `done` gets what was chosen (nothing if cancelled).
    fn open_panel(&mut self, title: &'static str, panel: FilePanel, done: impl FnOnce(&mut Tg, Vec<PathBuf>, &mut Cx<Tg>) + 'static, cx: &mut Cx<Self>) {
        // (One at a time: the one open is its own window, brought
        // forward by its own tab.)
        if self.panel.is_some() {
            return;
        }
        self.panel = Some(Box::new(done));
        let me = cx.messenger();
        file_panel::show(title, panel, move |a| {
            let paths = match a.result {
                file_panel::PanelResult::Chosen(p) => vec![p],
                file_panel::PanelResult::Several(ps) => ps,
                file_panel::PanelResult::Cancelled => Vec::new(),
            };
            me.send(Box::new(Chosen(paths)));
        });
    }

    fn attach(&mut self, cx: &mut Cx<Self>) {
        if self.conv.is_none() {
            return;
        }
        let dir = olo_xdg::user_dir("PICTURES").unwrap_or_else(home);
        self.open_panel(
            "Attach Files",
            FilePanel::open(dir).multiple(),
            |tg, paths, _| {
                if let Some(c) = &mut tg.conv
                    && !paths.is_empty()
                {
                    c.files.extend(paths);
                    c.focus_field = true;
                }
            },
            cx,
        );
    }

    fn open_forward(&mut self, from: ChatId, ids: Vec<MsgId>, cx: &mut Cx<Self>) {
        if let Some(w) = self.forward.take() {
            w.close(cx);
        }
        self.forward = Picker::open(cx, Pick::Forward { from, ids });
        self.feed(cx);
    }

    fn open_contacts(&mut self, cx: &mut Cx<Self>) {
        if let Some((w, _)) = self.contacts.as_ref().filter(|(w, _)| w.is_open(cx)) {
            w.update(cx, |_, cx| cx.activate_window());
            return;
        }
        self.backend.send(Cmd::Contacts);
        self.contacts = Picker::open(cx, Pick::Contacts).map(|w| (w, Vec::new()));
        self.feed(cx);
    }

    fn open_info(&mut self, chat: ChatId, cx: &mut Cx<Self>) {
        if let Some((w, _)) = self.info.take() {
            w.close(cx);
        }
        let title = self.store.chats.get(&chat).map_or_else(|| "Info".into(), |c| c.title.clone());
        let main = cx.messenger();
        let window = olo_kit::open_window_here(cx, WindowOptions::new(title, 300.0, 420.0).min_size(300.0, 360.0).child(), move |cx| {
            let focus = FocusHandle::new();
            focus.focus(cx);
            Info { main, chat, ch: None, pic: None, focus }
        });
        self.info = window.map(|w| (w, chat));
        self.feed(cx);
    }

    /// The windows that show what this one knows (Forward To, Contacts,
    /// a chat's Info) told it again: as they open, and as the account
    /// or a picture changes.
    fn feed(&mut self, cx: &mut Cx<Self>) {
        if let Some(w) = self.forward.as_ref().filter(|w| w.is_open(cx)) {
            let rows: Vec<PickRow> = self
                .store
                .order
                .iter()
                .filter_map(|id| self.store.chats.get(id))
                .map(|c| PickRow { id: c.id, name: c.title.clone(), detail: String::new(), also: None, pic: if c.photo.is_some() { self.pics.get(&Pic::Avatar(c.id), &self.backend) } else { None } })
                .collect();
            w.update(cx, move |p, cx| p.fed(rows, false, cx));
        }
        if let Some((w, list)) = self.contacts.as_ref().filter(|(w, _)| w.is_open(cx)) {
            let rows: Vec<PickRow> = list
                .iter()
                .map(|c| PickRow {
                    id: c.id,
                    name: c.name.clone(),
                    detail: c.seen.as_ref().map(|s| s.describe(when::now())).or_else(|| c.phone.clone()).unwrap_or_default(),
                    also: c.username.clone(),
                    pic: if c.photo.is_some() { self.pics.get(&Pic::Avatar(c.id), &self.backend) } else { None },
                })
                .collect();
            let loading = list.is_empty();
            w.update(cx, move |p, cx| p.fed(rows, loading, cx));
        }
        if let Some((w, chat)) = self.info.as_ref().filter(|(w, _)| w.is_open(cx)) {
            let ch = self.store.chats.get(chat).cloned();
            let pic = ch.as_ref().filter(|c| c.photo.is_some()).and_then(|_| self.pics.get(&Pic::Avatar(*chat), &self.backend));
            w.update(cx, move |i, cx| {
                i.ch = ch;
                i.pic = pic;
                cx.invalidate();
            });
        }
    }

    /// What a picker's row or an Info window's setting asks.
    fn picked(&mut self, picked: Picked, cx: &mut Cx<Self>) {
        match picked {
            Picked::Forward { from, ids, to } => {
                self.backend.send(Cmd::Forward { from, ids, to });
                self.open_chat(to, None, cx);
                self.forward = None;
            }
            Picked::Contact(id) => {
                if !self.store.chats.contains_key(&id)
                    && let Some(c) = self.contacts.as_ref().and_then(|(_, l)| l.iter().find(|c| c.id == id)).cloned()
                {
                    self.store.put_chat(Chat {
                        id: c.id,
                        kind: ChatKind::User,
                        title: c.name.clone(),
                        username: c.username.clone(),
                        photo: c.photo,
                        contact: true,
                        seen: c.seen.clone(),
                        ..Chat::default()
                    });
                    self.store.sort();
                }
                self.open_chat(id, None, cx);
                cx.activate_window();
            }
            Picked::Chat(chat, cmd) => self.chat_command(chat, cmd, cx),
        }
    }

    pub fn list_act(&mut self, a: list::Act, cx: &mut Cx<Self>) {
        match a {
            list::Act::Open(id) => self.open_chat(id, None, cx),
            list::Act::OpenAt(chat, id) => {
                self.open_chat(chat, None, cx);
                if self.store.threads.get(&(chat, None)).is_some_and(|t| t.msgs.contains_key(&id)) {
                    if let Some(c) = &mut self.conv {
                        c.jump = Some(id);
                    }
                } else {
                    self.backend.send(Cmd::Around { chat, id });
                }
            }
            list::Act::Command(id, cmd) => self.chat_command(id, &cmd, cx),
        }
    }

    pub fn conv_act(&mut self, a: conv::Act, cx: &mut Cx<Self>) {
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
            conv::Act::Message(id, cmd) => self.message_command(id, &cmd, cx),
            conv::Act::Hit(id, h) => self.hit(id, h, cx),
            conv::Act::Attach => self.attach(cx),
            conv::Act::Info => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.open_info(chat, cx);
                }
            }
            conv::Act::Topic(t) => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.open_chat(chat, t, cx);
                }
            }
            conv::Act::CancelReply => {
                if let Some(c) = &mut self.conv {
                    if c.edit.take().is_some() {
                        self.field.update(cx, move |f, cx| f.set_text("", cx));
                    }
                    c.reply = None;
                }
                cx.invalidate();
            }
            conv::Act::Unstage(i) => {
                if let Some(c) = &mut self.conv
                    && i < c.files.len()
                {
                    c.files.remove(i);
                }
                cx.invalidate();
            }
            conv::Act::Bottom => {
                if let Some(c) = &mut self.conv {
                    c.at_bottom = true;
                }
                cx.invalidate();
            }
        }
    }

    /// The field's text changed: tell the chat we're typing (every five
    /// seconds at most).
    fn typed(&mut self, cx: &mut Cx<Self>) {
        let empty = self.field.read(cx).text().is_empty();
        if let Some(c) = &mut self.conv
            && !empty
        {
            let t = now();
            if t - c.typing_sent > 5.0 {
                c.typing_sent = t;
                self.backend.send(Cmd::Typing { chat: c.chat, topic: c.topic, doing: Doing::Typing });
            }
        }
        // (The composer says what it shows again: its button sends once
        // there's something to, and it's as tall as its lines.)
        cx.invalidate();
    }

    /// The search changed: Telegram is asked after a pause in typing.
    fn search_changed(&mut self, cx: &mut Cx<Self>) {
        self.view.query = self.search.read(cx).text(cx).to_owned();
        if let Some(timer) = self.searching.take() {
            cx.cancel(timer);
        }
        let q = self.view.query.trim().to_owned();
        if !q.is_empty() && q != self.found.query && q != self.searched {
            self.searching = Some(cx.after_do(Duration::from_millis(400), move |tg, _| {
                if tg.view.query.trim() == q {
                    tg.searched = q.clone();
                    tg.backend.send(Cmd::Search { query: q });
                }
            }));
        }
        cx.invalidate();
    }

    fn menus(&self) -> Vec<Menu> {
        let ready = matches!(self.status, Status::Online | Status::Updating | Status::Connecting | Status::Offline(_) | Status::Waiting(_));
        let chat = self.conv.as_ref().and_then(|c| self.store.chats.get(&c.chat));
        // (Its settings: signing in, in Integrations, and its notifications.)
        let file = Menu::new("File").app_items(&["integrations", "notifications"]).item("Contacts", "contacts").key("k").enabled(ready).close_quit();
        let edit = Menu::new("Edit")
            .item("Find…", "find")
            .key("f")
            .enabled(ready)
            .item("Select All Chats", "select-all-chats")
            .enabled(ready)
            .item("Deselect All Chats", "deselect-chats")
            .enabled(!self.chosen.is_empty());
        let mut chat_menu = Menu::new("Chat").item("Attach Files…", "attach").key("o").enabled(chat.is_some());
        chat_menu = match &self.voice {
            None => chat_menu.item("Record Voice Message", "record-voice").enabled(chat.is_some() && self.conv.as_ref().is_some_and(|c| c.edit.is_none())),
            Some(_) => chat_menu.item("Send Voice Message", "send-voice").item("Cancel Recording", "cancel-voice"),
        };
        chat_menu = chat_menu.item("Chat Info", "info").key("i").enabled(chat.is_some()).separator();
        if self.chosen.len() > 1 {
            chat_menu = self.chats_menu(chat_menu);
        } else if let Some(c) = chat {
            chat_menu = chat_menu
                .item(if c.pinned { "Unpin" } else { "Pin" }, "chat:pin")
                .item(if c.muted { "Unmute" } else { "Mute" }, "chat:mute")
                .item(if c.unread > 0 || c.marked { "Mark as Read" } else { "Mark as Unread" }, "chat:read")
                .item(if c.archived { "Unarchive" } else { "Archive" }, "chat:archive");
        }
        // Every chat the list shows (its folder, the archive, unknown
        // senders), as its right-click menu's.
        let unread = ready && list::any_unread(&self.store, list::Group::of(&self.view));
        chat_menu = chat_menu.separator().item("Mark All as Read", "mark-all-read").enabled(unread);
        let mut view = Menu::new("View").item("All Chats", "folder:all").key("0").checked(self.view.folder.is_none() && !self.view.archive);
        for (i, f) in self.store.folders.iter().enumerate().take(8) {
            view = view.item(f.title.clone(), format!("folder:{}", i + 1)).key(&(i + 1).to_string()).checked(self.view.folder == Some(f.id));
        }
        view = view
            .item("Archived Chats", "archive")
            .key("9")
            .checked(self.view.archive)
            .item("Unknown Senders", "unknown")
            .checked(self.view.unknown)
            .separator()
            .item("Previous Chat", "prev")
            .key("up")
            .item("Next Chat", "next")
            .key("down")
            .separator()
            .item(if self.side.collapsed() { "Show Chat List" } else { "Hide Chat List" }, "sidebar")
            // (Every app's keys for it: `app.sidebar`, ⌥⌘S.)
            .key("s");
        let account = Menu::new("Account")
            .item(self.me.as_ref().map_or_else(|| "Not Signed In".into(), |m| m.name.clone()), "me")
            .enabled(false)
            .separator()
            .item("Sign Out…", "signout")
            .enabled(self.me.is_some());
        vec![file, edit, chat_menu, view, account]
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
        match command {
            "quit" | "close" => self.quit(cx),
            "quit-now" => olo_kit::quit(),
            "contacts" => self.open_contacts(cx),
            "find" => self.search.update(cx, |s, cx| s.focus(cx)),
            "attach" => self.attach(cx),
            "record-voice" => self.voice_start(false, cx),
            "send-voice" => self.voice_end(true, cx),
            "cancel-voice" => self.voice_end(false, cx),
            "info" => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.open_info(chat, cx);
                }
            }
            c if c.starts_with("chats:") => self.chats_command(c, cx),
            "mark-all-read" => self.group_command(list::Group::of(&self.view), "chats:read", cx),
            "select-all-chats" => {
                self.chosen = list::shown(&self.store, &self.view);
                self.chosen_from = self.chosen.first().copied();
            }
            "deselect-chats" => self.chosen.clear(),
            c if c.starts_with("chat:") => {
                if let Some(chat) = self.conv.as_ref().map(|c| c.chat) {
                    self.chat_command(chat, c, cx);
                }
            }
            "folder:all" => {
                self.view.folder = None;
                self.view.archive = false;
                self.view.unknown = false;
            }
            "sidebar" => {
                self.side.toggle();
                cx.invalidate();
            }
            "archive" => {
                self.view.archive = true;
                self.view.unknown = false;
                self.view.folder = None;
            }
            "unknown" => {
                self.view.unknown = true;
                self.view.archive = false;
                self.view.folder = None;
            }
            c if c.starts_with("folder:") => {
                // The nth folder, as the View menu lists them.
                let n: usize = c.trim_start_matches("folder:").parse().unwrap_or(0);
                self.view.folder = n.checked_sub(1).and_then(|i| self.store.folders.get(i)).map(|f| f.id);
                self.view.archive = false;
                self.view.unknown = false;
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
                    self.open_chat(ids[i], None, cx);
                }
            }
            "signout" => self.ask("Sign Out", "Sign out of Telegram on this computer? Your chats stay on Telegram.", "Sign Out", vec![Cmd::SignOut], cx),
            _ => {}
        }
        cx.invalidate();
    }

    fn told(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Cx<Self>) {
        let message = match message.downcast::<Event>() {
            Ok(e) => return self.event(*e, cx),
            Err(m) => m,
        };
        let message = match message.downcast::<stickers::Msg>() {
            Ok(m) => {
                let (redraw, next) = self.stickers.message(*m);
                if let Some(next) = next {
                    self.sticker_timer(next, cx);
                }
                if redraw {
                    cx.invalidate();
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<voice::Msg>() {
            Ok(m) => return self.voice_msg(*m, cx),
            Err(m) => m,
        };
        let message = match message.downcast::<pics::Decoded>() {
            Ok(d) => {
                self.pics.decoded(*d);
                self.feed(cx);
                return cx.invalidate();
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Chosen>() {
            Ok(chosen) => {
                if let Some(done) = self.panel.take() {
                    done(self, chosen.0, cx);
                    cx.invalidate();
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Picked>() {
            Ok(picked) => return self.picked(*picked, cx),
            Err(m) => m,
        };
        let message = match message.downcast::<NoteEvent>() {
            Ok(e) => {
                match *e {
                    NoteEvent::Action { id, token, .. } => {
                        if let Some(&(chat, _)) = self.note_chats.get(&id) {
                            self.open_chat(chat, None, cx);
                            match &token {
                                Some(t) => cx.activate_window_with_token(t),
                                None => cx.activate_window(),
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
        let message = match message.downcast::<Launch>() {
            Ok(launch) => {
                if launch.args.iter().any(|a| a == "--reconnect") {
                    // Preferences signed in or out: start again with the
                    // account as it is.
                    self.reconnect(cx);
                } else {
                    cx.activate_window();
                }
                return;
            }
            Err(m) => m,
        };
        if let Ok(h24) = message.downcast::<Clock>() {
            self.h24 = h24.0;
            cx.invalidate();
        }
    }

    fn saved(&self) -> Saved {
        Saved {
            chat: self.conv.as_ref().map(|c| c.chat),
            topic: self.conv.as_ref().and_then(|c| c.topic),
            folder: self.view.folder,
            list_width: Some(self.side.width()),
            list_hidden: self.side.collapsed(),
        }
    }
}

impl View for Tg {
    type Message = Box<dyn std::any::Any + Send>;

    fn message(&mut self, message: Self::Message, cx: &mut Cx<Self>) {
        self.told(message, cx);
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// Closing the main window ends the app, its other windows too:
    /// once changes still on their way to Telegram are sent.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        self.quit(cx);
        false
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        self.focused = active;
        cx.invalidate();
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&self.saved()).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let bar = MenuBar::new(cx, self.menus());
        let p = theme::palette();
        let vs = cx.viewport_size();
        let scale = cx.scale_factor();
        // (A pane's edges on device pixels.)
        let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
        let snap_rect = |b: Bounds<Pixels>| Bounds::from_corners(point(snap(b.left()), snap(b.top())), point(snap(b.right()), snap(b.bottom())));
        let hairline = px(style::line_pixels(scale) / scale);
        let top = olo_kit::menu::BAR_HEIGHT;
        let full = Bounds { origin: point(px(0.0), px(top)), size: size(vs.width, (vs.height - px(top)).max(px(0.0))) };
        // Who's typing, while it lasts.
        let t = now();
        let mut next = f64::INFINITY;
        for l in self.store.typing.values_mut() {
            l.retain(|x| x.until > t);
            next = l.iter().map(|x| x.until).fold(next, f64::min);
        }
        if next.is_finite() {
            self.wake_in(Duration::from_secs_f64((next - t).max(0.05)), cx);
        }
        if self.player.as_ref().is_some_and(|(_, _, p)| p.state.finished.load(std::sync::atomic::Ordering::Relaxed)) {
            self.player = None;
        }
        let mut layer = div().absolute().inset_0();
        match &self.status {
            Status::NotSetUp | Status::SignedOut => {
                let (title, detail) = if self.status == Status::NotSetUp {
                    ("Telegram isn’t set up yet", "Connect your Telegram account in Preferences ▸ Integrations: an API ID from my.telegram.org, then sign in with a code or a QR code.")
                } else {
                    ("Not signed in to Telegram", "Sign in to your Telegram account in Preferences ▸ Integrations, with a code sent to your phone or by scanning a QR code.")
                };
                let panel = c(p.panel_background);
                layer = layer.child(div().absolute().left(full.origin.x).top(full.origin.y).w(full.size.width).h(full.size.height).bg(panel).child(
                    EmptyState::new(title).icon("telegram").detail(detail).action("Open Integrations…", |_, _| {
                        let _ = olo_xdg::spawn_detached("olo-preferences integrations");
                    }),
                ));
            }
            _ => {
                // The list, a grip, the conversation; every pane's edges on
                // device pixels, at every scale (on the 2-point grid they
                // were at 1.5× only: at 1.25× the list's pane, laid out as a
                // root of its own, ran a pixel past the window).
                let fw = f32::from(full.size.width);
                // (Away: none of it, the conversation the whole width.)
                let lw = (self.side.shown(fw, 320.0) / 2.0).round() * 2.0;
                let left = snap_rect(Bounds { origin: full.origin, size: size(px(lw), full.size.height) });
                let rx = if lw > 0.0 { snap(px(lw) + hairline) } else { full.origin.x };
                let right = snap_rect(Bounds::from_corners(point(rx, full.origin.y), full.bottom_right()));
                // Changes on their way to Telegram: a line under the list.
                let (pending, total, waiting) = self.sync;
                self.list_rect = if pending > 0 {
                    let line = Bounds::from_corners(point(left.origin.x, snap(left.bottom() - px(SYNC_H))), left.bottom_right());
                    let done = total.saturating_sub(pending) as f32 / total.max(1) as f32;
                    let label = if pending == 1 { "Sending a change to Telegram…".to_owned() } else { format!("Sending {pending} changes to Telegram…") };
                    let trailing = match (&self.status, waiting) {
                        (Status::Offline(_), _) => "offline: sent when back".to_owned(),
                        (_, Some(s)) => format!("Telegram asks to wait {s} s"),
                        _ => format!("{} of {total} sent", total - pending),
                    };
                    let edge = c(p.shadow);
                    layer = layer.child(
                        div()
                            .absolute()
                            .left(line.origin.x)
                            .top(line.origin.y)
                            .w(line.size.width)
                            .h(line.size.height)
                            .bg(c(p.panel_background))
                            .px(px(m::SPACING))
                            .pt(px(m::SPACING + 1.0))
                            .child(canvas(move |g, b| style::hline(g, b.left(), b.right(), b.top(), edge)).absolute().inset_0())
                            .child(StatusBar::new(done).label(label).trailing(trailing).bar_height(6.0)),
                    );
                    Bounds::from_corners(left.origin, point(left.right(), line.top()))
                } else {
                    left
                };
                if lw > 0.0 {
                    let list = list::render(self, self.list_rect, cx);
                    layer = layer.child(pane(list, self.list_rect, vs));
                    let shadow = c(p.shadow);
                    layer = layer.child(canvas(move |g, _| style::vline(g, left.right(), left.origin.y, left.bottom(), shadow)).absolute().inset_0());
                }
                if self.conv.is_some() {
                    // The conversation over its composer, as tall as its
                    // text (up to eight lines).
                    let (_, composer_h, _) = conv::composer_size(self, f32::from(right.size.width), cx);
                    let split = snap(right.bottom() - px(composer_h));
                    self.conv_rect = Bounds::from_corners(right.origin, point(right.right(), split));
                    self.composer_rect = Bounds::from_corners(point(right.origin.x, split), right.bottom_right());
                    let conv = conv::render(self, self.conv_rect, cx);
                    layer = layer.child(pane(conv, self.conv_rect, vs));
                    let composer = conv::composer(self, self.composer_rect, cx);
                    layer = layer.child(pane(composer, self.composer_rect, vs));
                } else {
                    let detail = match &self.status {
                        Status::Connecting => "Connecting to Telegram…".to_owned(),
                        Status::Updating => "Catching up…".to_owned(),
                        Status::Offline(why) => why.clone(),
                        Status::Waiting(s) => format!("Telegram asks us to wait {s} seconds."),
                        _ => "Choose a chat from the list to start messaging.".to_owned(),
                    };
                    let ground = bubble::Look::of(&p).ground;
                    layer = layer.child(
                        div()
                            .absolute()
                            .left(right.origin.x)
                            .top(right.origin.y)
                            .w(right.size.width)
                            .h(right.size.height)
                            .bg(ground)
                            .child(EmptyState::new("No chat selected").icon("telegram").detail(detail)),
                    );
                }
                // The divider between them: dragged to size the list, or
                // put it away for the conversation's room (the kit's
                // SidePane: past its narrowest, a double click, View ▸
                // Hide Chat List), and out again from the window's edge.
                layer = layer.child(olo_kit::widgets::side_divider(
                    &self.side,
                    lw,
                    f32::from(full.origin.y),
                    f32::from(full.size.height),
                    "Chat List",
                    cx.listener(|_: &mut Tg, _: &(), cx| cx.invalidate()),
                ));
                // A problem to tell, across the conversation's top.
                if let (Some(text), Some(until)) = (self.banner.text().cloned(), self.banner.until()) {
                    self.wake_in(until.saturating_duration_since(Instant::now()), cx);
                    let b = Bounds { origin: point(right.origin.x + px(12.0), right.origin.y + px(conv::HEADER + 8.0)), size: size(right.size.width - px(24.0), px(30.0)) };
                    let failure = c(p.failure).opacity(0.92);
                    layer = layer.child(
                        canvas(move |g, _| {
                            let b = style::snap(g, b);
                            g.paint_quad(olo_kit::fill(b, failure).corner_radii(px(8.0)));
                            style::label_fit(
                                g,
                                b.origin.x + px(12.0),
                                b.center().y,
                                Align::Left,
                                text.clone(),
                                style::PLAIN_SIZE,
                                style::ui_font(FontWeight::NORMAL),
                                white(),
                                b.size.width - px(24.0),
                            );
                            olo_kit::agent::record(g, olo_kit::agent::Node { role: "Alert", name: Some(text.clone().into()), bounds: b, ..Default::default() });
                        })
                        .absolute()
                        .inset_0(),
                    );
                }
            }
        }
        // Quitting while changes are on their way: how they go, and Quit
        // Now (what's left is kept and sent next time).
        if let Some(until) = self.quitting {
            let (pending, total, waiting) = self.sync;
            let left_s = until.saturating_duration_since(Instant::now()).as_secs();
            self.wake_in(Duration::from_secs(1), cx);
            let done = total.saturating_sub(pending) as f32 / total.max(1) as f32;
            let what = if pending == 1 { "1 change".to_owned() } else { format!("{pending} changes") };
            let trailing = match waiting {
                Some(s) => format!("Telegram asks to wait {s} s"),
                None => format!("quitting in {left_s} s at most"),
            };
            let sheet = div()
                .w(px(360.0))
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .p(px(m::WINDOW_INSET))
                .bg(c(p.panel_background))
                .border_1()
                .border_color(c(p.shadow))
                .child(StringView::new("Sending your changes to Telegram").bold(true))
                .child(StringView::new(format!("{what} to chats (archived, read, muted or pinned) still to send. Whatever isn’t sent now is kept and sent when Telegram next opens.")).wrap(true))
                .child(StatusBar::new(done).trailing(trailing).bar_height(8.0))
                .child(button_row().child(Button::new("Quit Now").on_click(|_, _| olo_kit::quit())));
            layer = layer.child(div().absolute().inset_0().flex().items_center().justify_center().bg(c(p.shadow).opacity(0.25)).child(sheet));
        }
        div()
            .size_full()
            .relative()
            .on_drop(cx.listener(|tg: &mut Tg, paths: &ExternalPaths, cx| {
                if let Some(c) = &mut tg.conv {
                    c.files.extend(paths.paths().iter().cloned());
                    c.focus_field = true;
                    cx.invalidate();
                }
            }))
            .child(layer)
            .child(bar)
    }
}

/// What a picker window is for.
enum Pick {
    /// Forwarding these messages to a chat.
    Forward { from: ChatId, ids: Vec<MsgId> },
    /// The account's contacts, one to message.
    Contacts,
}

/// What a picker's choice, or an Info window's setting, asks of the
/// main window.
enum Picked {
    Forward {
        from: ChatId,
        ids: Vec<MsgId>,
        to: ChatId,
    },
    Contact(ChatId),
    /// A chat's setting ("chat:mute", …).
    Chat(ChatId, &'static str),
}

/// A row to pick: a chat or a contact, as the main window knows it.
#[derive(Clone)]
struct PickRow {
    id: ChatId,
    name: String,
    detail: String,
    /// What else a search finds it by (a contact's username).
    also: Option<String>,
    pic: Option<std::sync::Arc<olo_kit::RenderImage>>,
}

/// The Forward To and Contacts windows: a search and a list to pick
/// from. A window of the main one's, on its looper: told the rows as
/// they change (`Tg::feed`), telling it what's picked.
struct Picker {
    main: olo_kit::Messenger<Box<dyn std::any::Any + Send>>,
    pick: Pick,
    search: Child<SearchField>,
    scroll: ScrollHandle,
    hover: Option<usize>,
    /// Every row there is, and whether they're still to come.
    all: Vec<PickRow>,
    loading: bool,
    /// How many the list shows (the search applied), and its rows' top
    /// as painted.
    shown: Vec<ChatId>,
    top: Rc<Cell<f32>>,
    focus: FocusHandle,
}

impl Picker {
    fn open(cx: &mut Cx<Tg>, pick: Pick) -> Option<LocalWindow<Picker>> {
        let forward = matches!(pick, Pick::Forward { .. });
        let (title, w, h) = if forward { ("Forward To", 360.0, 480.0) } else { ("Contacts", 360.0, 520.0) };
        let mut options = WindowOptions::new(title, w, h).min_size(300.0, 320.0);
        if forward {
            options = options.dialog();
        }
        let main = cx.messenger();
        olo_kit::open_window_here(cx, options, move |cx| {
            let search = search_field(cx, if forward { "Search chats" } else { "Search contacts" });
            cx.subscribe(&search, |p: &mut Picker, e: &SearchEvent, cx| {
                if let SearchEvent::Changed = e {
                    p.hover = None;
                    cx.invalidate();
                }
            });
            search.update(cx, |s, cx| s.focus(cx));
            Picker { main, pick, search, scroll: ScrollHandle::new(), hover: None, all: Vec::new(), loading: !forward, shown: Vec::new(), top: Rc::default(), focus: FocusHandle::new() }
        })
    }

    /// The rows, from the main window.
    fn fed(&mut self, rows: Vec<PickRow>, loading: bool, cx: &mut Cx<Self>) {
        self.all = rows;
        self.loading = loading;
        cx.invalidate();
    }

    /// A row's height: a row and the space after it.
    fn row_h(&self) -> f32 {
        if matches!(self.pick, Pick::Forward { .. }) { 40.0 + m::SPACING } else { 46.0 + m::SPACING }
    }

    fn row_at(&self, q: Point<Pixels>) -> Option<usize> {
        let y = f32::from(q.y) - self.top.get();
        let i = (y / self.row_h()).floor();
        (y >= 0.0 && (i as usize) < self.shown.len()).then_some(i as usize)
    }

    fn choose(&mut self, i: usize, cx: &mut Cx<Self>) {
        let Some(&id) = self.shown.get(i) else { return };
        match &self.pick {
            Pick::Forward { from, ids } => {
                self.main.send(Box::new(Picked::Forward { from: *from, ids: ids.clone(), to: id }));
                cx.close_window();
            }
            Pick::Contacts => {
                self.main.send(Box::new(Picked::Contact(id)));
            }
        }
    }
}

impl View for Picker {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let q = self.search.read(cx).text(cx).to_lowercase();
        let forward = matches!(self.pick, Pick::Forward { .. });
        let shown: Vec<PickRow> = self.all.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q) || r.also.as_ref().is_some_and(|u| u.contains(&q))).cloned().collect();
        self.shown = shown.iter().map(|r| r.id).collect();
        let row_h = self.row_h();
        let (hover, top) = (self.hover, self.top.clone());
        let total = shown.len();
        let rows = canvas(move |g, b| {
            top.set(f32::from(b.origin.y));
            let p = theme::palette();
            let ink = c(p.list_item_text);
            let dark = p.mode == olo_kit::palette::ThemeMode::Dark;
            let visible = g.content_mask().bounds;
            for (i, row) in shown.iter().enumerate() {
                let r = Bounds { origin: point(b.origin.x, b.origin.y + px(i as f32 * row_h)), size: size(b.size.width, px(row_h)) };
                if r.bottom() < visible.origin.y || r.origin.y > visible.bottom() {
                    continue;
                }
                if hover == Some(i) {
                    style::fill_rect(g, r, ink.opacity(0.06));
                }
                let d = if forward { 30.0 } else { 34.0 };
                let at = Bounds { origin: point(r.origin.x + px(if forward { 22.0 } else { 24.0 } - d / 2.0), r.center().y - px(d / 2.0)), size: size(px(d), px(d)) };
                bubble::avatar(g, at, row.id, &row.name, row.pic.as_ref(), dark);
                let x = at.right() + px(10.0);
                let room = r.right() - x - px(m::SPACING);
                if forward {
                    style::label_fit(g, x, r.center().y, Align::Left, row.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, room);
                } else {
                    style::label_fit(g, x, r.center().y - px(7.5), Align::Left, row.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, room);
                    style::label_fit(g, x, r.center().y + px(7.5), Align::Left, row.detail.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), room);
                }
                olo_kit::agent::record(g, olo_kit::agent::Node { role: "ListItem", name: Some(row.name.clone().into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
            }
        })
        .w_full()
        .h(px(total as f32 * row_h));
        let content = div()
            .id("pick-rows")
            .w_full()
            .h(px(total as f32 * row_h))
            .child(rows)
            .on_mouse_move(cx.listener(|p: &mut Picker, e: &MouseMoveEvent, cx| {
                let i = p.row_at(e.position);
                if i != p.hover {
                    p.hover = i;
                    cx.invalidate();
                }
            }))
            .on_hover(cx.listener(|p: &mut Picker, on: &bool, cx| {
                if !*on && p.hover.take().is_some() {
                    cx.invalidate();
                }
            }))
            .on_left_click(cx.listener(|p: &mut Picker, e: &olo_kit::ClickEvent, cx| {
                let olo_kit::ClickEvent::Mouse(mc) = e else { return };
                if let Some(i) = p.row_at(mc.up.position) {
                    p.choose(i, cx);
                }
            }));
        let list: Element = if self.loading { EmptyState::new("Loading contacts…").icon("people").into_any_element() } else { ScrollView::new(&self.scroll).child(content).into_any_element() };
        let mut col = div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::TIGHT + m::SPACING)).child(&self.search).child(div().flex_1().min_h_0().child(list));
        if forward {
            col = col.child(div().child(button_row().child(Button::new("Cancel").on_click(cx.listener(|_: &mut Picker, _: &olo_kit::ClickEvent, cx| cx.close_window())))));
        }
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_: &mut Picker, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && forward {
                    cx.close_window();
                }
            }))
            .child(col)
    }
}

/// A chat's Info window: its picture, name and status, and its settings.
/// A window of the main one's, told the chat as it changes
/// (`Tg::feed`).
struct Info {
    main: olo_kit::Messenger<Box<dyn std::any::Any + Send>>,
    chat: ChatId,
    ch: Option<Chat>,
    pic: Option<std::sync::Arc<olo_kit::RenderImage>>,
    focus: FocusHandle,
}

impl View for Info {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let chat = self.chat;
        let Some(ch) = self.ch.clone() else { return div().size_full() };
        let pic = self.pic.clone();
        let sub = match ch.kind {
            ChatKind::User | ChatKind::Bot => ch.seen.as_ref().map(|s| s.describe(when::now())).unwrap_or_default(),
            ChatKind::Channel => ch.members.map(|n| format!("{n} subscribers")).unwrap_or_default(),
            ChatKind::Group | ChatKind::Forum => ch.members.map(|n| format!("{n} members")).unwrap_or_default(),
            ChatKind::Saved => "Your notes, files and forwarded messages".into(),
        };
        let (title, id) = (ch.title.clone(), ch.id);
        let head = canvas(move |g, b| {
            let p = theme::palette();
            let at = Bounds { origin: point(b.center().x - px(48.0), b.origin.y + px(22.0)), size: size(px(96.0), px(96.0)) };
            bubble::avatar(g, at, id, &title, pic.as_ref(), p.mode == olo_kit::palette::ThemeMode::Dark);
            let ink = c(p.panel_text);
            let width = b.size.width - px(2.0 * m::WINDOW_INSET);
            let t = style::label_fit(g, b.center().x, at.bottom() + px(20.0), Align::Center, title.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink, width);
            olo_kit::agent::record(g, olo_kit::agent::Node::label(title.clone(), t));
            if !sub.is_empty() {
                let s = style::label_fit(g, b.center().x, at.bottom() + px(40.0), Align::Center, sub.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), width);
                olo_kit::agent::record(g, olo_kit::agent::Node::label(sub.clone(), s));
            }
        })
        .w_full()
        .h(px(22.0 + 96.0 + 60.0));
        let toggle = |label: &'static str, on: bool, cmd: &'static str| {
            let main = self.main.clone();
            CheckBox::new(label, on).on_toggle(move |_, _| {
                main.send(Box::new(Picked::Chat(chat, cmd)));
            })
        };
        // Its settings, a space under the username, and the link a
        // space under them.
        let mut body = div().px(px(m::WINDOW_INSET)).pt(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING));
        if let Some(u) = &ch.username {
            body = body.child(BBox::new("Username").child(StringView::new(format!("@{u}"))));
        }
        body = body.child(div()).child(toggle("Mute notifications", ch.muted, "chat:mute")).child(toggle("Pin in chat list", ch.pinned, "chat:pin")).child(toggle(
            "Archived",
            ch.archived,
            "chat:archive",
        ));
        if let Some(u) = ch.username.clone() {
            body = body.child(div()).child(div().flex().flex_row().child(Button::new("Copy Link").on_click(move |_, _| clipboard::write(ClipboardItem::new_string(format!("https://t.me/{u}"))))));
        }
        // Escape or ⌘W puts it away.
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_: &mut Info, e: &KeyDownEvent, cx| {
                let k = &e.keystroke;
                if (k.key == "escape" && !k.modifiers.modified()) || (k.key == "w" && k.modifiers.control) {
                    cx.close_window();
                }
            }))
            .child(head)
            .child(body)
    }
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |app| {
        let proxy = olo_kit::proxy();
        instance::listen(APP_ID, proxy.clone());
        let events = proxy.clone();
        let backend = Backend::start(move |e| events.send(e));
        backend.send(Cmd::Chats);
        let clock = proxy.clone();
        olo_kit::settings::watch(move |s| clock.send(Clock(s.clock.twenty_four_hour)));
        let restored: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let window = olo_kit::open_window(WindowOptions::new("Telegram", 980.0, 660.0).min_size(620.0, 420.0).keep_size("main"), move |cx| Tg::new(backend, restored, cx));
        // What the account, the pictures' and stickers' workers, the
        // microphone, notifications and another launch say: the window's.
        olo_kit::message_handler(app, move |message, _| {
            window.messenger().send(message);
        });
    });
}

impl Tg {
    fn new(backend: Backend, restored: Saved, cx: &mut Cx<Self>) -> Tg {
        let proxy = olo_kit::proxy();
        let search = search_field(cx, "Search");
        let field = text_field(cx, |f| f.multiline(true).font_size(bubble::TEXT).name("Message").hint("Message"));
        cx.subscribe(&search, |tg: &mut Tg, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                tg.search_changed(cx);
            }
        });
        cx.subscribe(&field, |tg: &mut Tg, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => {
                let can = tg.conv.as_ref().is_some_and(|c| !tg.field.read(cx).text().trim().is_empty() || !c.files.is_empty());
                if can {
                    tg.send(cx);
                }
            }
            TextFieldEvent::Changed => tg.typed(cx),
            TextFieldEvent::Typed(_) => {}
        });
        Tg {
            backend,
            store: store::Store::default(),
            pics: pics::Pics::new(proxy.clone()),
            stickers: stickers::Stickers::new(proxy.clone()),
            status: Status::Connecting,
            sync: (0, 0, None),
            quitting: None,
            me: None,
            view: list::View { folder: restored.folder, ..list::View::default() },
            found: list::Found::default(),
            conv: None,
            side: SidePane::new(list::WIDTH, list::MIN_WIDTH).restored(restored.list_width, restored.list_hidden),
            h24: false,
            notes: Notifier::new(proxy, "Telegram", "olo-telegram"),
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
            voice: None,
            voice_token: 0,
            voice_ended: HashMap::new(),
            voice_to: HashMap::new(),
            focused: true,
            drafts: HashMap::new(),
            kept: Vec::new(),
            banner: Notice::default(),
            searched: String::new(),
            searching: None,
            restored,
            badge: 0,
            search,
            field,
            hot: None,
            list_scroll: ScrollHandle::new(),
            list_rows: Vec::new(),
            chosen: Vec::new(),
            chosen_from: None,
            list_hover: None,
            list_top: Rc::default(),
            wake: Due::default(),
            title: "Telegram".into(),
            list_rect: Bounds::default(),
            conv_rect: Bounds::default(),
            composer_rect: Bounds::default(),
        }
    }
}
