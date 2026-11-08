//! A pretend account (`OLO_TELEGRAM_DEMO=1`), answering the app as the
//! real one would, with no network: for headless tests, screenshots and
//! the bench. People, groups, a channel, a forum, folders and archived
//! chats; messages with replies, forwards, edits, reactions, mentions,
//! photos, voice messages, files, stickers and polls. Sending works (it's
//! read a moment later, and the people answer); `OLO_TELEGRAM_DEMO=N`
//! with a number adds that many more chats (thousands, for the bench),
//! and `OLO_TELEGRAM_DEMO_EVERY=S` has a message arrive every S seconds.

use olo_sync::Mutex;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    backend::{Cmd, Event, Pic, Sink, Status},
    model::*,
};

const ME: i64 = 1000;
const SAVED: ChatId = ME;
const MIRA: ChatId = 1001;
const THEO: ChatId = 1002;
const BOT: ChatId = 1003;
const JUN: ChatId = 1004;
const ELIF: ChatId = 1005;
const CREW: ChatId = -501;
const FAMILY: ChatId = -502;
const RELEASES: ChatId = -1_000_000_000_601;
const RUST: ChatId = -1_000_000_000_602;
const OLDPROJ: ChatId = -503;
const NEWS: ChatId = -1_000_000_000_604;
/// Two people who wrote first, never written back to: unknown senders.
const STRANGER: ChatId = 1_500_001;
const STRANGER2: ChatId = 1_500_002;
/// Generated chats start here.
const MANY: ChatId = 2_000_000;

struct Demo {
    now: i64,
    chats: HashMap<ChatId, Chat>,
    msgs: HashMap<ChatId, Vec<Message>>,
    next_id: MsgId,
    sink: Sink,
    pictures: PathBuf,
    /// Changes "on their way" (`OLO_TELEGRAM_DEMO_SEND_MS`: each takes
    /// that long, to see the sync line and quitting wait), of how many.
    sending: (usize, usize),
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn person(id: i64, name: &str) -> Sender {
    Sender { id, name: name.into(), photo: None }
}

fn me() -> Sender {
    person(ME, "You")
}

/// Messages written out: (minutes ago, sender, text) and extras.
struct Script {
    msgs: Vec<Message>,
    t: i64,
    chat: ChatId,
    id: MsgId,
}

impl Script {
    fn new(chat: ChatId, t: i64) -> Script {
        Script { msgs: Vec::new(), t, chat, id: 1 }
    }

    fn say(&mut self, ago_min: i64, from: Option<Sender>, text: &str) -> &mut Message {
        let out = from.as_ref().is_some_and(|f| f.id == ME);
        self.msgs.push(Message { id: self.id, chat: self.chat, from, date: self.t - ago_min * 60, out, text: text.into(), ..Message::default() });
        self.id += 1;
        let m = self.msgs.last_mut().unwrap();
        m.entities = auto_entities(&m.text);
        m
    }
}

/// Links, mentions and hashtags in `text`, as Telegram marks them.
fn auto_entities(text: &str) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut i = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let w = word.trim_end();
        let w = w.trim_end_matches([',', '.', '!', '?', ')', ':']);
        let kind = if w.starts_with("https://") || w.starts_with("http://") {
            Some(EntityKind::Url)
        } else if w.starts_with('@') && w.len() > 1 {
            Some(EntityKind::Mention)
        } else if w.starts_with('#') && w.len() > 1 {
            Some(EntityKind::Hashtag)
        } else if w.starts_with('/') && w.len() > 1 && w[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Some(EntityKind::BotCommand)
        } else {
            None
        };
        if let Some(kind) = kind {
            out.push(Entity { start: i, end: i + w.len(), kind });
        }
        i += word.len();
    }
    out
}

fn mark(m: &mut Message, word: &str, kind: EntityKind) {
    if let Some(s) = m.text.find(word) {
        m.entities.push(Entity { start: s, end: s + word.len(), kind });
        m.entities.sort_by_key(|e| e.start);
    }
}

fn react(m: &mut Message, list: &[(&str, u32, bool)]) {
    m.reactions = list.iter().map(|(e, n, mine)| Reaction { emoji: (*e).into(), count: *n, mine: *mine }).collect();
}

fn chat(id: ChatId, kind: ChatKind, title: &str) -> Chat {
    Chat { id, kind, title: title.into(), contact: kind == ChatKind::User, ..Chat::default() }
}

impl Demo {
    fn new(sink: Sink) -> Demo {
        let t = now();
        let pictures = crate::account::dir().join("demo");
        let mut d = Demo { now: t, chats: HashMap::new(), msgs: HashMap::new(), next_id: 10_000, sink, pictures, sending: (0, 0) };
        let mira = || Some(person(MIRA, "Mira Okafor"));
        let theo = || Some(person(THEO, "Theo Brandt"));
        let jun = || Some(person(JUN, "Jun Park"));
        let elif = || Some(person(ELIF, "Elif Demir"));
        let you = || Some(me());

        // Saved Messages.
        let mut s = Script::new(SAVED, t);
        s.say(60 * 30, you(), "Ideas for the cabin trip: kayaks, the long trail, star map app");
        let m = s.say(60 * 26, you(), "");
        m.media = Some(Media::File { name: "Packing list.pdf".into(), size: 184_320, mime: "application/pdf".into() });
        s.say(60 * 3, you(), "Remember: renew the passport before March");
        d.add(Chat { pinned: true, pin_order: 0, ..chat(SAVED, ChatKind::Saved, "Saved Messages") }, s.msgs);

        // Mira: a conversation with a photo, replies and reactions.
        let mut s = Script::new(MIRA, t);
        s.say(60 * 26, mira(), "Did you see the sunrise this morning? 🌅");
        s.say(60 * 26 - 2, you(), "I slept right through it. Was it good?");
        let m = s.say(60 * 26 - 3, mira(), "Unreal. I took this from the ridge");
        m.media = Some(Media::Photo { w: 1280, h: 853, thumb: None });
        react(m, &[("😍", 1, true)]);
        let m = s.say(60 * 25, you(), "Wow, that light! We should hike up there together this weekend");
        m.reply_to = Some(ReplyTo { id: 3, from: Some("Mira Okafor".into()), text: Some("Photo: Unreal. I took this from the ridge".into()) });
        s.say(60 * 25 - 1, mira(), "Saturday? I'll bring coffee ☕");
        let m = s.say(45, mira(), "Also, the trail guide I mentioned: https://trails.example.org/ridge-loop");
        m.media = Some(Media::WebPage {
            url: "https://trails.example.org/ridge-loop".into(),
            site: Some("Trail Guide".into()),
            title: Some("Ridge Loop — 11 km, 640 m up".into()),
            description: Some("A steady climb through pines to the open ridge, with views over the whole valley. Best at dawn.".into()),
            photo: false,
        });
        let m = s.say(40, you(), "Perfect, saved it. Saturday works for me");
        react(m, &[("👍", 1, false)]);
        // An animated sticker (Olo's own heart, `olo_images::lottie`).
        let m = s.say(39, mira(), "");
        m.media = Some(Media::Sticker { emoji: "❤️".into(), animated: true, w: 512, h: 512 });
        let m = s.say(12, mira(), "");
        m.media = Some(Media::Voice { duration: 14.0, waveform: wave(MIRA, 64), size: 23_000 });
        s.say(11, mira(), "Let's meet at the trailhead at 6. Is that too early?");
        let mut c = Chat { pinned: true, pin_order: 1, unread: 2, ..chat(MIRA, ChatKind::User, "Mira Okafor") };
        c.username = Some("mira_okafor".into());
        c.seen = Some(Seen::Online);
        c.read_outbox = 8;
        d.add(c, s.msgs);

        // Design Crew: a group with mentions, a forward, a poll and a file.
        let mut s = Script::new(CREW, t);
        let m = s.say(60 * 50, None, "");
        m.service = Some("Jun Park created the group “Design Crew”".into());
        s.say(60 * 5, jun(), "Morning all! New mockups for the onboarding flow are up");
        let m = s.say(60 * 5 - 1, jun(), "");
        m.media = Some(Media::File { name: "Onboarding v4.fig".into(), size: 12_400_000, mime: "application/octet-stream".into() });
        let m = s.say(60 * 4, elif(), "These are lovely. The second screen feels crowded though");
        react(m, &[("💯", 2, false)]);
        let m = s.say(60 * 4 - 3, theo(), "Agree with Elif. Could we move the toggle to settings?");
        m.reply_to = Some(ReplyTo { id: 4, from: Some("Elif Demir".into()), text: Some("These are lovely. The second screen feels crowded though".into()) });
        let m = s.say(60 * 3, jun(), "Good call. @you what do you think about the colours?");
        m.mentioned = true;
        if let Some(e) = m.entities.iter_mut().find(|e| e.kind == EntityKind::Mention) {
            e.kind = EntityKind::MentionName(ME);
        }
        let m = s.say(60 * 2, elif(), "Forwarding the brand guidelines for reference");
        m.forwarded = Some("Brand Team".into());
        m.text = "Primary blue is #2B7BD6 — use it for actions only, never for large fills.".into();
        m.entities = Vec::new();
        mark(m, "never", EntityKind::Bold);
        let m = s.say(90, jun(), "");
        m.media = Some(Media::Poll {
            question: "Which onboarding illustration style?".into(),
            answers: vec![("Flat line art".into(), 3), ("Soft 3D".into(), 5), ("Photos".into(), 1)],
            voters: 9,
            closed: false,
            quiz: false,
        });
        let m = s.say(20, theo(), "Soft 3D it is then 🎉");
        react(m, &[("🎉", 3, false), ("❤️", 1, false)]);
        let m = s.say(6, elif(), "I'll update the illustrations page tonight");
        mark(m, "illustrations", EntityKind::Code);
        let mut c = Chat { unread: 4, mentions: 1, members: Some(5), ..chat(CREW, ChatKind::Group, "Design Crew") };
        c.read_inbox = 6;
        d.add(c, s.msgs);

        // A channel.
        let mut s = Script::new(RELEASES, t);
        let m = s.say(60 * 24 * 6, None, "Olo 0.9 is out! Workspaces per display, a new Terminal and faster everything.");
        m.views = Some(18_204);
        mark(m, "Olo 0.9", EntityKind::Bold);
        react(m, &[("🔥", 412, false), ("👍", 120, false)]);
        let m = s.say(60 * 24 * 2, None, "Tip: press ⌘Space and type anything — apps, files, settings, even menu commands.");
        m.views = Some(14_877);
        m.pinned = true;
        let m = s.say(60 * 7, None, "Olo 1.0 beta: StyledEdit gets pictures, page view and Word documents. Read the notes: https://olo.example.org/notes/1.0");
        m.views = Some(9_311);
        m.replies = Some(84);
        m.media = Some(Media::WebPage {
            url: "https://olo.example.org/notes/1.0".into(),
            site: Some("Olo".into()),
            title: Some("What’s new in Olo 1.0".into()),
            description: Some("Pictures in documents, page view, headers and footers, and a whole new Telegram.".into()),
            photo: false,
        });
        react(m, &[("🎉", 266, true), ("❤️", 98, false)]);
        let c = Chat { members: Some(24_310), verified: true, muted: true, unread: 1, ..chat(RELEASES, ChatKind::Channel, "Olo Releases") };
        d.add(c, s.msgs);

        // Theo: a sticker and a document.
        let mut s = Script::new(THEO, t);
        s.say(60 * 30, theo(), "Can you send me the invoice from March?");
        let m = s.say(60 * 29, you(), "");
        m.media = Some(Media::File { name: "Invoice-2026-03.pdf".into(), size: 96_412, mime: "application/pdf".into() });
        let m = s.say(60 * 29 - 1, theo(), "");
        m.media = Some(Media::Sticker { emoji: "🙏".into(), animated: false, w: 512, h: 512 });
        let m = s.say(60 * 8, you(), "Are we still on for lunch tomorrow?");
        m.edited = Some(t - 60 * 8 + 30);
        s.say(60 * 7, theo(), "Yes! The usual place at 12:30");
        let mut c = Chat { read_outbox: 4, ..chat(THEO, ChatKind::User, "Theo Brandt") };
        c.seen = Some(Seen::At(t - 60 * 23));
        d.add(c, s.msgs);

        // A forum with topics.
        let mut s = Script::new(RUST, t);
        let m = s.say(60 * 9, jun(), "Has anyone moved to the 2024 edition yet? Any surprises?");
        m.topic = Some(1);
        let m = s.say(60 * 8, elif(), "Only gen becoming a keyword bit me");
        m.topic = Some(1);
        mark(m, "gen", EntityKind::Code);
        let m = s.say(60 * 2, theo(), "Why does this borrow fail?\n\nlet first = &v[0];\nv.push(4);\nprintln!(\"{first}\");");
        m.topic = Some(2);
        if let Some(p) = m.text.find("let first") {
            let end = m.text.len();
            m.entities.push(Entity { start: p, end, kind: EntityKind::Pre("rust".into()) });
        }
        let m = s.say(55, mira(), "Pushing may reallocate, so first could dangle. Clone the value or push first.");
        m.topic = Some(2);
        mark(m, "first", EntityKind::Code);
        m.reply_to = Some(ReplyTo { id: 3, from: Some("Theo Brandt".into()), text: Some("Why does this borrow fail?".into()) });
        let m = s.say(15, jun(), "Made a tiny terminal ray tracer this weekend ✨");
        m.topic = Some(3);
        m.media = Some(Media::Photo { w: 1024, h: 640, thumb: None });
        let c = Chat { members: Some(1_284), unread: 3, ..chat(RUST, ChatKind::Forum, "Rust Lovers") };
        d.add(c, s.msgs);

        // A bot.
        let mut s = Script::new(BOT, t);
        s.say(60 * 3, you(), "/forecast");
        let m = s.say(60 * 3, Some(person(BOT, "Weather Bot")), "Today: ☀️ 21° / 12°, light wind from the west.\nTomorrow: 🌦 17° / 10°, showers after noon.");
        mark(m, "Today:", EntityKind::Bold);
        mark(m, "Tomorrow:", EntityKind::Bold);
        d.add(chat(BOT, ChatKind::Bot, "Weather Bot"), s.msgs);

        // Family: muted group.
        let mut s = Script::new(FAMILY, t);
        s.say(60 * 4, Some(person(1010, "Mum")), "Who's bringing dessert on Sunday?");
        s.say(60 * 3, Some(person(1011, "Sam")), "Me! Apple crumble 🍏");
        let m = s.say(60 * 2, Some(person(1010, "Mum")), "");
        m.media = Some(Media::Photo { w: 900, h: 1200, thumb: None });
        let c = Chat { muted: true, unread: 12, members: Some(6), ..chat(FAMILY, ChatKind::Group, "Family") };
        d.add(c, s.msgs);

        // More people.
        let mut s = Script::new(JUN, t);
        s.say(60 * 24 * 2, jun(), "Thanks for the review yesterday!");
        s.say(60 * 24 * 2 - 5, you(), "Any time 🙂");
        let mut c = chat(JUN, ChatKind::User, "Jun Park");
        c.read_outbox = 2;
        c.seen = Some(Seen::Recently);
        d.add(c, s.msgs);
        let mut s = Script::new(ELIF, t);
        let m = s.say(60 * 24 * 3, elif(), "");
        m.media = Some(Media::Audio { title: Some("Morning Light".into()), performer: Some("Field Notes".into()), duration: 214.0, name: None, size: 5_100_000 });
        s.say(60 * 24 * 3 - 1, elif(), "This one's been on repeat all week");
        let mut c = Chat { draft: Some("I love it! Is the whole album".into()), ..chat(ELIF, ChatKind::User, "Elif Demir") };
        c.seen = Some(Seen::At(t - 3600 * 5));
        d.add(c, s.msgs);

        // Archived.
        let mut s = Script::new(OLDPROJ, t);
        s.say(60 * 24 * 40, Some(person(1020, "Ravi")), "Closing this group, thanks everyone!");
        d.add(Chat { archived: true, members: Some(8), ..chat(OLDPROJ, ChatKind::Group, "Old Project") }, s.msgs);
        let mut s = Script::new(NEWS, t);
        let m = s.say(60 * 24, None, "This week: five quiet places to read in the city.");
        m.views = Some(3_210);
        d.add(Chat { archived: true, unread: 3, muted: true, ..chat(NEWS, ChatKind::Channel, "City Newsletter") }, s.msgs);

        // Unknown senders: not contacts, never written to.
        let mut s = Script::new(STRANGER, t);
        s.say(30, Some(person(STRANGER, "Crypto Deals Daily")), "🚀 Exclusive offer! Double your coins in 24 hours, DM now");
        d.add(Chat { contact: false, unknown: true, unread: 1, ..chat(STRANGER, ChatKind::User, "Crypto Deals Daily") }, s.msgs);
        let mut s = Script::new(STRANGER2, t);
        s.say(60 * 6, Some(person(STRANGER2, "Alex Recruiter")), "Hi! Saw your profile, are you open to new roles?");
        d.add(Chat { contact: false, unknown: true, unread: 1, ..chat(STRANGER2, ChatKind::User, "Alex Recruiter") }, s.msgs);

        // Many more, for scrolling.
        let extra: usize = std::env::var("OLO_TELEGRAM_DEMO").ok().and_then(|v| v.parse().ok()).unwrap_or(24);
        const FIRST: [&str; 12] = ["Ada", "Ben", "Cleo", "Dev", "Emma", "Finn", "Gia", "Hugo", "Iris", "Jonas", "Kai", "Lena"];
        const LAST: [&str; 9] = ["Ito", "Novak", "Silva", "Moreau", "Haddad", "Kowalski", "Nakamura", "Olsen", "Rossi"];
        const LINES: [&str; 8] =
            ["See you then!", "Sounds good 👍", "Can you call me when you're free?", "Haha, exactly", "Sent you the photos", "On my way", "Happy birthday!! 🎂", "Let me check and get back to you"];
        for i in 0..extra {
            let id = MANY + i as i64;
            let name = format!("{} {}", FIRST[i % FIRST.len()], LAST[(i / FIRST.len()) % LAST.len()]);
            let mut s = Script::new(id, t);
            // Each has a long history for scrolling back, only made when opened.
            let ago = 60 * 24 * (4 + i as i64 / 3);
            s.say(ago, Some(person(id, &name)), LINES[i % LINES.len()]);
            let c = Chat { unread: if i % 7 == 0 { 1 } else { 0 }, ..chat(id, ChatKind::User, &name) };
            d.add(c, s.msgs);
        }
        d
    }

    fn add(&mut self, mut chat: Chat, msgs: Vec<Message>) {
        if let Some(last) = msgs.last() {
            chat.last = Some(Preview::of(last));
            chat.order = last.date;
        }
        if chat.read_inbox == 0 {
            chat.read_inbox = msgs.len().saturating_sub(chat.unread as usize) as MsgId;
        }
        self.msgs.insert(chat.id, msgs);
        self.chats.insert(chat.id, chat);
    }

    fn emit(&self, e: Event) {
        (self.sink)(e);
    }

    /// The long history a generated chat has when scrolled back.
    fn history(&mut self, chat: ChatId) -> &mut Vec<Message> {
        let t = self.now;
        let msgs = self.msgs.entry(chat).or_default();
        if chat >= MANY && msgs.len() < 3 {
            let name = msgs.first().and_then(|m| m.from.clone());
            let mut older = Vec::new();
            for k in 0..300 {
                let out = k % 3 == 1;
                let from = if out { Some(me()) } else { name.clone() };
                let text = format!("Message {} of our long chat — {}", k + 1, ["about the plans", "a quick question", "the photos from Sunday", "the meeting notes"][k % 4]);
                older.push(Message { id: k as MsgId + 1, chat, from, date: t - 60 * 24 * 60 * 30 + k as i64 * 3600, out, text, ..Message::default() });
            }
            let base = older.len() as MsgId;
            for (i, m) in msgs.iter_mut().enumerate() {
                m.id = base + i as MsgId + 1;
            }
            older.append(msgs);
            *msgs = older;
        }
        self.msgs.get_mut(&chat).unwrap()
    }

    fn page(&mut self, chat: ChatId, topic: Option<MsgId>, before: Option<MsgId>) -> (Vec<Message>, bool) {
        let all: Vec<Message> = self.history(chat).iter().filter(|m| topic.is_none() || m.topic == topic).filter(|m| before.is_none_or(|b| m.id < b)).cloned().collect();
        let n = all.len();
        let from = n.saturating_sub(40);
        (all[from..].to_vec(), from > 0)
    }

    fn chat_changed(&self, id: ChatId) {
        if let Some(c) = self.chats.get(&id) {
            self.emit(Event::Chat(c.clone()));
        }
    }

    fn refresh_last(&mut self, chat: ChatId) {
        let last = self.msgs.get(&chat).and_then(|m| m.last()).map(|m| (Preview::of(m), m.date));
        if let Some(c) = self.chats.get_mut(&chat) {
            c.last = last.as_ref().map(|l| l.0.clone());
            c.order = last.map_or(c.order, |l| l.1);
        }
    }

    fn find(&mut self, chat: ChatId, id: MsgId) -> Option<&mut Message> {
        self.msgs.get_mut(&chat)?.iter_mut().find(|m| m.id == id)
    }

    /// A message from someone in `chat`, now.
    fn arrive(&mut self, chat: ChatId, from: Sender, text: &str) {
        self.next_id += 1;
        let id = self.msgs.get(&chat).and_then(|m| m.last()).map_or(1, |m| m.id + 1).max(self.next_id);
        let m = Message { id, chat, from: Some(from), date: now(), text: text.into(), entities: auto_entities(text), ..Message::default() };
        self.msgs.entry(chat).or_default().push(m.clone());
        self.refresh_last(chat);
        if let Some(c) = self.chats.get_mut(&chat) {
            c.unread += 1;
        }
        self.emit(Event::New(m));
        self.chat_changed(chat);
    }

    /// A picture for a message: drawn once into the cache.
    fn animated_sticker(&self, chat: ChatId, id: MsgId) -> bool {
        self.msgs.get(&chat).and_then(|v| v.iter().find(|m| m.id == id)).is_some_and(|m| matches!(m.media, Some(Media::Sticker { animated: true, .. })))
    }

    /// The animated sticker: its file (a `.tgs`), or its picture (its
    /// first frame, as Telegram's thumbnails are).
    fn heart(&self, file: bool) -> Option<PathBuf> {
        std::fs::create_dir_all(&self.pictures).ok()?;
        let tgs = self.pictures.join("heart.tgs");
        if !tgs.exists() {
            crate::account::private_write(&tgs, &olo_images::lottie::heart_tgs()).ok()?;
        }
        if file {
            return Some(tgs);
        }
        let path = self.pictures.join("heart.png");
        if !path.exists() {
            let l = olo_images::lottie::Lottie::open(&tgs).ok()?;
            let mut p = olo_images::lottie::Painter::new(256, 256);
            let mut px = p.paint(&l, 0.0).ok()?.to_vec();
            olo_images::lottie::unpremultiply(&mut px);
            let mut out = Vec::new();
            {
                let mut enc = png::Encoder::new(&mut out, 256, 256);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                enc.write_header().ok()?.write_image_data(&px).ok()?;
            }
            crate::account::private_write(&path, &out).ok()?;
        }
        Some(path)
    }

    fn picture(&self, chat: ChatId, id: MsgId, w: u32, h: u32) -> Option<PathBuf> {
        let path = self.pictures.join(format!("{chat}-{id}.png"));
        if !path.exists() {
            std::fs::create_dir_all(&self.pictures).ok()?;
            let png = landscape(w.min(640), h.min(640), (chat as u64).wrapping_mul(31).wrapping_add(id as u64));
            crate::account::private_write(&path, &png).ok()?;
        }
        Some(path)
    }
}

/// A voice message's bars (0–31), made up but steady.
fn wave(seed: i64, n: usize) -> Vec<u8> {
    let mut x = seed as u64 | 1;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let envelope = ((i as f32 / n as f32) * std::f32::consts::PI).sin();
            ((x % 20) as f32 * envelope + 4.0).min(31.0) as u8
        })
        .collect()
}

/// A picture of hills under a sky, `w`×`h`, as a PNG (the demo's photos).
fn landscape(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let hues: [([f32; 3], [f32; 3]); 4] =
        [([0.98, 0.62, 0.38], [0.36, 0.30, 0.55]), ([0.55, 0.78, 0.96], [0.20, 0.42, 0.30]), ([0.99, 0.80, 0.55], [0.45, 0.25, 0.30]), ([0.70, 0.84, 0.95], [0.25, 0.35, 0.50])];
    let (sky, land) = hues[(seed % 4) as usize];
    let phase = (seed % 97) as f32 / 15.0;
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let ridge = 0.55 + 0.08 * (fx * 6.0 + phase).sin() + 0.04 * (fx * 15.0 + phase * 2.0).sin();
            let near = 0.72 + 0.06 * (fx * 4.0 - phase).cos();
            let sun = ((fx - 0.7).powi(2) + (fy - 0.3).powi(2)).sqrt();
            let c: [f32; 3] = if fy > near {
                [land[0] * 0.6, land[1] * 0.6, land[2] * 0.6]
            } else if fy > ridge {
                land
            } else {
                let glow = (1.0 - sun * 3.0).max(0.0);
                let k = 1.0 - fy * 0.5;
                [(sky[0] * k + glow).min(1.0), (sky[1] * k + glow * 0.8).min(1.0), (sky[2] * k + glow * 0.5).min(1.0)]
            };
            px.extend(c.iter().map(|v| (v * 255.0) as u8));
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut wr) = enc.write_header() {
            let _ = wr.write_image_data(&px);
        }
    }
    out
}

pub fn spawn(mut from: UnboundedReceiver<Cmd>, sink: Sink) {
    let _ = std::thread::Builder::new().name("telegram-demo".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("a runtime");
        rt.block_on(async move {
            let demo = Arc::new(Mutex::new(Demo::new(sink.clone())));
            sink(Event::Status(Status::Online));
            sink(Event::Me(Me { id: ME, name: "Alex Rivera".into(), phone: Some("+1 555 010 0199".into()), username: Some("alexr".into()), photo: None }));
            if let Some(every) = std::env::var("OLO_TELEGRAM_DEMO_EVERY").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&s| s > 0) {
                let d = demo.clone();
                tokio::spawn(async move {
                    const SAY: [(ChatId, &str, &str); 4] = [
                        (MIRA, "Mira Okafor", "Just checked the forecast, Saturday looks clear ☀️"),
                        (THEO, "Theo Brandt", "Running 5 minutes late!"),
                        (CREW, "Jun Park", "Pushed the new illustrations, have a look"),
                        (JUN, "Jun Park", "Are you around for a quick call?"),
                    ];
                    let mut k = 0;
                    loop {
                        tokio::time::sleep(Duration::from_secs(every)).await;
                        let (chat, name, text) = SAY[k % SAY.len()];
                        let from = if chat == CREW { person(JUN, name) } else { person(chat, name) };
                        d.lock().unwrap().arrive(chat, from, text);
                        k += 1;
                    }
                });
            }
            while let Some(cmd) = from.recv().await {
                handle(&demo, cmd);
            }
        });
    });
}

fn handle(demo: &Arc<Mutex<Demo>>, cmd: Cmd) {
    let mut d = demo.lock().unwrap();
    // The person's changes take a while to "reach Telegram" when asked
    // to, as the real account's outbox shows them (shown at once all the
    // same).
    let ms = std::env::var("OLO_TELEGRAM_DEMO_SEND_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0);
    if let Some(ms) = ms
        && matches!(cmd, Cmd::Archive { .. } | Cmd::Mute { .. } | Cmd::PinChat { .. } | Cmd::MarkUnread { .. })
    {
        d.sending.0 += 1;
        d.sending.1 += 1;
        let (pending, total) = d.sending;
        d.emit(Event::Sync { pending, total, waiting: None });
        let demo = demo.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms * pending as u64));
            let mut d = demo.lock().unwrap();
            d.sending.0 -= 1;
            if d.sending.0 == 0 {
                d.sending.1 = 0;
            }
            let (pending, total) = d.sending;
            d.emit(Event::Sync { pending, total, waiting: None });
        });
    }
    match cmd {
        Cmd::Chats => {
            d.emit(Event::Folders(vec![
                Folder { id: 2, title: "Personal".into(), emoticon: Some("👤".into()), contacts: true, non_contacts: true, exclude_archived: true, ..Folder::default() },
                Folder { id: 3, title: "Groups".into(), emoticon: Some("👥".into()), groups: true, exclude_archived: true, ..Folder::default() },
                Folder { id: 4, title: "Channels".into(), emoticon: Some("📢".into()), channels: true, bots: true, exclude_archived: true, ..Folder::default() },
                Folder {
                    id: 5,
                    title: "Unread".into(),
                    emoticon: Some("✅".into()),
                    contacts: true,
                    non_contacts: true,
                    groups: true,
                    channels: true,
                    bots: true,
                    exclude_read: true,
                    ..Folder::default()
                },
            ]));
            let mut all: Vec<Chat> = d.chats.values().cloned().collect();
            all.sort_by_key(|c| std::cmp::Reverse(c.order));
            let n = all.len();
            for (i, page) in all.chunks(100).enumerate() {
                d.emit(Event::Chats { chats: page.to_vec(), done: (i + 1) * 100 >= n, cached: false });
            }
        }
        Cmd::Open { chat, topic } => {
            let (msgs, more) = d.page(chat, topic, None);
            d.emit(Event::Messages { chat, topic, msgs, more, before: None });
            if d.chats.get(&chat).is_some_and(|c| c.kind == ChatKind::Forum) {
                let t = d.now;
                let topics = vec![
                    Topic { id: 1, title: "General".into(), emoji: Some("💬".into()), color: 0x6FB9F0, date: t - 60 * 8, ..Topic::default() },
                    Topic { id: 2, title: "Help".into(), emoji: Some("🆘".into()), color: 0xFB6F5F, unread: 2, date: t - 55 * 60, ..Topic::default() },
                    Topic { id: 3, title: "Showcase".into(), emoji: Some("✨".into()), color: 0xFFD67E, unread: 1, date: t - 15 * 60, ..Topic::default() },
                ];
                d.emit(Event::Topics { chat, topics });
            }
        }
        Cmd::Older { chat, topic, before } => {
            let (msgs, more) = d.page(chat, topic, Some(before));
            d.emit(Event::Messages { chat, topic, msgs, more, before: Some(before) });
        }
        Cmd::Around { chat, id } => {
            let all = d.history(chat).clone();
            let at = all.iter().position(|m| m.id >= id).unwrap_or(0);
            let from = at.saturating_sub(20);
            let to = (at + 20).min(all.len());
            d.emit(Event::Around { chat, id, msgs: all[from..to].to_vec(), more: from > 0 });
        }
        Cmd::Send(out) => {
            d.next_id += 1;
            let id = d.msgs.get(&out.chat).and_then(|m| m.last()).map_or(1, |m| m.id + 1).max(d.next_id);
            let media = out.files.first().map(|f| {
                let name = f.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
                let size = std::fs::metadata(f).map_or(0, |m| m.len());
                let picture = !out.as_files && ["png", "jpg", "jpeg", "webp", "gif"].iter().any(|e| name.to_lowercase().ends_with(e));
                if picture { Media::Photo { w: 800, h: 600, thumb: None } } else { Media::File { name, size, mime: "application/octet-stream".into() } }
            });
            let media = media.or_else(|| out.voice.as_ref().map(|(_, dur, w)| Media::Voice { duration: *dur, waveform: w.clone(), size: 0 }));
            let reply_to = out.reply_to.and_then(|r| {
                let m = d.msgs.get(&out.chat)?.iter().find(|m| m.id == r)?;
                Some(ReplyTo { id: r, from: m.from.as_ref().map(|f| f.name.clone()), text: Some(m.summary()) })
            });
            let msg = Message {
                id,
                chat: out.chat,
                from: Some(me()),
                date: now(),
                out: true,
                text: out.text.clone(),
                entities: out.entities.clone(),
                reply_to,
                media,
                topic: out.topic,
                ..Message::default()
            };
            d.msgs.entry(out.chat).or_default().push(msg.clone());
            d.refresh_last(out.chat);
            if let Some(c) = d.chats.get_mut(&out.chat) {
                c.draft = None;
                // Written back: known now.
                c.unknown = false;
            }
            let (local, chat) = (out.local, out.chat);
            let dm = demo.clone();
            drop(d);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                {
                    let d = dm.lock().unwrap();
                    d.emit(Event::Sent { local, msg: msg.clone() });
                    d.chat_changed(chat);
                }
                if chat == SAVED || chat == BOT || chat < 0 {
                    if chat == BOT {
                        tokio::time::sleep(Duration::from_millis(600)).await;
                        dm.lock().unwrap().arrive(BOT, person(BOT, "Weather Bot"), "Today: ☀️ 21° / 12°, light wind from the west.");
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1200)).await;
                {
                    let mut d = dm.lock().unwrap();
                    if let Some(c) = d.chats.get_mut(&chat) {
                        c.read_outbox = msg.id;
                    }
                    d.emit(Event::Read { chat, outbox: true, up_to: msg.id });
                    d.chat_changed(chat);
                }
                if chat == MIRA {
                    let name = "Mira Okafor";
                    dm.lock().unwrap().emit(Event::Doing { chat, topic: None, who: name.into(), doing: Doing::Typing });
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                    let mut d = dm.lock().unwrap();
                    d.emit(Event::Doing { chat, topic: None, who: name.into(), doing: Doing::Stopped });
                    d.arrive(chat, person(MIRA, name), "Sounds great, see you there! 🥾");
                }
            });
        }
        Cmd::Edit { chat, id, text, entities } => {
            if let Some(m) = d.find(chat, id) {
                m.text = text;
                m.entities = entities;
                m.edited = Some(now());
                let m = m.clone();
                d.emit(Event::Edited(m));
                d.refresh_last(chat);
                d.chat_changed(chat);
            }
        }
        Cmd::Delete { chat, ids, .. } => {
            if let Some(v) = d.msgs.get_mut(&chat) {
                v.retain(|m| !ids.contains(&m.id));
            }
            d.emit(Event::Deleted { chat: Some(chat), ids });
            d.refresh_last(chat);
            d.chat_changed(chat);
        }
        Cmd::Forward { from, ids, to } => {
            let name = d.chats.get(&from).map(|c| c.title.clone());
            let picked: Vec<Message> = d.msgs.get(&from).map(|v| v.iter().filter(|m| ids.contains(&m.id)).cloned().collect()).unwrap_or_default();
            for mut m in picked {
                d.next_id += 1;
                m.id = d.msgs.get(&to).and_then(|v| v.last()).map_or(1, |l| l.id + 1).max(d.next_id);
                m.forwarded = m.forwarded.clone().or_else(|| m.from.as_ref().map(|f| f.name.clone())).or(name.clone());
                m.chat = to;
                m.from = Some(me());
                m.out = true;
                m.date = now();
                m.reactions.clear();
                m.reply_to = None;
                d.msgs.entry(to).or_default().push(m.clone());
                d.emit(Event::New(m));
            }
            d.refresh_last(to);
            d.chat_changed(to);
        }
        Cmd::React { chat, id, emoji } => {
            if let Some(m) = d.find(chat, id) {
                for r in &mut m.reactions {
                    if r.mine {
                        r.mine = false;
                        r.count -= 1;
                    }
                }
                m.reactions.retain(|r| r.count > 0);
                if let Some(e) = emoji {
                    match m.reactions.iter_mut().find(|r| r.emoji == e) {
                        Some(r) => {
                            r.count += 1;
                            r.mine = true;
                        }
                        None => m.reactions.push(Reaction { emoji: e, count: 1, mine: true }),
                    }
                }
                let m = m.clone();
                d.emit(Event::Edited(m));
            }
        }
        Cmd::Read { chat, up_to } => {
            if let Some(c) = d.chats.get_mut(&chat) {
                c.read_inbox = c.read_inbox.max(up_to);
                c.unread = 0;
                c.mentions = 0;
                c.marked = false;
            }
            d.chat_changed(chat);
        }
        Cmd::MarkUnread { chat, on } => {
            if let Some(c) = d.chats.get_mut(&chat) {
                c.marked = on;
            }
            d.chat_changed(chat);
        }
        Cmd::Typing { .. } => {}
        Cmd::Draft { chat, text } => {
            if let Some(c) = d.chats.get_mut(&chat) {
                c.draft = Some(text).filter(|t| !t.trim().is_empty());
            }
            d.chat_changed(chat);
        }
        Cmd::PinChat { chat, on } => {
            let top = d.chats.values().filter(|c| c.pinned).map(|c| c.pin_order).max().unwrap_or(0);
            if let Some(c) = d.chats.get_mut(&chat) {
                c.pinned = on;
                c.pin_order = top + 1;
            }
            d.chat_changed(chat);
        }
        Cmd::Archive { chat, on } => {
            if let Some(c) = d.chats.get_mut(&chat) {
                c.archived = on;
                c.pinned = false;
            }
            d.chat_changed(chat);
        }
        Cmd::Mute { chat, on } => {
            if let Some(c) = d.chats.get_mut(&chat) {
                c.muted = on;
            }
            d.chat_changed(chat);
        }
        Cmd::DeleteChat { chat } => {
            d.chats.remove(&chat);
            d.msgs.remove(&chat);
            d.emit(Event::ChatGone(chat));
        }
        Cmd::PinMessage { chat, id, on } => {
            if let Some(m) = d.find(chat, id) {
                m.pinned = on;
                let m = m.clone();
                d.emit(Event::Edited(m));
            }
        }
        Cmd::Fetch(pic) => match pic {
            Pic::Avatar(_) => {}
            Pic::Thumb(chat, id) | Pic::File(chat, id) if d.animated_sticker(chat, id) => match d.heart(matches!(pic, Pic::File(..))) {
                Some(path) => d.emit(Event::Fetched { pic, path }),
                None => d.emit(Event::FetchFailed { pic, why: "The demo couldn’t make its sticker.".into() }),
            },
            Pic::Thumb(chat, id) | Pic::File(chat, id) => {
                let px = d.msgs.get(&chat).and_then(|v| v.iter().find(|m| m.id == id)).and_then(|m| m.media.as_ref()).and_then(|m| m.pixels());
                match px.and_then(|(w, h)| d.picture(chat, id, w, h)) {
                    Some(path) => d.emit(Event::Fetched { pic, path }),
                    None => d.emit(Event::FetchFailed { pic, why: "The demo has no file for it.".into() }),
                }
            }
        },
        Cmd::SaveFile { chat, id, to } => {
            let px = d.msgs.get(&chat).and_then(|v| v.iter().find(|m| m.id == id)).and_then(|m| m.media.as_ref()).and_then(|m| m.pixels());
            let ok = match px.and_then(|(w, h)| d.picture(chat, id, w, h)) {
                Some(p) => std::fs::copy(p, &to).is_ok(),
                None => std::fs::write(&to, b"Olo Telegram demo file\n").is_ok(),
            };
            if ok {
                d.emit(Event::Saved { to });
            } else {
                d.emit(Event::Error(format!("Couldn’t save {}.", to.display())));
            }
        }
        Cmd::Search { query } => {
            let q = query.to_lowercase();
            let mut chats: Vec<Chat> = d.chats.values().filter(|c| c.title.to_lowercase().contains(&q) || c.username.as_ref().is_some_and(|u| u.contains(&q))).cloned().collect();
            chats.sort_by_key(|c| std::cmp::Reverse(c.order));
            let mut msgs: Vec<Message> = d.msgs.values().flatten().filter(|m| !q.is_empty() && m.text.to_lowercase().contains(&q)).cloned().collect();
            msgs.sort_by_key(|m| std::cmp::Reverse(m.date));
            msgs.truncate(50);
            d.emit(Event::Results { query, chats, msgs });
        }
        Cmd::Contacts => {
            let mut people: Vec<Contact> = d
                .chats
                .values()
                .filter(|c| c.kind == ChatKind::User)
                .map(|c| Contact { id: c.id, name: c.title.clone(), phone: None, username: c.username.clone(), photo: None, seen: c.seen.clone() })
                .collect();
            people.sort_by(|a, b| a.name.cmp(&b.name));
            d.emit(Event::Contacts(people));
        }
        Cmd::Resolve { username } => {
            let chat = d.chats.values().find(|c| c.username.as_deref() == Some(username.trim_start_matches('@'))).cloned();
            d.emit(Event::Resolved { username, chat });
        }
        Cmd::SignOut => d.emit(Event::Status(Status::SignedOut)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_found() {
        let e = auto_entities("see https://a.example/x, ask @mira about #trip");
        let kinds: Vec<_> = e.iter().map(|e| e.kind.clone()).collect();
        assert_eq!(kinds, [EntityKind::Url, EntityKind::Mention, EntityKind::Hashtag]);
        let t = "see https://a.example/x, ask @mira about #trip";
        assert_eq!(&t[e[0].start..e[0].end], "https://a.example/x");
        assert_eq!(&t[e[1].start..e[1].end], "@mira");
    }

    #[test]
    fn demo_answers() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let sink: Sink = Arc::new(move |e| g.lock().unwrap().push(e));
        let demo = Arc::new(Mutex::new(Demo::new(sink)));
        handle(&demo, Cmd::Chats);
        handle(&demo, Cmd::Open { chat: MANY, topic: None });
        handle(&demo, Cmd::Older { chat: MANY, topic: None, before: 262 });
        let got = got.lock().unwrap();
        let chats: usize = got.iter().map(|e| if let Event::Chats { chats, .. } = e { chats.len() } else { 0 }).sum();
        assert!(chats > 30);
        let pages: Vec<(usize, bool, MsgId)> = got.iter().filter_map(|e| if let Event::Messages { msgs, more, .. } = e { Some((msgs.len(), *more, msgs[0].id)) } else { None }).collect();
        assert_eq!(pages, [(40, true, 262), (40, true, 222)]);
        let png = landscape(32, 24, 3);
        assert_eq!(&png[1..4], b"PNG");
    }
}
