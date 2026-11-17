//! An iPhone's other notifications: Apple's Notification Center Service
//! (ANCS), which an iPhone offers over low energy to a paired computer
//! its owner allowed (Show Notifications): every notification on the
//! phone as it's added, changed and removed, what it says when asked,
//! and its actions (Answer and Decline of a call, Clear).
//!
//! The phone is the provider; three characteristics:
//!
//! - **Notification Source** (notified): eight bytes a notification:
//!   what happened, its flags, its category and the category's count,
//!   and its number ([`told`]).
//! - **Control Point** (written): ask what a notification says
//!   ([`get_notification`]), an app's name ([`get_app`]), or do one of a
//!   notification's actions ([`act`]).
//! - **Data Source** (notified): the answers, each cut into as many
//!   packets as the link needs, spliced back together here ([`Reader`]).
//!
//! Written from Apple's specification (Apple Notification Center
//! Service Specification, and its appendix's numbers). Only the bytes:
//! [`super::link`] has the characteristics from BlueZ.

/// The service and its characteristics.
pub const SERVICE: &str = "7905f431-b5ce-4e99-a40f-4b1e122d00d0";
pub const NOTIFICATION_SOURCE: &str = "9fbf120d-6301-42d9-8c58-25e699a21dbd";
pub const CONTROL_POINT: &str = "69d1d8f3-45e1-49a8-9821-9bbdfdaad9d9";
pub const DATA_SOURCE: &str = "22eac6e9-24d6-4bb5-be44-b36ace7c7bfb";

/// The app whose notifications are the phone's text messages (which
/// come whole another way, the Message Access Profile).
pub const MESSAGES_APP: &str = "com.apple.MobileSMS";

/// What happened to a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Added,
    Modified,
    Removed,
}

/// A notification's category, as far as it matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    IncomingCall,
    MissedCall,
    Voicemail,
    /// Social, schedule, email, news and the rest.
    Other,
}

/// A Notification Source packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Told {
    pub event: Event,
    /// Delivered quietly on the phone.
    pub silent: bool,
    pub important: bool,
    /// It was there before this computer asked to be told.
    pub pre_existing: bool,
    /// It has a positive action (Answer), a negative one (Decline, Clear).
    pub positive: bool,
    pub negative: bool,
    pub category: Category,
    /// The phone's number for it, good until the link goes.
    pub uid: u32,
}

/// A Notification Source packet read (none: not one).
pub fn told(bytes: &[u8]) -> Option<Told> {
    let &[event, flags, category, _count, a, b, c, d] = bytes.get(..8)? else { return None };
    Some(Told {
        event: match event {
            0 => Event::Added,
            1 => Event::Modified,
            2 => Event::Removed,
            _ => return None,
        },
        silent: flags & 1 != 0,
        important: flags & 2 != 0,
        pre_existing: flags & 4 != 0,
        positive: flags & 8 != 0,
        negative: flags & 16 != 0,
        category: match category {
            1 => Category::IncomingCall,
            2 => Category::MissedCall,
            3 => Category::Voicemail,
            _ => Category::Other,
        },
        uid: u32::from_le_bytes([a, b, c, d]),
    })
}

/// What's asked of a notification, in this order (the answer's too):
/// its app, title, subtitle, message and the labels of its actions. A
/// title, subtitle and message come cut to a length asked for.
const ASKED: [(u8, Option<u16>); 6] = [(0, None), (1, Some(160)), (2, Some(160)), (3, Some(1024)), (6, None), (7, None)];

/// The command asking what notification `uid` says.
pub fn get_notification(uid: u32) -> Vec<u8> {
    let mut command = vec![0];
    command.extend_from_slice(&uid.to_le_bytes());
    for (id, most) in ASKED {
        command.push(id);
        if let Some(most) = most {
            command.extend_from_slice(&most.to_le_bytes());
        }
    }
    command
}

/// The command asking app `id`'s name as the phone shows it.
pub fn get_app(id: &str) -> Vec<u8> {
    let mut command = vec![1];
    command.extend_from_slice(id.as_bytes());
    command.extend_from_slice(&[0, 0]);
    command
}

/// The command doing notification `uid`'s positive action (Answer) or
/// its negative one (Decline, Clear).
pub fn act(uid: u32, positive: bool) -> Vec<u8> {
    let mut command = vec![2];
    command.extend_from_slice(&uid.to_le_bytes());
    command.push(if positive { 0 } else { 1 });
    command
}

/// What a notification says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Notification {
    pub uid: u32,
    /// Its app's identifier ("com.apple.mobilemail").
    pub app: String,
    pub title: String,
    pub subtitle: String,
    pub message: String,
    /// Its actions' labels ("Answer", "Decline", "Clear"; empty: none).
    pub positive: String,
    pub negative: String,
}

/// An answer on the Data Source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Notification(Notification),
    /// An app's identifier and its name.
    App(String, String),
}

/// The Data Source's packets spliced into answers: an answer comes in
/// as many packets as the link needs, and is whole when every attribute
/// asked for has come.
#[derive(Debug, Default)]
pub struct Reader {
    buffer: Vec<u8>,
}

/// The attribute tuples at `at` in `bytes` (id, length, value), `count`
/// of them: their values and where they end; none while some is missing.
fn tuples(bytes: &[u8], mut at: usize, count: usize) -> Option<(Vec<String>, usize)> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let length = u16::from_le_bytes([*bytes.get(at + 1)?, *bytes.get(at + 2)?]) as usize;
        let value = bytes.get(at + 3..at + 3 + length)?;
        values.push(String::from_utf8_lossy(value).trim().to_owned());
        at += 3 + length;
    }
    Some((values, at))
}

impl Reader {
    /// A packet came: the answers it completes (usually none or one).
    pub fn push(&mut self, packet: &[u8]) -> Vec<Answer> {
        self.buffer.extend_from_slice(packet);
        let mut answers = Vec::new();
        loop {
            let b = &self.buffer;
            let done = match b.first() {
                None => break,
                Some(0) => {
                    let Some(uid) = b.get(1..5) else { break };
                    let uid = u32::from_le_bytes([uid[0], uid[1], uid[2], uid[3]]);
                    let Some((v, end)) = tuples(b, 5, ASKED.len()) else { break };
                    let mut v = v.into_iter();
                    let mut next = || v.next().unwrap_or_default();
                    answers.push(Answer::Notification(Notification { uid, app: next(), title: next(), subtitle: next(), message: next(), positive: next(), negative: next() }));
                    end
                }
                Some(1) => {
                    let Some(nul) = b[1..].iter().position(|&x| x == 0) else { break };
                    let id = String::from_utf8_lossy(&b[1..1 + nul]).into_owned();
                    let Some((v, end)) = tuples(b, 2 + nul, 1) else { break };
                    answers.push(Answer::App(id, v.into_iter().next().unwrap_or_default()));
                    end
                }
                // (Not an answer's start: what's held makes no sense.)
                Some(_) => b.len(),
            };
            self.buffer.drain(..done);
        }
        answers
    }

    /// Let go of an answer that never became whole (its command is given
    /// up on: what comes next is another's start).
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    pub fn waiting(&self) -> bool {
        !self.buffer.is_empty()
    }
}

/// An answer's bytes, as a phone sends them (the stand-in's, and tests').
pub fn answer_bytes(answer: &Answer) -> Vec<u8> {
    let tuple = |out: &mut Vec<u8>, id: u8, value: &str| {
        out.push(id);
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    };
    let mut out = Vec::new();
    match answer {
        Answer::Notification(n) => {
            out.push(0);
            out.extend_from_slice(&n.uid.to_le_bytes());
            for ((id, _), value) in ASKED.iter().zip([&n.app, &n.title, &n.subtitle, &n.message, &n.positive, &n.negative]) {
                tuple(&mut out, *id, value);
            }
        }
        Answer::App(id, name) => {
            out.push(1);
            out.extend_from_slice(id.as_bytes());
            out.push(0);
            tuple(&mut out, 0, name);
        }
    }
    out
}

/// A Notification Source packet's bytes (the stand-in's, and tests').
pub fn told_bytes(t: &Told, count: u8) -> Vec<u8> {
    let event = match t.event {
        Event::Added => 0,
        Event::Modified => 1,
        Event::Removed => 2,
    };
    let flags = u8::from(t.silent) | u8::from(t.important) << 1 | u8::from(t.pre_existing) << 2 | u8::from(t.positive) << 3 | u8::from(t.negative) << 4;
    let category = match t.category {
        Category::IncomingCall => 1,
        Category::MissedCall => 2,
        Category::Voicemail => 3,
        Category::Other => 4,
    };
    let mut out = vec![event, flags, category, count];
    out.extend_from_slice(&t.uid.to_le_bytes());
    out
}

/// What a command asks (the stand-in's reading of the Control Point).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    GetNotification(u32),
    GetApp(String),
    Act { uid: u32, positive: bool },
}

/// A Control Point command read (none: malformed, which a phone refuses).
pub fn command(bytes: &[u8]) -> Option<Command> {
    let uid = |b: &[u8]| Some(u32::from_le_bytes([*b.get(1)?, *b.get(2)?, *b.get(3)?, *b.get(4)?]));
    match *bytes.first()? {
        0 => Some(Command::GetNotification(uid(bytes)?)),
        1 => {
            let nul = bytes[1..].iter().position(|&x| x == 0)?;
            Some(Command::GetApp(String::from_utf8_lossy(&bytes[1..1 + nul]).into_owned()))
        }
        2 if bytes.len() == 6 => Some(Command::Act { uid: uid(bytes)?, positive: bytes[5] == 0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_source_says() {
        // Added, important with both actions, an incoming call, the
        // category's first, number 0x01020304.
        let t = told(&[0, 0x1A, 1, 1, 4, 3, 2, 1]).unwrap();
        assert_eq!(t, Told { event: Event::Added, silent: false, important: true, pre_existing: false, positive: true, negative: true, category: Category::IncomingCall, uid: 0x0102_0304 });
        assert_eq!(told(&told_bytes(&t, 1)), Some(t));
        let old = told(&[0, 0x05, 6, 12, 9, 0, 0, 0]).unwrap();
        assert!(old.silent && old.pre_existing && !old.positive && old.category == Category::Other && old.uid == 9);
        assert_eq!(told(&[2, 0, 4, 0, 9, 0, 0, 0]).map(|t| t.event), Some(Event::Removed));
        assert_eq!(told(&[7, 0, 4, 0, 9, 0, 0, 0]), None, "no such event");
        assert_eq!(told(&[0, 0, 4]), None, "too short");
    }

    #[test]
    fn the_commands() {
        // The app; the title, subtitle and message with how much of
        // each; the two labels.
        assert_eq!(get_notification(0x0102_0304), [0, 4, 3, 2, 1, 0, 1, 160, 0, 2, 160, 0, 3, 0, 4, 6, 7]);
        assert_eq!(get_app("com.apple.mobilemail"), [&[1][..], b"com.apple.mobilemail", &[0, 0]].concat());
        assert_eq!(act(9, true), [2, 9, 0, 0, 0, 0]);
        assert_eq!(act(9, false), [2, 9, 0, 0, 0, 1]);
        assert_eq!(command(&get_notification(7)), Some(Command::GetNotification(7)));
        assert_eq!(command(&get_app("a.b")), Some(Command::GetApp("a.b".into())));
        assert_eq!(command(&act(7, false)), Some(Command::Act { uid: 7, positive: false }));
        assert_eq!(command(&[2, 7, 0]), None);
        assert_eq!(command(&[9]), None);
    }

    /// An answer whole, in pieces of every size, two in a row, and one
    /// given up on.
    #[test]
    fn answers_spliced() {
        let n = Notification {
            uid: 77,
            app: "com.apple.mobilemail".into(),
            title: "Jun Tanaka".into(),
            subtitle: "Photos".into(),
            message: "They’re up, have a look when you can.".into(),
            positive: String::new(),
            negative: "Clear".into(),
        };
        let whole = answer_bytes(&Answer::Notification(n.clone()));
        assert_eq!(Reader::default().push(&whole), [Answer::Notification(n.clone())]);
        for size in [1, 5, 20, 23, 182] {
            let mut r = Reader::default();
            let mut got = Vec::new();
            for piece in whole.chunks(size) {
                assert!(got.is_empty(), "whole before its end at {size}");
                got = r.push(piece);
            }
            assert_eq!(got, [Answer::Notification(n.clone())], "in pieces of {size}");
            assert!(!r.waiting());
        }
        let app = Answer::App("com.apple.mobilemail".into(), "Mail".into());
        let both = [answer_bytes(&app), whole.clone()].concat();
        assert_eq!(Reader::default().push(&both), [app.clone(), Answer::Notification(n.clone())]);
        // Half an answer, given up on; the next is read from its start.
        let mut r = Reader::default();
        assert!(r.push(&whole[..30]).is_empty() && r.waiting());
        r.reset();
        assert_eq!(r.push(&answer_bytes(&app)), [app]);
        // Nonsense is dropped.
        assert!(r.push(&[9, 9, 9]).is_empty() && !r.waiting());
    }
}
