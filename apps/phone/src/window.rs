//! Phone, the app: the phone's conversations and contacts in a window
//! (Ben, 2026-11-16: "did you already build an app for working with the
//! messages, contacts etc? if not we need that too", and "make sure the
//! messages/contacts/etc app is really nicely styled, the weather and
//! telegram apps are good examples of well styled apps"). As Telegram's:
//! a list down the left (the conversations, each with who it's with,
//! their latest words, when, and how many are unread; or the contacts),
//! and beside it the conversation as balloons (ours on the right in a
//! tint of the keyboard's blue, theirs on the left on the card colour,
//! runs of one side's joined, a heading for each day), with the field to
//! write in under it. A one-time code in a message has Copy under it.
//!
//! It holds nothing of its own: the service has the phone
//! (`olo-phoned`, service.rs), and this asks it on its socket,
//! off the UI thread, whenever the state it publishes says something
//! changed (`PhoneState::changes`; the file followed, not polled). What
//! a person can do here an agent can by command (`open:KEY`, `send:TEXT`,
//! `to:ADDRESS`, `tab:messages`), and reads as the window's tree: each
//! row and balloon is recorded as it's painted.
//!
//! `OLO_PHONE_DEMO=1`: made-up people and messages and no service, for
//! the layout lint, the benchmark and pictures.

use std::{cell::RefCell, rc::Rc, time::Duration};

use olo_kit::{
    BorderStyle, Bounds, Child, ClickEvent, ClipboardItem, Corners, CursorStyle, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, Menu, MenuBar, MouseButton, MouseDownEvent,
    MouseMoveEvent, Painter, ParentElement as _, Pixels, ScrollHandle, SharedString, Styled as _, View, WindowOptions, agent, canvas, div, hsla, metrics as m,
    para::{self, Para},
    person, point, px, quad,
    search::{SearchEvent, SearchField, search_field},
    size,
    style::{self, Align, c},
    theme, transparent_black, when, white,
    widgets::{Button, EmptyState, ScrollView, SidePane, StringView, TextField, TextFieldEvent, side_divider, text_field},
};
use olo_protocol::phone::{self, Conversation, Kind, Link, Message, Person, PhoneState, Request};
use olo_system::phone::pbap;
use serde::{Deserialize, Serialize};

use olo_phone::codes;

pub const APP_ID: &str = "olo.phone";

/// A conversation's row, and a contact's.
const ROW: f32 = 64.0;
const PERSON_ROW: f32 = 48.0;
const PICTURE: f32 = 40.0;
const LIST: f32 = 300.0;
const LIST_MIN: f32 = 230.0;
/// The conversation's header.
const HEADER: f32 = 60.0;
/// A message's text, and the small print (its time).
const TEXT: f32 = 13.0;
const META: f32 = 10.5;
/// Inside a balloon; its corners, and one where it joins the next of the
/// same side's.
const PAD: (f32, f32) = (11.0, 7.0);
const RADIUS: f32 = 13.0;
const JOINED: f32 = 4.0;
/// The widest a balloon gets, and the most of the pane's width.
const WIDEST: f32 = 460.0;
const SHARE: f32 = 0.74;
/// Messages of one side closer together than this are a run.
const RUN: i64 = 120;
/// The send button, across.
const SEND: f32 = 34.0;
/// How long the service has to answer (it waits on the phone to send).
const ASK: Duration = Duration::from_secs(45);

pub fn options() -> WindowOptions {
    WindowOptions::new("Phone", 920.0, 640.0).min_size(660.0, 420.0).keep_size("main")
}

/// `OLO_PHONE_DEMO`: made-up people and no service; a number: that many
/// more conversations and contacts (the benchmark's).
fn demo() -> Option<usize> {
    let v = std::env::var("OLO_PHONE_DEMO").ok().filter(|v| !v.is_empty())?;
    Some(v.parse::<usize>().ok().filter(|n| *n > 1).unwrap_or(0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Tab {
    #[default]
    Messages,
    Contacts,
}

/// What the right of the window shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
enum Shown {
    #[default]
    Nothing,
    /// A conversation, by its key.
    Thread(String),
    /// A message to an address there's no conversation with yet.
    New(String),
    /// A contact's card (their name).
    Person(String),
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    tab: Tab,
    shown: Shown,
    draft: String,
    list_width: Option<f32>,
    list_hidden: bool,
}

/// What the layout lint draws the window as.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub enum LintState {
    Conversation,
    Contacts,
    Card,
    Nothing,
    NewMessage,
}

pub enum Msg {
    /// The service's state, as it publishes it.
    State(PhoneState),
    /// Asking `n`'s answer: the conversations (or why not), the contacts.
    Loaded(u64, Result<Vec<Conversation>, String>, Vec<Person>),
    /// A message sent, or why it wasn't (with what it was, to put back).
    Sent(Result<(), String>, String),
    /// The clock's style changed: 24 hours or 12.
    Clock(bool),
    /// Started again while running, with these arguments.
    Launch(Vec<String>),
    /// Something told to the service, with nothing to hear back.
    Told,
    /// The latest one-time code, asked for to copy.
    Code(Option<String>),
}

/// A balloon laid out: where it is from the conversation's top, and
/// what's in it. Its left is settled as it's painted (ours keep to the
/// right of however wide the pane is).
struct Balloon {
    y: f32,
    w: f32,
    h: f32,
    ours: bool,
    /// Its text, shaped; None: not yet (its size is a guess meanwhile).
    text: Option<Rc<Para>>,
    time: SharedString,
    /// The time beside the text's last line, or on a line of its own.
    time_under: bool,
    joins_above: bool,
    joins_below: bool,
    /// A one-time code it has: copied by the button under it.
    code: Option<String>,
    /// Which message of the conversation.
    index: usize,
}

enum Item {
    Day(f32, SharedString),
    Balloon(Balloon),
}

/// A message's text shaped for the width and look in force, the code it
/// has, and its time as it's written (and how wide).
#[derive(Clone)]
struct Wrapped {
    text: Rc<Para>,
    code: Option<String>,
    time: SharedString,
    time_w: f32,
}

/// Messages shaped: kept while the conversation, its width, the look and
/// the clock's style stay. Shaping is the cost (some 70 µs a message),
/// so it's never all done in a frame: what's in view at once, the rest
/// a few milliseconds a frame (`shape_more`), a message not yet shaped
/// taking its size as it was before, or a guess by its length.
#[derive(Default)]
struct Shaped {
    of: (String, u32, u64, bool),
    /// By the message's place in the conversation: which message it is
    /// (its handle and length: a conversation asked for again is the
    /// same messages, or isn't), its text shaped, and the day it's of.
    ids: Vec<(String, usize)>,
    text: Vec<Option<Wrapped>>,
    days: Vec<i32>,
    /// How many are shaped.
    count: usize,
    /// Sizes as last shaped (another width, another look): to guess by.
    was: Vec<Option<(f32, f32)>>,
}

impl Shaped {
    /// Made to hold conversation `t` as it is now: what's shaped of the
    /// messages it still has is kept, the rest made room for.
    fn hold(&mut self, t: &Conversation) {
        let n = t.messages.len();
        let same = self.ids.len() == n && t.messages.iter().zip(&self.ids).all(|(m, id)| m.handle == id.0 && m.text.len() == id.1);
        if same {
            return;
        }
        for (i, m) in t.messages.iter().enumerate() {
            let kept = self.ids.get(i).is_some_and(|id| m.handle == id.0 && m.text.len() == id.1);
            if !kept {
                let id = (m.handle.clone(), m.text.len());
                let day = when::day_number(m.at);
                if i < self.ids.len() {
                    (self.ids[i], self.text[i], self.days[i]) = (id, None, day);
                    if let Some(was) = self.was.get_mut(i) {
                        *was = None;
                    }
                } else {
                    self.ids.push(id);
                    self.text.push(None);
                    self.days.push(day);
                }
            }
        }
        self.ids.truncate(n);
        self.text.truncate(n);
        self.days.truncate(n);
        self.was.resize(n, None);
        self.count = self.text.iter().filter(|t| t.is_some()).count();
    }
}

/// What a conversation was laid out for: itself and how much of it, how
/// many of its messages are shaped, the width, the look, the clock's
/// style and the day.
type LaidFor = (String, usize, Option<(String, bool)>, usize, u32, u64, bool, i32);

/// The most of a frame given to shaping messages not in view.
const SHAPING: Duration = Duration::from_micros(2500);

pub struct Phone {
    state: PhoneState,
    threads: Vec<Conversation>,
    contacts: Vec<Person>,
    /// The service couldn't be asked (it isn't running, or says no).
    failed: Option<String>,
    tab: Tab,
    shown: Shown,
    search: Child<SearchField>,
    draft: Child<TextField>,
    side: SidePane,
    list_scroll: ScrollHandle,
    people_scroll: ScrollHandle,
    talk_scroll: ScrollHandle,
    /// The row under the pointer: which list, which row.
    hot: Option<(&'static str, usize)>,
    /// A message on its way (the field waits), and why the last wasn't
    /// sent.
    sending: bool,
    trouble: Option<String>,
    /// Askings made, and the state's count of changes when last asked.
    asked: u64,
    seen_changes: Option<u64>,
    /// The conversation shown is to be at its end once laid out (opened,
    /// or a message came while it was there).
    to_end: bool,
    h24: bool,
    demo: bool,
    shaped: RefCell<Shaped>,
    /// Where the conversation's balloons and their Copy buttons are, from
    /// its top left, as last laid out (for clicks), what that was laid
    /// out for (laid out again only when it changes) and how tall.
    laid: Rc<RefCell<Vec<Item>>>,
    laid_for: RefCell<Option<(LaidFor, f32)>>,
    /// What the search finds in each list, for the words and the data it
    /// was found in (`data` counts the data's changes).
    found: RefCell<[Option<(String, u64, Rc<Vec<usize>>)>; 2]>,
    data: u64,
    /// The conversation's pane as last laid out (its width), and whether
    /// more shaping is due next frame.
    talk_width: f32,
    shaping: bool,
    _watch: Option<olo_kit::watch::Watch>,
}

fn key_of(address: &str) -> String {
    pbap::key(address)
}

/// A name with letters in it (a person's), not a number or an address.
fn named(name: &str) -> bool {
    name.chars().any(char::is_alphabetic) && !name.contains('@')
}

/// Someone's picture: their initials on their colour, or a head and
/// shoulders where only a number is known.
fn picture(g: &mut Painter, r: Bounds<Pixels>, name: &str, address: &str, dark: bool) {
    let id = person::id_of(&key_of(if address.is_empty() { name } else { address }));
    if named(name) {
        person::picture(g, r, id, name, None, dark)
    } else {
        person::nobody(g, r, id, dark)
    }
    olo_kit::review::own_grid(g, r);
}

/// What a message says, on one line.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(160));
    for ch in text.chars().take(160) {
        out.push(if ch.is_whitespace() { ' ' } else { ch });
    }
    out
}

/// Whether `text` could be a number or an address to write to.
fn writable(text: &str) -> bool {
    let t = text.trim();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    (digits >= 3 && t.chars().all(|ch| ch.is_ascii_digit() || " +-().".contains(ch))) || (t.contains('@') && t.contains('.') && !t.contains(' '))
}

impl Phone {
    pub fn new(cx: &mut Cx<Phone>) -> Phone {
        let saved: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Phone::with(cx, saved, demo())
    }

    fn with(cx: &mut Cx<Phone>, saved: Saved, demo: Option<usize>) -> Phone {
        let search = search_field(cx, "Search");
        cx.subscribe(&search, |v: &mut Phone, e: &SearchEvent, cx| {
            if matches!(e, SearchEvent::Changed) {
                v.hot = None;
                cx.invalidate();
            }
            // Return on a number or an address typed: a message to it.
            if matches!(e, SearchEvent::Entered) {
                let q = v.search.read(cx).text(cx).trim().to_owned();
                if writable(&q) {
                    v.write_to(&q, cx);
                }
            }
        });
        let draft = text_field(cx, |f| f.multiline(true).font_size(TEXT).name("Message").hint("Text Message").with_text(saved.draft.clone()));
        cx.subscribe(&draft, |v: &mut Phone, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => v.send_typed(cx),
            // (Its height follows its lines; Send follows whether there's
            // anything.)
            TextFieldEvent::Changed => cx.invalidate(),
            TextFieldEvent::Typed(_) => {}
        });
        let mut phone = Phone {
            state: PhoneState::default(),
            threads: Vec::new(),
            contacts: Vec::new(),
            failed: None,
            tab: saved.tab,
            shown: saved.shown,
            search,
            draft,
            side: SidePane::new(LIST, LIST_MIN).restored(saved.list_width, saved.list_hidden),
            list_scroll: ScrollHandle::new(),
            people_scroll: ScrollHandle::new(),
            talk_scroll: ScrollHandle::new(),
            hot: None,
            sending: false,
            trouble: None,
            asked: 0,
            seen_changes: None,
            to_end: true,
            h24: false,
            demo: demo.is_some(),
            shaped: RefCell::default(),
            laid: Rc::default(),
            laid_for: RefCell::default(),
            found: RefCell::default(),
            data: 0,
            talk_width: 0.0,
            shaping: false,
            _watch: None,
        };
        if let Some(many) = demo {
            let (state, threads, contacts) = crate::demo::sample(when::now(), many);
            (phone.state, phone.threads, phone.contacts) = (state, threads, contacts);
            if phone.shown == Shown::Nothing {
                phone.shown = phone.threads.first().map(|t| Shown::Thread(t.key.clone())).unwrap_or_default();
            }
            return phone;
        }
        // The service's state: read now, and whenever it's written (off
        // this thread both times).
        let to = cx.messenger();
        phone._watch = olo_kit::watch::watch([PhoneState::path()], move |_| {
            to.send(Msg::State(PhoneState::load()));
        });
        cx.work(PhoneState::load, Msg::State);
        let clock = cx.messenger();
        olo_kit::settings::watch(move |s| {
            clock.send(Msg::Clock(s.clock.twenty_four_hour));
        });
        phone
    }

    /// As the layout lint draws it: the demo's people, in `state`.
    #[cfg(test)]
    pub fn for_lint(cx: &mut Cx<Phone>, state: LintState) -> Phone {
        let mut phone = Phone::with(cx, Saved::default(), Some(0));
        match state {
            LintState::Conversation => {}
            LintState::Contacts => phone.tab = Tab::Contacts,
            LintState::Card => (phone.tab, phone.shown) = (Tab::Contacts, Shown::Person("Mira Kovač".into())),
            LintState::Nothing => phone.shown = Shown::Nothing,
            LintState::NewMessage => phone.shown = Shown::New("+1 (510) 555-0100".into()),
        }
        phone
    }

    /// Ask the service for the conversations and the contacts.
    fn load(&mut self, cx: &mut Cx<Self>) {
        if self.demo {
            return;
        }
        self.asked += 1;
        let n = self.asked;
        cx.work(
            || {
                let threads = match phone::ask(&Request::Conversations, ASK) {
                    Ok(r) if r.ok => Ok(r.conversations.unwrap_or_default()),
                    Ok(r) => Err(r.error.unwrap_or_else(|| "The phone service wouldn’t say.".into())),
                    Err(_) => Err("The phone service isn’t running.".into()),
                };
                let contacts = phone::ask(&Request::Contacts { query: String::new() }, ASK).ok().and_then(|r| r.contacts).unwrap_or_default();
                (threads, contacts)
            },
            move |(threads, contacts)| Msg::Loaded(n, threads, contacts),
        );
    }

    fn thread(&self) -> Option<&Conversation> {
        match &self.shown {
            Shown::Thread(key) => self.threads.iter().find(|t| &t.key == key),
            _ => None,
        }
    }

    /// The address the field writes to, and the name over it.
    fn writing_to(&self) -> Option<(String, String)> {
        match &self.shown {
            Shown::Thread(_) => self.thread().map(|t| (t.address.clone(), t.name.clone())),
            Shown::New(address) => Some((address.clone(), self.name_of(address).unwrap_or_else(|| address.clone()))),
            _ => None,
        }
    }

    fn name_of(&self, address: &str) -> Option<String> {
        let key = key_of(address);
        self.contacts.iter().find(|p| p.phones.iter().chain(&p.emails).any(|a| key_of(a) == key)).map(|p| p.name.clone())
    }

    /// What the search finds in list `which` (0: the conversations, 1:
    /// the contacts), found again only when the words or the data change.
    fn found(&self, which: usize, cx: &Cx<Self>, find: impl FnOnce(&str) -> Vec<usize>) -> Rc<Vec<usize>> {
        let q = self.search.read(cx).text(cx).trim().to_lowercase();
        let mut kept = self.found.borrow_mut();
        if let Some((words, data, found)) = &kept[which]
            && *words == q
            && *data == self.data
        {
            return found.clone();
        }
        let found = Rc::new(find(&q));
        kept[which] = Some((q, self.data, found.clone()));
        found
    }

    /// The conversations the list shows: all, or those the search finds
    /// (in a name, a number or what was said).
    fn listed(&self, cx: &Cx<Self>) -> Rc<Vec<usize>> {
        self.found(0, cx, |q| {
            (0..self.threads.len())
                .filter(|&i| {
                    let t = &self.threads[i];
                    q.is_empty() || t.name.to_lowercase().contains(q) || t.address.to_lowercase().contains(q) || t.messages.iter().any(|m| m.text.to_lowercase().contains(q))
                })
                .collect()
        })
    }

    fn people(&self, cx: &Cx<Self>) -> Rc<Vec<usize>> {
        self.found(1, cx, |q| {
            let digits: String = q.chars().filter(char::is_ascii_digit).collect();
            (0..self.contacts.len())
                .filter(|&i| {
                    let p = &self.contacts[i];
                    q.is_empty()
                        || p.name.to_lowercase().contains(q)
                        || p.emails.iter().any(|e| e.to_lowercase().contains(q))
                        || (digits.len() >= 2 && p.phones.iter().any(|n| n.chars().filter(char::is_ascii_digit).collect::<String>().contains(&digits)))
                })
                .collect()
        })
    }

    fn open_thread(&mut self, key: &str, cx: &mut Cx<Self>) {
        self.shown = Shown::Thread(key.to_owned());
        self.tab = Tab::Messages;
        (self.to_end, self.trouble) = (true, None);
        self.mark_read(cx);
        self.draft.focus(cx);
        cx.invalidate();
    }

    /// A message to `address`: its conversation if there is one, else a
    /// new one.
    fn write_to(&mut self, address: &str, cx: &mut Cx<Self>) {
        let key = key_of(address);
        match self.threads.iter().find(|t| t.key == key).map(|t| t.key.clone()) {
            Some(key) => self.open_thread(&key, cx),
            None => {
                self.shown = Shown::New(address.trim().to_owned());
                self.trouble = None;
                self.draft.focus(cx);
                cx.invalidate();
            }
        }
    }

    /// What's shown and unread is read: said to the service (and so to
    /// the phone), with the window in front.
    fn mark_read(&mut self, cx: &mut Cx<Self>) {
        let Some(t) = self.thread() else { return };
        if t.unread() == 0 || !cx.is_window_active() {
            return;
        }
        let key = t.key.clone();
        if let Some(t) = self.threads.iter_mut().find(|t| t.key == key) {
            t.messages.iter_mut().for_each(|m| m.read = true);
        }
        if !self.demo {
            cx.work(move || drop(phone::ask(&Request::MarkRead { key }, ASK)), |()| Msg::Told);
        }
    }

    fn send_typed(&mut self, cx: &mut Cx<Self>) {
        let text = self.draft.read(cx).text().trim().to_owned();
        if !text.is_empty() {
            self.send(text, cx);
        }
    }

    fn send(&mut self, text: String, cx: &mut Cx<Self>) {
        let Some((to, _)) = self.writing_to() else { return };
        if self.sending {
            return;
        }
        self.draft.update(cx, |f, cx| f.set_text("", cx));
        (self.trouble, self.to_end) = (None, true);
        if self.demo {
            let key = key_of(&to);
            let message = Message { handle: format!("demo-{}", when::now()), text, at: when::now(), outgoing: true, read: true };
            match self.threads.iter_mut().find(|t| t.key == key) {
                Some(t) => t.messages.push(message),
                None => self.threads.insert(0, Conversation { key: key.clone(), name: self.name_of(&to).unwrap_or_else(|| to.clone()), address: to, messages: vec![message] }),
            }
            self.shown = Shown::Thread(key);
            self.data += 1;
            return cx.invalidate();
        }
        self.sending = true;
        let said = text.clone();
        cx.work(
            move || match phone::ask(&Request::Send { to, text }, Duration::from_secs(60)) {
                Ok(r) if r.ok => Ok(()),
                Ok(r) => Err(r.error.unwrap_or_else(|| "The phone didn’t take it.".into())),
                Err(_) => Err("The phone service isn’t running.".into()),
            },
            move |done| Msg::Sent(done, said),
        );
        cx.invalidate();
    }

    /// The conversation as text (Edit ▸ Copy Conversation).
    fn as_text(&self) -> String {
        let Some(t) = self.thread() else { return String::new() };
        let mut out = vec![format!("{} ({})", t.name, t.address)];
        for m in &t.messages {
            out.push(format!("{} {}  {}: {}", when::day(m.at, when::now()), when::time(m.at, self.h24), if m.outgoing { "Me" } else { t.name.as_str() }, m.text));
        }
        out.join("\n")
    }

    fn menus(&self) -> Vec<Menu> {
        let open = self.thread().is_some();
        let unread = self.thread().is_some_and(|t| t.unread() > 0);
        let paired = self.state.paired() || self.demo;
        vec![
            Menu::new("File").app_items(&["phone"]).item("New Message", "new-message").key("n").enabled(paired).close_quit(),
            Menu::new("Edit").item("Find…", "find").key("f").item("Copy Conversation", "copy-conversation").enabled(open),
            Menu::new("View")
                .item("Messages", "tab:messages")
                .key("1")
                .checked(self.tab == Tab::Messages)
                .item("Contacts", "tab:contacts")
                .key("2")
                .checked(self.tab == Tab::Contacts)
                .separator()
                .item(if self.side.collapsed() { "Show List" } else { "Hide List" }, "sidebar")
                .key("s"),
            Menu::new("Conversation")
                .item("Mark as Read", "mark-read")
                .enabled(unread)
                .item("Copy Latest Code", "copy-code")
                .enabled(self.state.code_at.is_some())
                .separator()
                .item("Try the Phone Again", "reconnect")
                .enabled(paired && !self.demo),
            Menu::new("Music")
                .item(if self.state.music.as_ref().is_some_and(|m| m.playing) { "Pause" } else { "Play" }, "music:toggle")
                .key("p")
                .enabled(self.state.music.is_some())
                .item("Next", "music:next")
                .enabled(self.state.music.is_some())
                .item("Previous", "music:previous")
                .enabled(self.state.music.is_some()),
        ]
    }
}

impl View for Phone {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::State(state) => {
                if state.music != self.state.music {
                    cx.invalidate();
                }
                let changed = self.seen_changes != Some(state.changes) || state.link != self.state.link || state.contacts != self.state.contacts || state.pid != self.state.pid;
                self.state = state;
                if changed {
                    self.seen_changes = Some(self.state.changes);
                    self.load(cx);
                }
            }
            Msg::Loaded(n, threads, contacts) => {
                // (An earlier asking's: old news.)
                if n != self.asked {
                    return;
                }
                match threads {
                    Ok(mut threads) => {
                        threads.sort_by_key(|t| std::cmp::Reverse(t.latest()));
                        // More in the conversation shown: to its end.
                        let had = self.thread().map(|t| t.messages.len());
                        self.threads = threads;
                        self.data += 1;
                        if had.is_some_and(|n| self.thread().is_some_and(|t| t.messages.len() > n)) {
                            self.to_end = true;
                        }
                        // A message just sent to someone new: theirs now.
                        if let Shown::New(address) = &self.shown {
                            let key = key_of(address);
                            if self.threads.iter().any(|t| t.key == key) {
                                self.shown = Shown::Thread(key);
                            }
                        }
                        self.failed = None;
                    }
                    Err(why) => self.failed = Some(why),
                }
                self.contacts = contacts;
                self.data += 1;
                self.mark_read(cx);
            }
            Msg::Sent(done, text) => {
                self.sending = false;
                if let Err(why) = done {
                    // What wasn't sent is back in the field, to send again.
                    self.trouble = Some(why);
                    if self.draft.read(cx).text().is_empty() {
                        self.draft.update(cx, |f, cx| f.set_text(text, cx));
                    }
                }
            }
            Msg::Clock(h24) => self.h24 = h24,
            Msg::Told => return,
            Msg::Code(code) => {
                if let Some(code) = code {
                    olo_kit::clipboard::write(ClipboardItem::new_string(code));
                }
                return;
            }
            Msg::Launch(args) => {
                if let Some(key) = args.iter().position(|a| a == "--open").and_then(|i| args.get(i + 1)).cloned() {
                    match self.threads.iter().find(|t| t.key == key || t.name == key).map(|t| t.key.clone()) {
                        Some(key) => self.open_thread(&key, cx),
                        None => self.shown = Shown::Thread(key),
                    }
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "new-message" => {
                self.tab = Tab::Contacts;
                self.search.update(cx, |s, cx| s.focus(cx));
            }
            "find" => self.search.update(cx, |s, cx| s.focus(cx)),
            "copy-conversation" => olo_kit::clipboard::write(ClipboardItem::new_string(self.as_text())),
            "tab:messages" => self.tab = Tab::Messages,
            "tab:contacts" => self.tab = Tab::Contacts,
            "sidebar" => self.side.toggle(),
            "mark-read" => self.mark_read(cx),
            "copy-code" => {
                if !self.demo {
                    cx.work(|| phone::ask(&Request::Code, ASK).ok().and_then(|r| r.code), Msg::Code);
                }
            }
            "reconnect" => {
                cx.work(|| drop(phone::ask(&Request::Reconnect, ASK)), |()| Msg::Told);
            }
            // A call through the phone to a number (its sound here);
            // the call ringing answered here; the call ended.
            c if c.starts_with("call:") || c == "answer" || c == "hang-up" => {
                let request = match c {
                    "answer" => Request::Answer,
                    "hang-up" => Request::HangUp,
                    _ => Request::Call { number: c[5..].trim().to_owned() },
                };
                if !self.demo {
                    // (What can't be done, the service says itself.)
                    cx.work(move || drop(phone::ask(&request, ASK)), |()| Msg::Told);
                }
            }
            // The phone's music: play or pause, the next, the one before.
            c if c.starts_with("music:") => {
                let Some(action) = phone::MusicAction::parse(&c[6..]) else { return false };
                if self.state.music.is_none() {
                    return false;
                }
                if self.demo {
                    // (Nothing to play: shown as it would be.)
                    if let (Some(m), phone::MusicAction::Toggle) = (&mut self.state.music, action) {
                        m.playing = !m.playing;
                    }
                } else {
                    cx.work(move || drop(phone::ask(&Request::Music { action }, ASK)), |()| Msg::Told);
                }
            }
            "send" => self.send_typed(cx),
            "close" | "quit" => olo_kit::quit(),
            // (An agent's: a conversation by its key or name; a message to
            // an address; what to send in the one shown.)
            c if c.starts_with("open:") => {
                let which = &c[5..];
                match self.threads.iter().find(|t| t.key == which || t.name == which).map(|t| t.key.clone()) {
                    Some(key) => self.open_thread(&key, cx),
                    None => return false,
                }
            }
            c if c.starts_with("to:") => self.write_to(&c[3..], cx),
            c if c.starts_with("send:") => self.send(c[5..].trim().to_owned(), cx),
            _ => return false,
        }
        cx.invalidate();
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let bar = MenuBar::new(cx, self.menus());
        let vs = cx.viewport_size();
        let total = f32::from(vs.width);
        let lw = (self.side.shown(total, 360.0) / 2.0).round() * 2.0;
        let right_w = total - lw;
        let left = (lw > 0.0).then(|| self.left(lw, cx));
        let right = self.right(right_w, cx);
        let shadow = c(p.shadow);
        let mut row = div().flex_1().min_h_0().flex().flex_row().relative();
        if let Some(left) = left {
            row = row.child(div().w(px(lw)).h_full().flex_shrink_0().child(left));
        }
        row = row.child(div().flex_1().min_w_0().h_full().child(right));
        if lw > 0.0 {
            // (The line between them, over both: on device pixels, with
            // no room of its own for a seam at 1.5×.)
            row = row.child(canvas(move |g, b: Bounds<Pixels>| style::vline(g, b.left() + px(lw), b.top(), b.bottom(), shadow)).absolute().inset_0());
        }
        // The divider: dragged to size the list, or to put it away.
        let top = olo_kit::menu::BAR_HEIGHT;
        let divider = side_divider(&self.side, lw, top, f32::from(vs.height) - top, "Conversations", cx.listener(|_: &mut Phone, _: &(), cx| cx.invalidate()));
        div().size_full().flex().flex_col().bg(c(p.panel_background)).child(bar).child(row).child(divider).into_element()
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        let saved = Saved { tab: self.tab, shown: self.shown.clone(), draft: self.draft.read(cx).text().to_owned(), list_width: Some(self.side.width()), list_hidden: self.side.collapsed() };
        serde_json::to_string(&saved).ok()
    }
}

/// What of a scroller's content shows: from where to where, down from
/// its top (a row's worth more either side: a scroll draws again). By
/// where it's scrolled to, not by what a drawing is clipped to, which
/// isn't narrowed to the scroller: painted by that, every row of 2,000
/// was drawn each frame (6 ms of the benchmark's 8.3).
fn in_view(scroll: &ScrollHandle, margin: f32) -> (f32, f32) {
    let from = -f32::from(scroll.offset().y);
    (from - margin, from + f32::from(scroll.bounds().size.height) + margin)
}

/// A list painted in one drawing, only its rows in view: `n` rows of
/// `row` points in the scroller `scroll`, painted by `paint` (the row,
/// whether the pointer is on it) and chosen by `pick`.
fn rows(
    cx: &mut Cx<Phone>,
    id: &'static str,
    scroll: &ScrollHandle,
    n: usize,
    row: f32,
    paint: impl Fn(&Phone, &mut Painter, Bounds<Pixels>, usize, bool) + 'static,
    pick: impl Fn(&mut Phone, usize, &mut Cx<Phone>) + 'static,
) -> Element {
    let scroll = scroll.clone();
    let drawn = cx.canvas(move |v: &Phone, g, b| {
        let (top, (from, to)) = (f32::from(b.origin.y), in_view(&scroll, row));
        let first = (from / row).floor().max(0.0) as usize;
        let last = ((to / row).ceil().max(0.0) as usize).min(n);
        for i in first..last {
            let r = Bounds { origin: point(b.origin.x, px(top + i as f32 * row)), size: size(b.size.width, px(row)) };
            paint(v, g, r, i, v.hot == Some((id, i)));
        }
    });
    let at = move |y: Pixels, top: Pixels| {
        let i = (f32::from(y - top) / row).floor();
        (i >= 0.0 && (i as usize) < n).then_some(i as usize)
    };
    div()
        .id(id)
        .w_full()
        .h(px(n as f32 * row))
        .flex_shrink_0()
        .on_click(cx.listener(move |v: &mut Phone, e: &ClickEvent, cx| {
            if e.standard_click()
                && let Some(i) = at(e.position().y, cx.listener_bounds().origin.y)
            {
                pick(v, i, cx);
            }
        }))
        .on_mouse_move(cx.listener(move |v: &mut Phone, e: &MouseMoveEvent, cx| {
            let now = at(e.position.y, cx.listener_bounds().origin.y).map(|i| (id, i));
            if v.hot != now {
                v.hot = now;
                cx.invalidate();
            }
        }))
        .on_hover(cx.listener(move |v: &mut Phone, on: &bool, cx| {
            if !*on && v.hot.is_some_and(|(list, _)| list == id) {
                v.hot = None;
                cx.invalidate();
            }
        }))
        .child(drawn.size_full())
}

/// A list row's colours: its text, its quieter text, and its ground
/// painted (chosen, or under the pointer).
fn row_ground(g: &mut Painter, r: Bounds<Pixels>, selected: bool, hot: bool) -> (Hsla, Hsla) {
    let p = theme::palette();
    if selected {
        style::fill_rect(g, r, c(p.list_selected_background));
        let t = c(p.list_selected_item_text);
        return (t, t.opacity(0.62));
    }
    let ink = c(p.list_item_text);
    if hot {
        style::fill_rect(g, r, ink.opacity(0.05));
    }
    (ink, c(olo_kit::palette::mix(p.list_item_text, p.list_background, 0.45)))
}

impl Phone {
    /// The list's side: Messages or Contacts, a search, and the rows.
    fn left(&mut self, _width: f32, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let unread: usize = self.threads.iter().map(Conversation::unread).sum();
        let messages = if unread > 0 { format!("Messages ({unread})") } else { "Messages".to_owned() };
        let tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .child(Button::new(messages).id("tab-messages").small().pushed(self.tab == Tab::Messages).on_click(cx.listener(|v: &mut Phone, _: &ClickEvent, cx| {
                v.tab = Tab::Messages;
                cx.invalidate();
            })))
            .child(Button::new("Contacts").id("tab-contacts").small().pushed(self.tab == Tab::Contacts).on_click(cx.listener(|v: &mut Phone, _: &ClickEvent, cx| {
                v.tab = Tab::Contacts;
                cx.invalidate();
            })))
            .child(div().flex_1())
            .child(Button::new("New").id("new-message").small().enabled(self.state.paired() || self.demo).on_click(cx.listener(|v: &mut Phone, _: &ClickEvent, cx| {
                v.command("new-message", cx);
            })));
        // (The buttons in a band as tall as the conversation's header
        // beside it; the search under.)
        let top = div()
            .flex()
            .flex_col()
            .px(px(m::SPACING))
            .pb(px(m::SPACING))
            .child(div().h(px(HEADER)).flex().flex_col().justify_center().child(tabs))
            .child(div().h(px(m::CONTROL_HEIGHT)).child(&self.search));
        let (handle, content) = match self.tab {
            Tab::Messages => (self.list_scroll.clone(), self.thread_rows(cx)),
            Tab::Contacts => (self.people_scroll.clone(), self.people_rows(cx)),
        };
        let list = ScrollView::new(&handle).border(false).auto_hide(true).background(p.list_background).on_scroll(cx.listener(|_: &mut Phone, _, cx| cx.invalidate())).child(content);
        let mut column = div().size_full().flex().flex_col().bg(c(p.panel_background)).child(top).child(div().flex_1().min_h_0().child(list));
        if let Some(playing) = self.playing(cx) {
            column = column.child(playing);
        }
        column.into_element()
    }

    /// Under the list while the phone plays or has played something:
    /// what, and its remote control (the phone is the player; BlueZ's
    /// own remote control of it, through the service).
    fn playing(&mut self, cx: &mut Cx<Self>) -> Option<Element> {
        let music = self.state.music.clone()?;
        let p = theme::palette();
        let press = |id: &'static str, label: &'static str, command: &'static str, cx: &mut Cx<Self>| {
            Button::new(label).id(id).small().on_click(cx.listener(move |v: &mut Phone, _: &ClickEvent, cx| {
                v.command(command, cx);
            }))
        };
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .child(press("music-previous", "Previous", "music:previous", cx))
            .child(press("music-toggle", if music.playing { "Pause" } else { "Play" }, "music:toggle", cx))
            .child(press("music-next", "Next", "music:next", cx));
        let mut words = div().flex().flex_col().min_w_0().child(StringView::new(music.title.clone()).cut(true).bold(true));
        if !music.artist.is_empty() {
            words = words.child(StringView::new(music.artist.clone()).cut(true).dim(true).small(true));
        }
        // (The window's own edge under it: its inset, whole at every
        // scale.)
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .px(px(m::SPACING))
                .pt(px(m::SPACING))
                .pb(px(m::WINDOW_INSET))
                .border_t_1()
                .border_color(c(p.shadow).opacity(0.55))
                .child(words)
                .child(buttons)
                .into_element(),
        )
    }

    fn thread_rows(&mut self, cx: &mut Cx<Self>) -> Element {
        let listed = self.listed(cx);
        if listed.is_empty() {
            let words = if !self.threads.is_empty() {
                "No conversation has that."
            } else if self.state.link == Link::Ready || self.demo {
                "No messages yet."
            } else {
                "The phone’s messages show here once it’s connected."
            };
            return div().p(px(m::SPACING)).child(StringView::new(words).dim(true).wrap(true)).into_element();
        }
        let (paint_list, pick_list) = (listed.clone(), listed.clone());
        let scroll = self.list_scroll.clone();
        rows(
            cx,
            "threads",
            &scroll,
            listed.len(),
            ROW,
            move |v, g, r, i, hot| {
                if let Some(t) = paint_list.get(i).and_then(|&k| v.threads.get(k)) {
                    v.thread_row(g, r, t, hot);
                }
            },
            move |v, i, cx| {
                if let Some(key) = pick_list.get(i).and_then(|&k| v.threads.get(k)).map(|t| t.key.clone()) {
                    v.open_thread(&key, cx);
                }
            },
        )
    }

    fn thread_row(&self, g: &mut Painter, r: Bounds<Pixels>, t: &Conversation, hot: bool) {
        let p = theme::palette();
        let selected = matches!(&self.shown, Shown::Thread(key) if *key == t.key);
        let (ink, dim) = row_ground(g, r, selected, hot);
        let pic = Bounds { origin: point(r.origin.x + px(10.0), r.center().y - px(PICTURE / 2.0)), size: size(px(PICTURE), px(PICTURE)) };
        picture(g, pic, &t.name, &t.address, p.mode == olo_kit::palette::ThemeMode::Dark);
        let (x, right) = (pic.right() + px(12.0), r.right() - px(12.0));
        let (title, small) = (style::PLAIN_SIZE + 0.5, style::DETAIL_SIZE);
        let (bold, normal) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
        let base1 = style::baseline_at(g, r.origin.y + px(21.0), title);
        let base2 = style::baseline_at(g, r.origin.y + px(43.0), small);
        // When, then the name in what's left of the line.
        let now = when::now();
        let at = if t.latest() > 0 { when::short(t.latest(), now, self.h24) } else { String::new() };
        let tw = style::text_width(at.clone(), small, normal.clone());
        style::label_on_baseline(g, right - tw, base1, Align::Left, at, small, normal.clone(), dim);
        style::label_on_baseline_fit(g, x, base1, Align::Left, t.name.clone(), title, bold.clone(), ink, right - tw - px(8.0) - x);
        // How many unread, then their latest words in what's left.
        let unread = t.unread();
        let mut room = right - x;
        if unread > 0 {
            let count = unread.to_string();
            let w = style::pill_width(&count, small, &bold);
            style::pill_on_baseline(g, right - px(w), base2, &count, small, bold.clone(), c(p.keyboard_navigation), white());
            room -= px(w + 8.0);
        }
        let last = t.messages.last();
        let words = last.map(|m| format!("{}{}", if m.outgoing { "You: " } else { "" }, one_line(&m.text))).unwrap_or_default();
        let words_ink = if unread > 0 && !selected { ink } else { dim };
        style::label_on_baseline_fit(g, x, base2, Align::Left, words.clone(), small, normal, words_ink, room);
        if g.content_mask().bounds.intersects(&r) {
            let name = if unread > 0 { format!("{}, {unread} unread", t.name) } else { t.name.clone() };
            agent::record(g, agent::Node { value: Some(words.into()), selected: Some(selected), ..agent::Node::button(name, r) });
        }
    }

    fn people_rows(&mut self, cx: &mut Cx<Self>) -> Element {
        let listed = self.people(cx);
        let typed = self.search.read(cx).text(cx).trim().to_owned();
        // A number or an address typed that's nobody's: a message to it.
        let direct = (writable(&typed) && self.name_of(&typed).is_none()).then_some(typed);
        if listed.is_empty() && direct.is_none() {
            let words = if !self.contacts.is_empty() {
                "No contact has that. A number or an address typed here can be written to."
            } else if self.state.contacts || self.demo {
                "The phone has no contacts."
            } else if self.state.kind == Kind::IPhone {
                "The phone’s contacts show here once it shares them (Sync Contacts, in its Bluetooth settings for this computer). A number typed above can be written to meanwhile."
            } else {
                "The phone’s contacts show here once it shares them. A number typed above can be written to meanwhile."
            };
            return div().p(px(m::SPACING)).child(StringView::new(words).dim(true).wrap(true)).into_element();
        }
        let lead = usize::from(direct.is_some());
        let (paint_list, pick_list) = (listed.clone(), listed.clone());
        let (paint_direct, pick_direct) = (direct.clone(), direct);
        let scroll = self.people_scroll.clone();
        rows(
            cx,
            "people",
            &scroll,
            listed.len() + lead,
            PERSON_ROW,
            move |v, g, r, i, hot| match (&paint_direct, i) {
                (Some(address), 0) => v.person_row(g, r, &format!("Message {address}"), "A number that’s nobody’s yet", address, false, hot),
                _ => {
                    if let Some(person) = paint_list.get(i - lead).and_then(|&k| v.contacts.get(k)) {
                        let first = person.phones.first().or(person.emails.first()).cloned().unwrap_or_default();
                        let more = person.phones.len() + person.emails.len();
                        let under = if more > 1 { format!("{first} and {} more", more - 1) } else { first.clone() };
                        let selected = matches!(&v.shown, Shown::Person(name) if *name == person.name);
                        v.person_row(g, r, &person.name, &under, &first, selected, hot);
                    }
                }
            },
            move |v, i, cx| match (&pick_direct, i) {
                (Some(address), 0) => v.write_to(address, cx),
                _ => {
                    let Some(person) = pick_list.get(i - lead).and_then(|&k| v.contacts.get(k)).cloned() else { return };
                    // One way to reach them: a message; several: their card.
                    match (person.phones.len() + person.emails.len(), person.phones.first().or(person.emails.first())) {
                        (1, Some(only)) => v.write_to(only, cx),
                        _ => {
                            v.shown = Shown::Person(person.name);
                            cx.invalidate();
                        }
                    }
                }
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn person_row(&self, g: &mut Painter, r: Bounds<Pixels>, name: &str, under: &str, address: &str, selected: bool, hot: bool) {
        let p = theme::palette();
        let (ink, dim) = row_ground(g, r, selected, hot);
        let d = 32.0;
        let pic = Bounds { origin: point(r.origin.x + px(12.0), r.center().y - px(d / 2.0)), size: size(px(d), px(d)) };
        picture(g, pic, name, address, p.mode == olo_kit::palette::ThemeMode::Dark);
        let (x, right) = (pic.right() + px(12.0), r.right() - px(12.0));
        let base1 = style::baseline_at(g, r.origin.y + px(16.0), style::PLAIN_SIZE);
        let base2 = style::baseline_at(g, r.origin.y + px(33.0), style::DETAIL_SIZE);
        style::label_on_baseline_fit(g, x, base1, Align::Left, name.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, right - x);
        style::label_on_baseline_fit(g, x, base2, Align::Left, under.to_owned(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim, right - x);
        if g.content_mask().bounds.intersects(&r) {
            agent::record(g, agent::Node { value: Some(under.to_owned().into()), selected: Some(selected), ..agent::Node::button(name.to_owned(), r) });
        }
    }

    /// The window's right: the conversation and its field, a contact's
    /// card, or what to do first.
    fn right(&mut self, width: f32, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let ground = style::card_ink(&p).gutter;
        let settings = |_: &ClickEvent, _: &mut olo_kit::WindowCx| olo_kit::app_menu::open_settings("phone");
        // No phone yet, or no service to ask: what to do.
        if !self.demo && !self.state.paired() {
            let (text, detail) = if self.state.pid == 0 {
                ("The phone service isn’t running", "Olo’s phone service starts with your session; Olo Doctor says why it isn’t running.")
            } else {
                ("No phone connected", "Connect an iPhone or an Android phone to read and answer its messages here, and to have the one-time codes texted to you offered where you sign in.")
            };
            return div().size_full().bg(ground).child(EmptyState::new(text).icon("phone").detail(detail).action("Connect a Phone…", settings)).into_element();
        }
        let content = match self.shown.clone() {
            Shown::Person(name) => self.card(&name, cx),
            Shown::Nothing => {
                div().size_full().child(EmptyState::new("No conversation chosen").icon(self.state.picture()).detail("Choose a conversation from the list, or start one with New.")).into_element()
            }
            Shown::Thread(_) | Shown::New(_) => self.talk(width, cx),
        };
        // How the phone stands, while it isn't simply connected.
        let notice = (!self.demo && self.state.link != Link::Ready).then(|| {
            let words = match (self.state.link, self.state.trouble.is_empty()) {
                (Link::Allow, _) => format!("“{}” hasn’t allowed this computer its messages yet.", self.state.name),
                (Link::Connecting, _) => format!("Connecting to “{}”…", self.state.name),
                (_, false) => self.state.trouble.clone(),
                _ => format!("“{}” is out of reach. It’s tried again by itself.", self.state.name),
            };
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .px(px(m::SPACING))
                .py(px(m::TIGHT))
                .bg(c(olo_kit::palette::mix(p.panel_background, p.warning, 0.18)))
                .child(div().flex_1().min_w_0().child(StringView::new(words).wrap(true)));
            row = match self.state.link {
                Link::Allow => row.child(Button::new("Phone Settings…").id("notice-settings").small().on_click(settings)),
                Link::Connecting => row,
                _ => row.child(Button::new("Try Again").id("notice-again").small().on_click(cx.listener(|v: &mut Phone, _: &ClickEvent, cx| {
                    v.command("reconnect", cx);
                }))),
            };
            row
        });
        let mut column = div().size_full().flex().flex_col().bg(ground);
        if let Some(notice) = notice {
            column = column.child(notice);
        }
        column.child(div().flex_1().min_h_0().child(content)).into_element()
    }

    /// A contact's card: who, and each way to write to them.
    fn card(&mut self, name: &str, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let Some(person) = self.contacts.iter().find(|p| p.name == name).cloned() else {
            return div().size_full().child(EmptyState::new("No such contact").icon("phone")).into_element();
        };
        let dark = p.mode == olo_kit::palette::ThemeMode::Dark;
        let (who, first) = (person.name.clone(), person.phones.first().or(person.emails.first()).cloned().unwrap_or_default());
        let face = canvas(move |g, b: Bounds<Pixels>| {
            let r = style::snap_centred(g, b.center(), 72.0, 72.0);
            picture(g, r, &who, &first, dark);
        })
        .w(px(72.0))
        .h(px(72.0))
        .flex_shrink_0();
        let mut ways = div().w_full().flex().flex_col().gap(px(m::SPACING));
        for (i, address) in person.phones.iter().chain(&person.emails).enumerate() {
            let (to, number) = (address.clone(), address.clone());
            let mut way = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(address.clone()).cut(true)));
            // (A number can be called too: through the phone, the call's
            // sound here.)
            if i < person.phones.len() {
                way = way.child(Button::new("Call").id(format!("call-{i}")).small().on_click(cx.listener(move |v: &mut Phone, _: &ClickEvent, cx| {
                    v.command(&format!("call:{number}"), cx);
                })));
            }
            ways = ways.child(way.child(Button::new("Message").id(format!("message-{i}")).small().on_click(cx.listener(move |v: &mut Phone, _: &ClickEvent, cx| {
                v.write_to(&to, cx);
            }))));
        }
        let column = div()
            .w(px(360.0))
            .max_w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(m::SPACING))
            .child(face)
            .child(olo_kit::widgets::heading(person.name.clone()))
            .child(div().h(px(m::TIGHT)))
            .child(ways);
        div().size_full().flex().flex_col().items_center().pt(px(m::WINDOW_INSET * 3.0)).px(px(m::WINDOW_INSET)).child(column).into_element()
    }

    /// The width a message's text wraps at in a pane `width` wide: the
    /// widest a balloon gets, or a share of a narrow pane, in steps (a
    /// pane dragged narrower wraps again every few points, not at each).
    fn text_width(width: f32) -> f32 {
        let widest = ((width - olo_kit::widgets::scroll::SCROLL_BAR) * SHARE).min(WIDEST).max(120.0);
        ((widest - 2.0 * PAD.0) / 16.0).floor() * 16.0
    }

    /// Shape message `i` of conversation `t` (if it isn't).
    fn shape(shaped: &mut Shaped, t: &Conversation, i: usize, text_w: f32, ink: Hsla, h24: bool) {
        let (Some(msg), Some(None)) = (t.messages.get(i), shaped.text.get(i)) else { return };
        let font = style::ui_font(FontWeight::NORMAL);
        let time: SharedString = when::time(msg.at, h24).into();
        shaped.text[i] = Some(Wrapped {
            text: Rc::new(Para::plain(&msg.text, TEXT, font.clone(), ink, text_w, None)),
            code: (!msg.outgoing).then(|| codes::find(&msg.text)).flatten().map(|found| found.code),
            time_w: f32::from(style::text_width(time.clone(), META, font)) + 10.0,
            time,
        });
        shaped.count += 1;
    }

    /// Where everything of conversation `t` goes, from what's shaped
    /// (and guesses for what isn't): the items, and how tall it all is.
    /// Arithmetic alone: nothing is shaped here.
    fn arrange(&self, t: &Conversation, text_w: f32, shaped: &Shaped, now: i64) -> (Vec<Item>, f32) {
        let small = para::line_height(META);
        let line = para::line_height(TEXT);
        let (mut items, mut y, mut day) = (Vec::with_capacity(t.messages.len() + 8), 12.0, None);
        for (i, msg) in t.messages.iter().enumerate() {
            let this_day = shaped.days.get(i).copied().unwrap_or_default();
            if day != Some(this_day) {
                day = Some(this_day);
                y += if items.is_empty() { 0.0 } else { 8.0 };
                items.push(Item::Day(y, when::day(msg.at, now).into()));
                y += 30.0;
            }
            let run = |j: usize| t.messages.get(j).is_some_and(|o: &Message| o.outgoing == msg.outgoing && (o.at - msg.at).abs() <= RUN && shaped.days.get(j) == Some(&this_day));
            let (joins_above, joins_below) = (i > 0 && run(i - 1), run(i + 1));
            y += if joins_above { 2.0 } else { 8.0 };
            let balloon = match shaped.text.get(i).and_then(Option::as_ref) {
                Some(wr) => {
                    let (last_right, _) = wr.text.last_line();
                    // The time beside the last line where there's room,
                    // else under.
                    let time_under = last_right + wr.time_w > text_w;
                    let w = (if time_under { wr.text.w.max(wr.time_w - 10.0) } else { wr.text.w.max(last_right + wr.time_w) }).min(text_w) + 2.0 * PAD.0;
                    let h = wr.text.h + if time_under { small } else { 0.0 } + 2.0 * PAD.1;
                    Balloon { y, w, h, ours: msg.outgoing, text: Some(wr.text.clone()), time: wr.time.clone(), time_under, joins_above, joins_below, code: wr.code.clone(), index: i }
                }
                None => {
                    // As it was at another width, or by its length.
                    let (w, h) = shaped.was.get(i).copied().flatten().unwrap_or_else(|| {
                        let chars = msg.text.chars().count() as f32 * 6.6 + 62.0;
                        let lines = (chars / text_w).ceil().max(1.0) + msg.text.matches('\n').count() as f32;
                        (chars.min(text_w) + 2.0 * PAD.0, lines * line + 2.0 * PAD.1)
                    });
                    Balloon { y, w: w.min(text_w + 2.0 * PAD.0), h, ours: msg.outgoing, text: None, time: SharedString::default(), time_under: false, joins_above, joins_below, code: None, index: i }
                }
            };
            y += balloon.h + if balloon.code.is_some() { 26.0 } else { 0.0 };
            items.push(Item::Balloon(balloon));
        }
        (items, y + 12.0)
    }

    /// The conversation shown, laid out for a pane `width` wide (a
    /// heading for each day, each message a balloon) into `laid`: its
    /// height. Laid out again only when it, the width, the look, the
    /// clock's style or the day changed, or more of it was shaped: a
    /// frame otherwise costs it nothing. What's in view is shaped here;
    /// the rest a little a frame (`shape_more`).
    fn lay(&mut self, width: f32, cx: &mut Cx<Self>) -> f32 {
        self.talk_width = width;
        let Some(t) = self.thread() else {
            self.laid.borrow_mut().clear();
            *self.laid_for.borrow_mut() = None;
            return 0.0;
        };
        let p = theme::palette();
        let ink = style::card_ink(&p).text;
        let text_w = Self::text_width(width);
        let look = style::key((p.document_text, p.mode == olo_kit::palette::ThemeMode::Dark));
        let now = when::now();
        let mut shaped = self.shaped.borrow_mut();
        let of = (t.key.clone(), text_w as u32, look, self.h24);
        if shaped.of != of {
            // (What it was is what's guessed by until it's shaped again;
            // another conversation's says nothing of this one.)
            let same = shaped.of.0 == of.0;
            let mut was = Vec::new();
            if same {
                was.resize(shaped.ids.len(), None);
                for item in self.laid.borrow().iter() {
                    if let Item::Balloon(bl) = item
                        && let Some(slot) = was.get_mut(bl.index)
                    {
                        *slot = Some((bl.w, bl.h));
                    }
                }
            }
            let (ids, days) = if same { (std::mem::take(&mut shaped.ids), std::mem::take(&mut shaped.days)) } else { (Vec::new(), Vec::new()) };
            *shaped = Shaped { of, text: vec![None; ids.len()], ids, days, count: 0, was };
        }
        shaped.hold(t);
        let laid_for: LaidFor = (t.key.clone(), t.messages.len(), t.messages.last().map(|m| (m.handle.clone(), m.read)), shaped.count, text_w as u32, look, self.h24, when::day_number(now));
        if let Some((was, height)) = &*self.laid_for.borrow()
            && *was == laid_for
        {
            return *height;
        }
        let (mut items, mut height) = self.arrange(t, text_w, &shaped, now);
        // What shows, shaped now: the foot of it where it's about to be
        // brought to its end, else where it's scrolled to.
        let view_h = f32::from(self.talk_scroll.bounds().size.height).max(400.0);
        let (from, to) = if self.to_end { (height - view_h - 200.0, height) } else { in_view(&self.talk_scroll, 200.0) };
        let showing: Vec<usize> = items
            .iter()
            .filter_map(|item| match item {
                Item::Balloon(bl) if bl.text.is_none() && bl.y + bl.h >= from && bl.y <= to => Some(bl.index),
                _ => None,
            })
            .collect();
        if !showing.is_empty() {
            for i in showing {
                Self::shape(&mut shaped, t, i, text_w, ink, self.h24);
            }
            (items, height) = self.arrange(t, text_w, &shaped, now);
        }
        let rough = shaped.count < t.messages.len();
        let laid_for: LaidFor = (laid_for.0, laid_for.1, laid_for.2, shaped.count, laid_for.4, laid_for.5, laid_for.6, laid_for.7);
        drop(shaped);
        *self.laid.borrow_mut() = items;
        *self.laid_for.borrow_mut() = Some((laid_for, height));
        if rough && !self.shaping {
            // (As the next frame starts: one slice a frame. Asked for at
            // once, they ran back to back inside one frame: 10 ms of it.)
            self.shaping = true;
            let me = cx.view_ref();
            cx.on_next_frame(move |cx| me.update(cx, |v: &mut Phone, cx| v.shape_more(cx)));
        }
        height
    }

    /// Shape some more of the conversation's messages, those nearest
    /// what shows first, for a few milliseconds; lay it out again, and
    /// keep what shows where it is (the messages above it are their true
    /// height now, not a guess). Again next frame while any are left.
    fn shape_more(&mut self, cx: &mut Cx<Self>) {
        self.shaping = false;
        let Some(t) = self.thread() else { return };
        let (from, _) = in_view(&self.talk_scroll, 0.0);
        let at_end = self.to_end || -f32::from(self.talk_scroll.offset().y) >= f32::from(self.talk_scroll.max_offset().height) - 4.0;
        // The first balloon showing: what's held where it is.
        let anchor = self.laid.borrow().iter().find_map(|item| match item {
            Item::Balloon(bl) if bl.y + bl.h >= from => Some((bl.index, bl.y)),
            _ => None,
        });
        let middle = anchor.map_or(t.messages.len().saturating_sub(1), |(i, _)| i);
        let p = theme::palette();
        let (ink, text_w) = (style::card_ink(&p).text, Self::text_width(self.talk_width));
        let began = std::time::Instant::now();
        {
            let mut shaped = self.shaped.borrow_mut();
            // (Outwards from what shows: below it, above it, in turn.)
            let n = t.messages.len();
            for step in 0..n {
                for i in [middle.checked_add(step), middle.checked_sub(step)].into_iter().flatten().filter(|i| *i < n) {
                    Self::shape(&mut shaped, t, i, text_w, ink, self.h24);
                }
                if began.elapsed() >= SHAPING {
                    break;
                }
            }
        }
        // Laid out with what's shaped now, and held in place.
        let width = self.talk_width;
        self.lay(width, cx);
        if at_end {
            self.to_end = true;
        } else if let Some((index, was)) = anchor {
            let now_at = self.laid.borrow().iter().find_map(|item| match item {
                Item::Balloon(bl) if bl.index == index => Some(bl.y),
                _ => None,
            });
            if let Some(y) = now_at.filter(|y| (y - was).abs() > 0.5) {
                let at = self.talk_scroll.offset();
                self.talk_scroll.set_offset_once_laid_out(cx, point(at.x, at.y - px(y - was)));
            }
        }
        cx.invalidate();
    }

    /// The conversation: its header, its balloons, the field.
    fn talk(&mut self, width: f32, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let dark = p.mode == olo_kit::palette::ThemeMode::Dark;
        let Some((address, name)) = self.writing_to() else {
            // (A conversation kept from before, not there now.)
            return div().size_full().child(EmptyState::new("No conversation chosen").icon("phone").detail("Choose a conversation from the list, or start one with New.")).into_element();
        };
        // The header: who, and the number.
        let (shadow, panel, ink, dim) = (c(p.shadow), c(p.panel_background), c(p.panel_text), style::secondary_text(&p));
        let (h_name, h_address) = (name.clone(), address.clone());
        let under = if named(&name) { address.clone() } else { String::new() };
        let header = canvas(move |g, b: Bounds<Pixels>| {
            style::fill_rect(g, b, panel);
            // (Its own lines: a name over a number isn't held level with
            // the list's buttons beside it.)
            olo_kit::review::own_grid(g, b);
            let pic = Bounds { origin: point(b.origin.x + px(12.0), b.center().y - px(18.0)), size: size(px(36.0), px(36.0)) };
            picture(g, pic, &h_name, &h_address, dark);
            let (x, room) = (pic.right() + px(12.0), b.right() - pic.right() - px(24.0));
            if under.is_empty() {
                style::label_fit(g, x, b.center().y, Align::Left, h_name.clone(), style::PLAIN_SIZE + 1.0, style::ui_font(FontWeight::BOLD), ink, room);
            } else {
                // (The name level with the list's buttons beside it.)
                let base1 = style::baseline_at(g, b.origin.y + px(22.0), style::PLAIN_SIZE + 1.0);
                let base2 = style::baseline_at(g, b.origin.y + px(40.0), style::DETAIL_SIZE);
                style::label_on_baseline_fit(g, x, base1, Align::Left, h_name.clone(), style::PLAIN_SIZE + 1.0, style::ui_font(FontWeight::BOLD), ink, room);
                style::label_on_baseline_fit(g, x, base2, Align::Left, under.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), dim, room);
            }
            style::hline(g, b.left(), b.right(), b.bottom() - style::hairline(g), shadow.opacity(0.5));
            agent::record(g, agent::Node { role: "Heading", value: Some(h_address.clone().into()), ..agent::Node::label(h_name.clone(), b) });
        })
        .w_full()
        .h(px(HEADER))
        .flex_shrink_0();

        // The balloons.
        let height = self.lay(width, cx);
        let laid = self.laid.clone();
        let them = name.clone();
        let look = Look::of(&p);
        let scroll = self.talk_scroll.clone();
        let balloons = cx.canvas(move |v: &Phone, g, b| {
            let Some(t) = v.thread() else { return };
            // (Its own spacing: a run's balloons sit close on purpose.)
            olo_kit::review::own_grid(g, b);
            // Those in view (they're in order down the page).
            let (from, to) = in_view(&scroll, 60.0);
            let laid = laid.borrow();
            let top_of = |item: &Item| match item {
                Item::Day(y, _) => *y,
                Item::Balloon(bl) => bl.y,
            };
            let first = laid.partition_point(|item| top_of(item) < from - 400.0);
            for item in laid[first..].iter().take_while(|item| top_of(item) <= to) {
                match item {
                    Item::Day(y, text) => {
                        let r = Bounds { origin: point(b.origin.x, b.origin.y + px(*y)), size: size(b.size.width, px(22.0)) };
                        if *y + 22.0 >= from {
                            let w = style::pill_width(text, META + 0.5, &style::ui_font(FontWeight::BOLD));
                            style::pill(g, r.center().x - px(w / 2.0), r.center().y, text, META + 0.5, style::ui_font(FontWeight::BOLD), look.day, look.day_text);
                            agent::record(g, agent::Node { role: "Heading", ..agent::Node::label(text.clone(), r) });
                        }
                    }
                    Item::Balloon(bl) => {
                        let r = balloon_at(bl, b);
                        if bl.y + bl.h + 26.0 < from {
                            continue;
                        }
                        look.paint(g, bl, r);
                        if let Some(code) = &bl.code {
                            let chip = code_chip(bl, b);
                            let words = format!("Copy {code}");
                            style::pill(g, chip.origin.x, chip.center().y, &words, META + 0.5, style::ui_font(FontWeight::BOLD), look.chip, look.chip_text);
                            agent::record(g, agent::Node::button(words, chip));
                        }
                        if let Some(m) = t.messages.get(bl.index) {
                            let who = if m.outgoing { "Me" } else { them.as_str() };
                            agent::record(g, agent::Node { role: "ListItem", value: Some(bl.time.clone()), ..agent::Node::label(format!("{who}: {}", m.text), r) });
                        }
                    }
                }
            }
        });
        let (click_laid, menu_laid) = (self.laid.clone(), self.laid.clone());
        let talk = div()
            .id("balloons")
            .w_full()
            .h(px(height))
            .flex_shrink_0()
            // A code's Copy.
            .on_click(cx.listener(move |v: &mut Phone, e: &ClickEvent, cx| {
                if !e.standard_click() {
                    return;
                }
                let b = cx.listener_bounds();
                let hit = click_laid.borrow().iter().find_map(|item| match item {
                    Item::Balloon(bl) if bl.code.is_some() && code_chip(bl, b).contains(&e.position()) => bl.code.clone(),
                    _ => None,
                });
                if let Some(code) = hit {
                    olo_kit::clipboard::write(ClipboardItem::new_string(code));
                    v.trouble = None;
                    cx.invalidate();
                }
            }))
            // A balloon's own menu: its words copied.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v: &mut Phone, e: &MouseDownEvent, cx| {
                    let b = cx.listener_bounds();
                    let hit = menu_laid.borrow().iter().find_map(|item| match item {
                        Item::Balloon(bl) if balloon_at(bl, b).contains(&e.position) => Some((bl.index, bl.code.clone())),
                        _ => None,
                    });
                    let Some((index, code)) = hit else { return };
                    let Some(text) = v.thread().and_then(|t| t.messages.get(index)).map(|m| m.text.clone()) else { return };
                    let mut menu = Menu::new("").item("Copy", "copy");
                    if code.is_some() {
                        menu = menu.item("Copy Code", "copy-code");
                    }
                    olo_kit::menu::context_menu(cx, menu, e.position, move |command, _| {
                        let what = if command == "copy-code" { code.clone().unwrap_or_default() } else { text.clone() };
                        olo_kit::clipboard::write(ClipboardItem::new_string(what));
                    });
                }),
            )
            .child(balloons.size_full());
        let inner: Element = if height > 24.0 {
            talk.into_element()
        } else {
            div()
                .p(px(m::WINDOW_INSET))
                .child(StringView::new(format!("No messages with {name} yet. What you write below goes as a text message from your phone.")).dim(true).wrap(true))
                .into_element()
        };
        let scroller = ScrollView::new(&self.talk_scroll).border(false).auto_hide(true).background(style::card_paper(&p).0).on_scroll(cx.listener(|_: &mut Phone, _, cx| cx.invalidate())).child(inner);
        if std::mem::take(&mut self.to_end) {
            // (Once it's laid out at its new height.)
            let handle = self.talk_scroll.clone();
            cx.after_do(Duration::ZERO, move |_: &mut Phone, cx| {
                handle.scroll_to_bottom(cx);
                cx.invalidate();
            });
        }

        // The field, as tall as its lines (six at most), and Send.
        let field_w = (width - 2.0 * m::WINDOW_INSET - SEND - m::SPACING - 28.0).max(80.0);
        let lines = self.draft.read(cx).rows_at(px(field_w)).min(6);
        let line = f32::from(TextField::line_height_for(cx.scale_factor(), TEXT));
        let field_h = (lines as f32 * line + 16.0).max(SEND);
        let can_send = !self.sending && !self.draft.read(cx).text().trim().is_empty() && (self.demo || self.state.link == Link::Ready);
        let (well, edge) = (c(p.document_background), c(p.shadow).opacity(0.55));
        let field = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(field_h))
            .child(
                canvas(move |g, b: Bounds<Pixels>| {
                    let r = style::snap(g, b);
                    g.paint_quad(quad(r, Corners::all(px(SEND / 2.0)), well, style::hairline(g), edge, BorderStyle::Solid));
                })
                .absolute()
                .inset_0(),
            )
            .child(div().size_full().px(px(14.0)).py(px(8.0)).child(&self.draft));
        let accent = c(p.keyboard_navigation);
        let sending = self.sending;
        let send = div()
            .id("send")
            .w(px(SEND))
            .h(px(SEND))
            .flex_shrink_0()
            .cursor(if can_send { CursorStyle::PointingHand } else { CursorStyle::Arrow })
            .on_click(cx.listener(move |v: &mut Phone, e: &ClickEvent, cx| {
                if e.standard_click() && can_send {
                    v.send_typed(cx);
                }
            }))
            .child(
                canvas(move |g, b: Bounds<Pixels>| {
                    let r = style::snap_centred(g, b.center(), SEND, SEND);
                    let cc = r.center();
                    let fill = if can_send { accent } else { accent.opacity(0.35) };
                    g.paint_sector(cc, px(0.0), px(SEND / 2.0), 0.0, std::f32::consts::TAU, fill);
                    // An arrow up.
                    let at = |dx: f32, dy: f32| point(cc.x + px(dx), cc.y + px(dy));
                    g.paint_segments(&[(at(0.0, 6.5), at(0.0, -6.0)), (at(-5.5, -0.5), at(0.0, -6.0)), (at(5.5, -0.5), at(0.0, -6.0))], px(2.0), true, white());
                    let mut node = agent::Node::button(if sending { "Sending…" } else { "Send" }, r);
                    node.enabled = can_send;
                    agent::record(g, node);
                })
                .size_full(),
            );
        let mut bottom = div().flex().flex_col().gap(px(m::TIGHT)).px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).pb(px(m::WINDOW_INSET)).bg(c(p.panel_background)).flex_shrink_0();
        if let Some(why) = &self.trouble {
            bottom = bottom.child(StringView::new(format!("Not sent: {why}")).color(c(p.failure)).wrap(true));
        }
        bottom = bottom.child(div().flex().flex_row().items_end().gap(px(m::SPACING)).child(field).child(send));
        let rule = canvas(move |g, b| style::hline(g, b.left(), b.right(), b.top(), shadow.opacity(0.5))).w_full().h(px(0.0)).flex_shrink_0();
        div().size_full().flex().flex_col().child(header).child(div().flex_1().min_h_0().child(scroller)).child(rule).child(bottom).into_element()
    }
}

/// Where a balloon is in the conversation's drawing `b`: ours to its
/// right, theirs to its left.
fn balloon_at(bl: &Balloon, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let x = if bl.ours { b.right() - px(14.0 + bl.w) } else { b.origin.x + px(14.0) };
    Bounds { origin: point(x, b.origin.y + px(bl.y)), size: size(px(bl.w), px(bl.h)) }
}

/// The Copy button under a balloon with a code.
fn code_chip(bl: &Balloon, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let r = balloon_at(bl, b);
    let words = format!("Copy {}", bl.code.as_deref().unwrap_or_default());
    let w = style::pill_width(&words, META + 0.5, &style::ui_font(FontWeight::BOLD));
    Bounds { origin: point(r.origin.x + px(2.0), r.bottom() + px(4.0)), size: size(px(w), px(style::pill_height(META + 0.5))) }
}

/// A conversation's colours.
#[derive(Clone)]
struct Look {
    theirs: Hsla,
    theirs_edge: Hsla,
    ours: Hsla,
    ours_edge: Hsla,
    dim: Hsla,
    lift: Hsla,
    day: Hsla,
    day_text: Hsla,
    chip: Hsla,
    chip_text: Hsla,
}

impl Look {
    fn of(p: &olo_kit::palette::Palette) -> Look {
        let ink = style::card_ink(p);
        let dark = p.mode == olo_kit::palette::ThemeMode::Dark;
        let (_, card) = style::card_paper(p);
        let ours = olo_kit::palette::mix(card, p.keyboard_navigation, if dark { 0.30 } else { 0.15 });
        Look {
            theirs: ink.card,
            theirs_edge: ink.edge,
            ours: c(ours),
            ours_edge: c(olo_kit::palette::mix(ours, p.keyboard_navigation, 0.35)),
            dim: ink.dim,
            lift: hsla(0.0, 0.0, 0.0, if dark { 50.0 } else { 14.0 } / 255.0),
            day: c(olo_kit::palette::mix(p.document_text, p.document_background, if dark { 0.75 } else { 0.62 })).opacity(0.85),
            day_text: if dark { c([235, 235, 235, 255]) } else { white() },
            chip: c(p.keyboard_navigation),
            chip_text: white(),
        }
    }

    /// A balloon at `r`: its corners full, or small where it joins its
    /// neighbour on the side it keeps to; a soft lift under it; its
    /// text, and its time.
    fn paint(&self, g: &mut Painter, bl: &Balloon, r: Bounds<Pixels>) {
        let (full, small) = (px(RADIUS), px(JOINED));
        let (above, below) = (if bl.joins_above { small } else { full }, if bl.joins_below { small } else { full });
        let corners = if bl.ours {
            Corners { top_left: full, top_right: above, bottom_left: full, bottom_right: below }
        } else {
            Corners { top_left: above, top_right: full, bottom_left: below, bottom_right: full }
        };
        let br = style::snap(g, r);
        let s = g.scale_factor();
        let lift = Bounds { origin: point(br.origin.x, br.origin.y + px(s.round().max(1.0) / s)), size: br.size };
        g.paint_quad(quad(lift, corners, self.lift, px(0.0), transparent_black(), BorderStyle::Solid));
        let (face, edge) = if bl.ours { (self.ours, self.ours_edge) } else { (self.theirs, self.theirs_edge) };
        g.paint_quad(quad(br, corners, face, style::hairline(g), edge, BorderStyle::Solid));
        // (Not shaped yet: its balloon alone, for the frame or two it takes.)
        let Some(text) = &bl.text else { return };
        text.paint(g, point(br.origin.x + px(PAD.0), br.origin.y + px(PAD.1)));
        // Its time: at the balloon's foot, on the right.
        let small_h = para::line_height(META);
        let cy = br.bottom() - px(PAD.1 + small_h / 2.0) + px(if bl.time_under { 0.0 } else { 1.0 });
        style::label(g, br.right() - px(PAD.0), cy, Align::Right, bl.time.clone(), META, style::ui_font(FontWeight::NORMAL), self.dim);
    }
}
