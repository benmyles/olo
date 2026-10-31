//! Telegram's chats and messages as plain data: what the app draws, the
//! demo account makes and the disk cache keeps. Nothing here talks to
//! Telegram; `net` makes these from grammers' types.

use serde::{Deserialize, Serialize};

/// A chat, by the Bot API's dialog id: users positive, basic groups
/// negative, channels and supergroups -100… (unique across kinds).
pub type ChatId = i64;
/// A message, within its chat.
pub type MsgId = i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChatKind {
    #[default]
    User,
    Bot,
    /// Saved Messages: the account's own chat.
    Saved,
    Group,
    /// A broadcast channel.
    Channel,
    /// A supergroup whose messages are in topics.
    Forum,
}

impl ChatKind {
    /// Messages in it show who sent them.
    pub fn many(self) -> bool {
        matches!(self, ChatKind::Group | ChatKind::Forum)
    }
}

/// One row of the chat list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: String,
    pub username: Option<String>,
    /// The picture's id (a key for its file) when it has one.
    pub photo: Option<i64>,
    pub unread: u32,
    /// Unread messages that mention us.
    pub mentions: u32,
    /// Marked unread by hand.
    pub marked: bool,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
    pub verified: bool,
    pub contact: bool,
    pub last: Option<Preview>,
    pub draft: Option<String>,
    /// Where it sorts (the latest message's or draft's time).
    pub order: i64,
    /// Our messages up to this one have been read.
    pub read_outbox: MsgId,
    /// Theirs up to this one we have read.
    pub read_inbox: MsgId,
    pub members: Option<u32>,
    /// A person's last seen ("online", "last seen 5 minutes ago"), as
    /// Unix seconds or a word (`Seen`).
    pub seen: Option<Seen>,
    /// Pinned in the list's order (a lower one first).
    pub pin_order: u32,
}

impl Chat {
    /// The latest of its messages we've seen is ours and they've read it.
    pub fn last_read(&self) -> bool {
        self.last.as_ref().is_some_and(|l| l.out && l.id <= self.read_outbox)
    }
}

/// When a person was last online.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Seen {
    Online,
    At(i64),
    Recently,
    LastWeek,
    LastMonth,
    LongAgo,
}

impl Seen {
    /// "online", "last seen 5 minutes ago", for a chat's header.
    pub fn describe(&self, now: i64) -> String {
        match self {
            Seen::Online => "online".into(),
            Seen::At(t) => {
                let ago = (now - t).max(0);
                match ago {
                    0..60 => "last seen just now".into(),
                    60..3600 => plural(ago / 60, "minute", "last seen", "ago"),
                    3600..86400 => plural(ago / 3600, "hour", "last seen", "ago"),
                    _ => plural(ago / 86400, "day", "last seen", "ago"),
                }
            }
            Seen::Recently => "last seen recently".into(),
            Seen::LastWeek => "last seen within a week".into(),
            Seen::LastMonth => "last seen within a month".into(),
            Seen::LongAgo => "last seen a long time ago".into(),
        }
    }
}

fn plural(n: i64, unit: &str, before: &str, after: &str) -> String {
    format!("{before} {n} {unit}{} {after}", if n == 1 { "" } else { "s" })
}

/// A chat's latest message, as its row shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Preview {
    pub id: MsgId,
    /// Who sent it, in a group ("You" for us).
    pub from: Option<String>,
    pub text: String,
    pub date: i64,
    pub out: bool,
}

impl Preview {
    pub fn of(m: &Message) -> Preview {
        Preview { id: m.id, from: m.from.as_ref().map(|f| f.name.clone()), text: m.summary(), date: m.date, out: m.out }
    }
}

/// Who sent a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sender {
    pub id: i64,
    pub name: String,
    pub photo: Option<i64>,
}

/// A message's link to the one it answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplyTo {
    pub id: MsgId,
    pub from: Option<String>,
    /// What it says (a summary), once known.
    pub text: Option<String>,
}

/// Formatting and links within a message's text, in bytes of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub start: usize,
    pub end: usize,
    pub kind: EntityKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    Pre(String),
    /// A link written out in the text.
    Url,
    /// Text that links somewhere else.
    TextUrl(String),
    /// @username.
    Mention,
    /// A person named without a username.
    MentionName(i64),
    Hashtag,
    Cashtag,
    BotCommand,
    Email,
    Phone,
    Quote,
    CustomEmoji(i64),
}

impl EntityKind {
    /// Clicking it goes somewhere.
    pub fn is_link(&self) -> bool {
        matches!(self, EntityKind::Url | EntityKind::TextUrl(_) | EntityKind::Mention | EntityKind::MentionName(_) | EntityKind::Email | EntityKind::Phone | EntityKind::Hashtag | EntityKind::BotCommand)
    }
}

/// What a message carries besides its text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Media {
    Photo { w: u32, h: u32, thumb: Option<Vec<u8>> },
    Video { w: u32, h: u32, duration: f32, round: bool, gif: bool, name: Option<String>, size: u64 },
    Voice { duration: f32, waveform: Vec<u8>, size: u64 },
    Audio { title: Option<String>, performer: Option<String>, duration: f32, name: Option<String>, size: u64 },
    File { name: String, size: u64, mime: String },
    Sticker { emoji: String, animated: bool, w: u32, h: u32 },
    Contact { name: String, phone: String },
    Location { lat: f64, lon: f64, title: Option<String>, address: Option<String> },
    Poll { question: String, answers: Vec<(String, u32)>, voters: u32, closed: bool, quiz: bool },
    WebPage { url: String, site: Option<String>, title: Option<String>, description: Option<String>, photo: bool },
    Dice { emoji: String, value: i32 },
    Other(String),
}

impl Media {
    /// How it's named in a chat's row or a reply's quote.
    pub fn label(&self) -> String {
        match self {
            Media::Photo { .. } => "Photo".into(),
            Media::Video { round: true, .. } => "Video message".into(),
            Media::Video { gif: true, .. } => "GIF".into(),
            Media::Video { .. } => "Video".into(),
            Media::Voice { duration, .. } => format!("Voice message ({})", clock(*duration)),
            Media::Audio { title, performer, .. } => match (performer, title) {
                (Some(p), Some(t)) => format!("{p} – {t}"),
                (_, Some(t)) => t.clone(),
                _ => "Audio".into(),
            },
            Media::File { name, .. } => name.clone(),
            Media::Sticker { emoji, .. } => format!("{emoji} Sticker"),
            Media::Contact { name, .. } => format!("Contact: {name}"),
            Media::Location { title: Some(t), .. } => format!("Location: {t}"),
            Media::Location { .. } => "Location".into(),
            Media::Poll { question, .. } => format!("Poll: {question}"),
            Media::WebPage { title: Some(t), .. } => t.clone(),
            Media::WebPage { url, .. } => url.clone(),
            Media::Dice { emoji, .. } => emoji.clone(),
            Media::Other(what) => what.clone(),
        }
    }

    /// Shown as a picture (with its own downloaded thumbnail).
    pub fn visual(&self) -> bool {
        matches!(self, Media::Photo { .. } | Media::Video { .. } | Media::Sticker { .. })
    }

    /// Its size in pixels, for pictures.
    pub fn pixels(&self) -> Option<(u32, u32)> {
        match *self {
            Media::Photo { w, h, .. } | Media::Video { w, h, .. } | Media::Sticker { w, h, .. } if w > 0 && h > 0 => Some((w, h)),
            _ => None,
        }
    }

    /// The bytes to download for all of it.
    pub fn size(&self) -> Option<u64> {
        match *self {
            Media::Video { size, .. } | Media::Voice { size, .. } | Media::Audio { size, .. } | Media::File { size, .. } => Some(size),
            _ => None,
        }
    }
}

/// "1:05" for 65 seconds (hours when there are some).
pub fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// "2.4 MB".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} bytes");
    }
    let mut v = n as f64 / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", UNITS[u]) } else { format!("{v:.0} {}", UNITS[u]) }
}

/// One kind of reaction on a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    /// One of them is ours.
    pub mine: bool,
}

/// A message being sent, until Telegram has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pending {
    Sending,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MsgId,
    pub chat: ChatId,
    pub from: Option<Sender>,
    pub date: i64,
    pub edited: Option<i64>,
    pub out: bool,
    pub text: String,
    pub entities: Vec<Entity>,
    pub reply_to: Option<ReplyTo>,
    /// Forwarded from (a name).
    pub forwarded: Option<String>,
    pub media: Option<Media>,
    pub reactions: Vec<Reaction>,
    pub views: Option<u32>,
    /// Comments or replies in its thread.
    pub replies: Option<u32>,
    /// The forum topic it's in.
    pub topic: Option<MsgId>,
    /// What a service message says ("Ann joined the group").
    pub service: Option<String>,
    pub pinned: bool,
    pub mentioned: bool,
    /// Messages sent together as an album share this.
    pub album: Option<i64>,
    pub via_bot: Option<String>,
    /// Ours, not yet in Telegram: its local id and state.
    pub pending: Option<(u64, Pending)>,
}

impl Message {
    /// One line for a chat's row, a quote or a notification.
    pub fn summary(&self) -> String {
        if let Some(s) = &self.service {
            return s.clone();
        }
        let text = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        match (&self.media, text.is_empty()) {
            (Some(m), true) => m.label(),
            (Some(Media::WebPage { .. }) | None, false) => text,
            (Some(m), false) => match m {
                Media::Photo { .. } | Media::Video { .. } => format!("{}: {text}", m.label()),
                _ => text,
            },
            (None, true) => String::new(),
        }
    }

    /// Mentions us (or replies to us).
    pub fn mentions_us(&self) -> bool {
        self.mentioned
    }
}

/// A folder of the chat list, as the account keeps them (Telegram's
/// dialog filters).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: i32,
    pub title: String,
    pub emoticon: Option<String>,
    pub contacts: bool,
    pub non_contacts: bool,
    pub groups: bool,
    pub channels: bool,
    pub bots: bool,
    pub exclude_muted: bool,
    pub exclude_read: bool,
    pub exclude_archived: bool,
    pub include: Vec<ChatId>,
    pub exclude: Vec<ChatId>,
    /// Pinned in this folder, in order.
    pub pinned: Vec<ChatId>,
}

impl Folder {
    /// The folder shows `chat`.
    pub fn admits(&self, chat: &Chat) -> bool {
        if self.pinned.contains(&chat.id) || self.include.contains(&chat.id) {
            return true;
        }
        if self.exclude.contains(&chat.id) {
            return false;
        }
        if self.exclude_muted && chat.muted {
            return false;
        }
        if self.exclude_read && chat.unread == 0 && !chat.marked && chat.mentions == 0 {
            return false;
        }
        if self.exclude_archived && chat.archived {
            return false;
        }
        match chat.kind {
            ChatKind::User | ChatKind::Saved => if chat.contact { self.contacts } else { self.non_contacts },
            ChatKind::Bot => self.bots,
            ChatKind::Group | ChatKind::Forum => self.groups,
            ChatKind::Channel => self.channels,
        }
    }
}

/// A forum's topic.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub id: MsgId,
    pub title: String,
    pub emoji: Option<String>,
    /// Its icon's colour (0xRRGGBB).
    pub color: u32,
    pub unread: u32,
    pub date: i64,
    pub closed: bool,
    pub pinned: bool,
    pub last: Option<Preview>,
}

/// Someone in the account's contacts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub id: ChatId,
    pub name: String,
    pub phone: Option<String>,
    pub username: Option<String>,
    pub photo: Option<i64>,
    pub seen: Option<Seen>,
}

/// Who is signed in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub id: i64,
    pub name: String,
    pub phone: Option<String>,
    pub username: Option<String>,
    pub photo: Option<i64>,
}

/// What someone in a chat is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Doing {
    Typing,
    RecordingVoice,
    RecordingVideo,
    SendingPhoto,
    SendingFile,
    ChoosingSticker,
    Stopped,
}

impl Doing {
    pub fn describe(self) -> &'static str {
        match self {
            Doing::Typing => "typing",
            Doing::RecordingVoice => "recording a voice message",
            Doing::RecordingVideo => "recording a video",
            Doing::SendingPhoto => "sending a photo",
            Doing::SendingFile => "sending a file",
            Doing::ChoosingSticker => "choosing a sticker",
            Doing::Stopped => "",
        }
    }
}

/// UTF-16 offsets (Telegram's) to byte offsets into `text`.
pub fn utf16_to_bytes(text: &str, offset: usize, length: usize) -> (usize, usize) {
    let (mut units, mut start, mut end) = (0usize, None, None);
    for (i, ch) in text.char_indices() {
        if start.is_none() && units >= offset {
            start = Some(i);
        }
        if end.is_none() && units >= offset + length {
            end = Some(i);
            break;
        }
        units += ch.len_utf16();
    }
    let start = start.unwrap_or(text.len());
    (start, end.unwrap_or(text.len()).max(start))
}

/// Byte offsets into `text` to UTF-16 ones (offset, length).
pub fn bytes_to_utf16(text: &str, start: usize, end: usize) -> (i32, i32) {
    let units = |s: &str| s.encode_utf16().count() as i32;
    let (start, end) = (start.min(text.len()), end.min(text.len()));
    let o = units(&text[..start]);
    (o, units(&text[start..end.max(start)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets() {
        // "😀" is two UTF-16 units and four bytes.
        let t = "a😀 bold";
        assert_eq!(utf16_to_bytes(t, 4, 4), (6, 10));
        assert_eq!(&t[6..10], "bold");
        assert_eq!(bytes_to_utf16(t, 6, 10), (4, 4));
        assert_eq!(utf16_to_bytes(t, 1, 2), (1, 5));
        assert_eq!(utf16_to_bytes(t, 40, 2), (t.len(), t.len()));
    }

    #[test]
    fn folders_admit() {
        let f = Folder { groups: true, exclude_muted: true, include: vec![7], ..Folder::default() };
        let g = Chat { id: 1, kind: ChatKind::Group, ..Chat::default() };
        assert!(f.admits(&g));
        assert!(!f.admits(&Chat { muted: true, ..g.clone() }));
        assert!(!f.admits(&Chat { id: 2, kind: ChatKind::User, ..Chat::default() }));
        assert!(f.admits(&Chat { id: 7, kind: ChatKind::User, ..Chat::default() }));
    }

    #[test]
    fn summaries() {
        let m = Message { text: "  hello\n there ".into(), ..Message::default() };
        assert_eq!(m.summary(), "hello there");
        let p = Message { media: Some(Media::Photo { w: 1, h: 1, thumb: None }), text: "look".into(), ..Message::default() };
        assert_eq!(p.summary(), "Photo: look");
        let v = Message { media: Some(Media::Voice { duration: 65.0, waveform: vec![], size: 0 }), ..Message::default() };
        assert_eq!(v.summary(), "Voice message (1:05)");
        assert_eq!(bytes(2_400_000), "2.4 MB");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
