//! The Message Access Profile (MAP 1.4), the client's side: what a phone
//! has in its inbox ([`Mas::listing`]), one message whole
//! ([`Mas::message`]), one sent ([`Mas::send`]), one marked read, and
//! being told of each as it comes: the client asks
//! ([`Mas::notify`]) and the phone then connects back to a server of the
//! client's own, the Message Notification Server, and sends it events
//! ([`Event`], read with [`event`]). Messages travel as bMessages
//! ([`BMessage`]). Written from the specification, and from what an
//! iPhone was seen to do (BlueFerry's protocol notes): an SMS and an
//! iMessage both come as `SMS_GSM`; a listing has the text in `subject`;
//! the sent folder is empty; a message is sent with an empty originator,
//! a recipient's `TEL` or `EMAIL`, and `TYPE:SMS_GSM` whichever way the
//! phone then sends it.

use std::io::{Read, Write};

use super::obex::{self, Client, Headers, Params, id};

/// The Message Access Server's OBEX target, and the Notification
/// Server's.
pub const MAS: [u8; 16] = [0xbb, 0x58, 0x2b, 0x40, 0x42, 0x0c, 0x11, 0xdb, 0xb0, 0xde, 0x08, 0x00, 0x20, 0x0c, 0x9a, 0x66];
pub const MNS: [u8; 16] = [0xbb, 0x58, 0x2b, 0x41, 0x42, 0x0c, 0x11, 0xdb, 0xb0, 0xde, 0x08, 0x00, 0x20, 0x0c, 0x9a, 0x66];

/// Their Bluetooth service classes (what a profile is asked of BlueZ by).
pub const MAS_UUID: &str = "00001132-0000-1000-8000-00805f9b34fb";
pub const MNS_UUID: &str = "00001133-0000-1000-8000-00805f9b34fb";

pub mod kind {
    pub const LISTING: &str = "x-bt/MAP-msg-listing";
    pub const MESSAGE: &str = "x-bt/message";
    pub const NOTIFY: &str = "x-bt/MAP-NotificationRegistration";
    pub const STATUS: &str = "x-bt/messageStatus";
    pub const EVENT: &str = "x-bt/MAP-event-report";
}

/// Application parameters' tags (MAP 1.4, 6.3.1).
pub mod tag {
    pub const MAX_LIST_COUNT: u8 = 0x01;
    pub const ATTACHMENT: u8 = 0x0A;
    pub const TRANSPARENT: u8 = 0x0B;
    pub const RETRY: u8 = 0x0C;
    pub const NOTIFICATION_STATUS: u8 = 0x0E;
    pub const MAS_INSTANCE: u8 = 0x0F;
    pub const SUBJECT_LENGTH: u8 = 0x13;
    pub const CHARSET: u8 = 0x14;
    pub const STATUS_INDICATOR: u8 = 0x17;
    pub const STATUS_VALUE: u8 = 0x18;
}

/// A message as a listing has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listed {
    /// The phone's handle for it (hexadecimal digits).
    pub handle: String,
    /// Its first words (on an iPhone, the text itself).
    pub subject: String,
    /// When, as the phone says it: `YYYYMMDDTHHMMSS`, with `Z` or an
    /// offset when it knows the zone (an iPhone often doesn't say).
    pub datetime: String,
    pub sender_name: String,
    /// A telephone number or an address.
    pub sender: String,
    pub read: bool,
    pub sent: bool,
}

/// A handle is the phone's word for a message: up to 16 hexadecimal
/// digits. Anything else isn't one (and is never sent back as one).
pub fn is_handle(h: &str) -> bool {
    (1..=16).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn attributes(e: &quick_xml::events::BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| {
            let key = a.key.local_name();
            let key: &str = key.as_ref();
            (key.to_ascii_lowercase(), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default())
        })
        .collect()
}

/// The elements called `name` in `xml`, each as its attributes. What
/// isn't XML reads as far as it is.
fn elements(xml: &[u8], name: &str) -> Vec<Vec<(String, String)>> {
    use quick_xml::events::Event as X;
    let text = String::from_utf8_lossy(xml);
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(X::Start(e) | X::Empty(e)) => {
                let local = e.local_name();
                let local: &str = local.as_ref();
                if local.eq_ignore_ascii_case(name) {
                    out.push(attributes(&e));
                }
            }
            Ok(X::Eof) | Err(_) => return out,
            _ => {}
        }
    }
}

/// A listing's messages, the newest first as the phone gives them.
pub fn listing(xml: &[u8]) -> Vec<Listed> {
    elements(xml, "msg")
        .into_iter()
        .map(|a| {
            let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
            Listed {
                handle: get("handle"),
                subject: get("subject"),
                datetime: get("datetime"),
                sender_name: get("sender_name"),
                sender: get("sender_addressing"),
                read: get("read") == "yes",
                sent: get("sent") == "yes",
            }
        })
        .filter(|m| is_handle(&m.handle))
        .collect()
}

/// What the phone tells its client's Notification Server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A message came (its handle, to fetch it by).
    New { handle: String, folder: String },
    /// One was read, deleted or moved on the phone; one sent got through
    /// or didn't; the phone's memory filled: `kind` is the phone's word
    /// ("ReadStatusChanged", "MessageDeleted", "SendingSuccess", …).
    Other { kind: String, handle: String },
}

/// The events in a report (one, as phones send them).
pub fn event(xml: &[u8]) -> Vec<Event> {
    elements(xml, "event")
        .into_iter()
        .filter_map(|a| {
            let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
            let (kind, handle) = (get("type"), get("handle"));
            match kind.as_str() {
                "NewMessage" if is_handle(&handle) => Some(Event::New { handle, folder: get("folder") }),
                // (A new message with no handle to fetch it by is nothing.)
                "NewMessage" | "" => None,
                _ => Some(Event::Other { kind, handle }),
            }
        })
        .collect()
}

/// A message whole, as it travels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BMessage {
    pub read: bool,
    /// "SMS_GSM", "MMS", "EMAIL", ….
    pub kind: String,
    pub folder: String,
    /// Who sent it: a name, and a telephone number or an address.
    pub from_name: String,
    pub from: String,
    /// Who it's to (their numbers or addresses).
    pub to: Vec<String>,
    pub text: String,
}

/// A vCard's name, and its telephone number or its address.
fn vcard_who(lines: &[&str]) -> (String, String) {
    let (mut name, mut full, mut tel, mut email) = (String::new(), String::new(), String::new(), String::new());
    for line in lines {
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.split(';').next().unwrap_or_default().to_ascii_uppercase();
        let value = value.trim();
        match key.as_str() {
            "FN" => full = value.to_owned(),
            // (Family;Given;…: as it reads.)
            "N" => name = value.split(';').filter(|p| !p.is_empty()).rev().collect::<Vec<_>>().join(" "),
            "TEL" if tel.is_empty() => tel = value.to_owned(),
            "EMAIL" if email.is_empty() => email = value.to_owned(),
            _ => {}
        }
    }
    (if full.is_empty() { name } else { full }, if tel.is_empty() { email } else { tel })
}

impl BMessage {
    /// Read one. `None`: not a bMessage.
    pub fn parse(bytes: &[u8]) -> Option<BMessage> {
        let text = String::from_utf8_lossy(bytes);
        let text = text.as_ref();
        text.trim_start().get(..10).filter(|s| s.eq_ignore_ascii_case("BEGIN:BMSG"))?;
        // The message's own text first: it is what its length says, or,
        // where that is wrong, up to the last END:MSG.
        let body_at = text.find("BEGIN:MSG")?;
        let after = &text[body_at + "BEGIN:MSG".len()..];
        let after = after.strip_prefix("\r\n").or_else(|| after.strip_prefix('\n')).unwrap_or(after);
        let end = after.rfind("END:MSG")?;
        let mut body = &after[..end];
        body = body.strip_suffix("\r\n").or_else(|| body.strip_suffix('\n')).unwrap_or(body);
        // (A line of the text that would read as the end is escaped.)
        let body = body.replace("/END\\:MSG", "END:MSG").replace("\r\n", "\n");

        let mut m = BMessage { text: body, ..Default::default() };
        let head = &text[..body_at];
        let lines: Vec<&str> = head.lines().map(str::trim).collect();
        let (mut depth, mut card): (usize, Vec<&str>) = (0, Vec::new());
        let mut in_envelope = false;
        for line in &lines {
            let upper = line.to_ascii_uppercase();
            match upper.as_str() {
                "BEGIN:BENV" => in_envelope = true,
                "BEGIN:VCARD" => {
                    depth += 1;
                    card.clear();
                }
                "END:VCARD" => {
                    depth = depth.saturating_sub(1);
                    let (name, who) = vcard_who(&card);
                    if in_envelope {
                        if !who.is_empty() {
                            m.to.push(who);
                        }
                    } else {
                        (m.from_name, m.from) = (name, who);
                    }
                }
                _ if depth > 0 => card.push(line),
                _ => {
                    if let Some(v) = upper.strip_prefix("STATUS:") {
                        m.read = v == "READ";
                    } else if let Some(v) = upper.strip_prefix("TYPE:") {
                        m.kind = v.to_owned();
                    } else if let Some(v) = line.get(7..).filter(|_| upper.starts_with("FOLDER:")) {
                        m.folder = v.to_owned();
                    }
                }
            }
        }
        Some(m)
    }

    /// A message to send to `to` (telephone numbers or addresses), as an
    /// iPhone takes it. `None` if a recipient isn't one: a recipient is
    /// structure in a bMessage, and what would break out of its line (a
    /// line end, a vCard's own separators) is refused, never escaped.
    pub fn outgoing(to: &[&str], text: &str) -> Option<Vec<u8>> {
        if to.is_empty() || text.is_empty() {
            return None;
        }
        let mut cards = String::new();
        for who in to {
            let who = who.trim();
            let email = who.contains('@');
            let fine = if email {
                who.len() <= 254 && who.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b)) && who.split('@').count() == 2 && !who.starts_with('@') && !who.ends_with('@')
            } else {
                (3..=20).contains(&who.len()) && who.bytes().enumerate().all(|(i, b)| b.is_ascii_digit() || (i == 0 && b == b'+')) && who.bytes().any(|b| b.is_ascii_digit())
            };
            if !fine {
                return None;
            }
            cards += &format!("BEGIN:VCARD\r\nVERSION:2.1\r\n{}:{who}\r\nEND:VCARD\r\n", if email { "EMAIL" } else { "TEL" });
        }
        // The text, its line ends as the format has them, and a line
        // that would read as its end escaped.
        let body: String = text.replace("\r\n", "\n").split('\n').map(|l| if l.trim_end().eq_ignore_ascii_case("END:MSG") { "/END\\:MSG" } else { l }).collect::<Vec<_>>().join("\r\n");
        let msg = format!("BEGIN:MSG\r\n{body}\r\nEND:MSG\r\n");
        Some(
            format!(
                "BEGIN:BMSG\r\nVERSION:1.0\r\nSTATUS:READ\r\nTYPE:SMS_GSM\r\nFOLDER:telecom/msg/outbox\r\nBEGIN:VCARD\r\nVERSION:2.1\r\nN:\r\nEND:VCARD\r\nBEGIN:BENV\r\n{cards}BEGIN:BBODY\r\nCHARSET:UTF-8\r\nLENGTH:{}\r\n{msg}END:BBODY\r\nEND:BENV\r\nEND:BMSG\r\n",
                msg.len()
            )
            .into_bytes(),
        )
    }
}

/// A session with the phone's Message Access Server, in its messages'
/// folder.
pub struct Mas<L: Read + Write> {
    obex: Client<L>,
}

impl<L: Read + Write> Mas<L> {
    /// Open it on `link` (a channel to the phone's MAS). Refused
    /// (`obex::Error::Refused(FORBIDDEN)`): the phone's owner hasn't
    /// turned on Show Notifications for this computer yet.
    pub fn open(link: L) -> obex::Result<Mas<L>> {
        let mut obex = Client::connect(link, &MAS)?;
        obex.set_path(None)?;
        obex.set_path(Some("telecom"))?;
        obex.set_path(Some("msg"))?;
        Ok(Mas { obex })
    }

    /// The newest `most` messages of `folder` ("inbox").
    pub fn listing(&mut self, folder: &str, most: u16) -> obex::Result<Vec<Listed>> {
        let params = Params::new().two(tag::MAX_LIST_COUNT, most).byte(tag::SUBJECT_LENGTH, 255);
        let (body, _) = self.obex.get(kind::LISTING, Some(folder), &params)?;
        Ok(listing(&body))
    }

    /// One message, whole.
    pub fn message(&mut self, handle: &str) -> obex::Result<BMessage> {
        if !is_handle(handle) {
            return Err(obex::Error::Malformed("a handle that isn't one"));
        }
        // (No attachments; its text as UTF-8.)
        let params = Params::new().byte(tag::ATTACHMENT, 0).byte(tag::CHARSET, 1);
        let (body, _) = self.obex.get(kind::MESSAGE, Some(handle), &params)?;
        BMessage::parse(&body).ok_or(obex::Error::Malformed("a message that isn't a bMessage"))
    }

    /// Ask to be told of messages as they come (the phone then connects
    /// to this computer's Notification Server), or to stop being.
    pub fn notify(&mut self, on: bool) -> obex::Result<()> {
        let params = Params::new().byte(tag::NOTIFICATION_STATUS, u8::from(on));
        // (A body there must be: a filler byte, as the profile has it.)
        self.obex.put(kind::NOTIFY, None, &params, &[0x30]).map(|_| ())
    }

    /// Mark a message read (on the phone too).
    pub fn mark_read(&mut self, handle: &str) -> obex::Result<()> {
        if !is_handle(handle) {
            return Err(obex::Error::Malformed("a handle that isn't one"));
        }
        let params = Params::new().byte(tag::STATUS_INDICATOR, 0).byte(tag::STATUS_VALUE, 1);
        self.obex.put(kind::STATUS, Some(handle), &params, &[0x30]).map(|_| ())
    }

    /// Send a message (made with [`BMessage::outgoing`]); the phone's
    /// handle for it, if it says.
    pub fn send(&mut self, bmessage: &[u8]) -> obex::Result<Option<String>> {
        // (Kept in the phone's sent folder; tried again by it; UTF-8.)
        let params = Params::new().byte(tag::TRANSPARENT, 0).byte(tag::RETRY, 1).byte(tag::CHARSET, 1);
        let answer = self.obex.put(kind::MESSAGE, Some("outbox"), &params, bmessage)?;
        Ok(answer.get_text(id::NAME).filter(|h| is_handle(h)).map(str::to_owned))
    }

    pub fn close(self) -> L {
        self.obex.disconnect()
    }

    pub fn link(&self) -> &L {
        self.obex.link()
    }
}

/// Serve one session of this computer's Notification Server on `link`
/// (a channel the phone opened): `told` hears each event as it comes.
/// Returns when the phone ends the session or the link goes.
pub fn serve_notifications<L: Read + Write>(link: L, mut told: impl FnMut(Event)) -> obex::Result<()> {
    use super::obex::{Asked, Server, code};
    let mut server = Server::new(link);
    loop {
        match server.next()? {
            Asked::Connect { target } if target.as_deref() == Some(&MNS[..]) => server.accept(Some(&MNS), 1)?,
            Asked::Connect { .. } => server.refuse(code::SERVICE_UNAVAILABLE)?,
            Asked::Put { headers, body } if headers.kind().as_deref() == Some(&kind::EVENT.to_ascii_lowercase()[..]) => {
                server.done(&Headers::new())?;
                for e in event(&body) {
                    told(e);
                }
            }
            Asked::Put { .. } | Asked::SetPath { .. } => server.refuse(code::BAD_REQUEST)?,
            Asked::Get { .. } => server.refuse(code::NOT_IMPLEMENTED)?,
            Asked::Disconnect => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_and_an_event_as_the_specification_writes_them() {
        // (MAP 1.4, 3.1.6.2's example, shortened; and one that isn't a
        // message at all.)
        let xml = br#"<MAP-msg-listing version = "1.0">
            <msg handle = "20000100001" subject = "Hello &amp; welcome" datetime = "20100804T125700" sender_name = "Jamie" sender_addressing = "+1-987-6543210" recipient_addressing = "+1-0123-456789" type = "SMS_GSM" size = "256" attachment_size = "0" priority = "no" read = "yes" sent = "no" protected = "no"/>
            <msg handle = "20000100002" subject = "Your Acme code is 482913" datetime = "20261116T221500-0800" sender_addressing = "22395" type = "SMS_GSM" read = "no"/>
            <msg handle = "../../etc" subject = "not a handle"/>
        </MAP-msg-listing>"#;
        let l = listing(xml);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].handle.as_str(), l[0].subject.as_str(), l[0].sender_name.as_str(), l[0].sender.as_str(), l[0].read), ("20000100001", "Hello & welcome", "Jamie", "+1-987-6543210", true));
        assert_eq!((l[1].subject.as_str(), l[1].datetime.as_str(), l[1].read, l[1].sent), ("Your Acme code is 482913", "20261116T221500-0800", false, false));
        assert_eq!(listing(b"<MAP-msg-listing version=\"1.0\"/>"), []);
        assert_eq!(listing(b"not xml <msg"), []);

        let report = br#"<MAP-event-report version = "1.0"><event type = "NewMessage" handle = "12345678" folder = "TELECOM/MSG/INBOX" old_folder = "" msg_type = "SMS_GSM" /></MAP-event-report>"#;
        assert_eq!(event(report), [Event::New { handle: "12345678".into(), folder: "TELECOM/MSG/INBOX".into() }]);
        let read = br#"<MAP-event-report version="1.1"><event type="ReadStatusChanged" handle="12345678" folder="TELECOM/MSG/INBOX" read_status="yes"/></MAP-event-report>"#;
        assert_eq!(event(read), [Event::Other { kind: "ReadStatusChanged".into(), handle: "12345678".into() }]);
        assert_eq!(event(br#"<MAP-event-report><event type="NewMessage" handle="x y"/></MAP-event-report>"#), []);
    }

    #[test]
    fn a_message_read_and_one_made() {
        // (MAP 1.4, 3.1.3's example, as an SMS.)
        let b = "BEGIN:BMSG\r\nVERSION:1.0\r\nSTATUS:UNREAD\r\nTYPE:SMS_GSM\r\nFOLDER:TELECOM/MSG/INBOX\r\nBEGIN:VCARD\r\nVERSION:2.1\r\nN:Kovac;Mira\r\nTEL:+15550142\r\nEND:VCARD\r\nBEGIN:BENV\r\nBEGIN:VCARD\r\nVERSION:2.1\r\nN:\r\nTEL:+15550100\r\nEND:VCARD\r\nBEGIN:BBODY\r\nCHARSET:UTF-8\r\nLENGTH:57\r\nBEGIN:MSG\r\nAre you still coming?\r\nBring the seedlings \u{1F331}\r\nEND:MSG\r\nEND:BBODY\r\nEND:BENV\r\nEND:BMSG\r\n";
        let m = BMessage::parse(b.as_bytes()).unwrap();
        assert_eq!((m.read, m.kind.as_str(), m.folder.as_str()), (false, "SMS_GSM", "TELECOM/MSG/INBOX"));
        assert_eq!((m.from_name.as_str(), m.from.as_str(), m.to.as_slice()), ("Mira Kovac", "+15550142", &["+15550100".to_owned()][..]));
        assert_eq!(m.text, "Are you still coming?\nBring the seedlings \u{1F331}");
        // From an address (an iMessage from an Apple ID), and its name in FN.
        let b = "BEGIN:BMSG\nVERSION:1.0\nSTATUS:READ\nTYPE:SMS_GSM\nFOLDER:telecom/msg/inbox\nBEGIN:VCARD\nVERSION:3.0\nFN:Tomas Lind\nN:Lind;Tomas\nEMAIL;TYPE=INTERNET:tomas@example.org\nEND:VCARD\nBEGIN:BENV\nBEGIN:BBODY\nLENGTH:9\nBEGIN:MSG\nok\nEND:MSG\nEND:BBODY\nEND:BENV\nEND:BMSG\n";
        let m = BMessage::parse(b.as_bytes()).unwrap();
        assert_eq!((m.read, m.from_name.as_str(), m.from.as_str(), m.text.as_str()), (true, "Tomas Lind", "tomas@example.org", "ok"));
        assert_eq!(BMessage::parse(b"hello"), None);
        assert_eq!(BMessage::parse(b"BEGIN:BMSG\r\nVERSION:1.0\r\n"), None);

        // One made reads back as it was meant, its length exact.
        let out = BMessage::outgoing(&["+15550142", "tomas@example.org"], "See you at ten.\nEND:MSG\nReally.").unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("TYPE:SMS_GSM\r\n") && text.contains("BEGIN:VCARD\r\nVERSION:2.1\r\nN:\r\nEND:VCARD\r\nBEGIN:BENV\r\n"));
        assert!(text.contains("TEL:+15550142\r\n") && text.contains("EMAIL:tomas@example.org\r\n"));
        let said: usize = text.split("LENGTH:").nth(1).unwrap().lines().next().unwrap().parse().unwrap();
        let (from, to) = (text.find("BEGIN:MSG").unwrap(), text.rfind("END:MSG\r\n").unwrap() + 9);
        assert_eq!(said, to - from);
        let back = BMessage::parse(&out).unwrap();
        assert_eq!((back.text.as_str(), back.to.len(), back.from.as_str()), ("See you at ten.\nEND:MSG\nReally.", 2, ""));
        // A recipient is structure: what would break out of it is refused.
        for bad in ["+1555\r\nTEL:+1666", "mira\r\n@example.org", "a@b@c", "@example.org", "12", "+", "tel:+15550142", "+1555;0142", ""] {
            assert_eq!(BMessage::outgoing(&[bad], "hi"), None, "{bad:?}");
        }
        assert_eq!(BMessage::outgoing(&[], "hi"), None);
        assert_eq!(BMessage::outgoing(&["+15550142"], ""), None);
    }

    #[test]
    fn handles_are_hexadecimal_and_nothing_else() {
        assert!(is_handle("20000100001") && is_handle("00A1B2c3"));
        for bad in ["", "12345678901234567", "../x", "12 34", "outbox", "１２３"] {
            assert!(!is_handle(bad), "{bad:?}");
        }
    }
}
