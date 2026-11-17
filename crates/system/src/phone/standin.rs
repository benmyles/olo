//! A stand-in iPhone, for tests and headless sessions: it speaks the
//! phone's side of the profiles over any link (a socket pair), so
//! nothing touches Bluetooth, a phone or anyone's messages. It serves
//! its messages ([`Phone::serve_messages`], the Message Access Server:
//! listings with the text in `subject` and every message `SMS_GSM`, as
//! an iPhone has them; nothing until its owner allows it) and its
//! contacts ([`Phone::serve_contacts`]), and tells its client's
//! Notification Server of a message that comes ([`Phone::tell`]).

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

use super::{
    map::{self, BMessage},
    obex::{self, Asked, Client, Headers, Params, Server, code, id},
    pbap,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub handle: String,
    pub from_name: String,
    pub from: String,
    pub text: String,
    /// As a listing says it (`YYYYMMDDTHHMMSS`).
    pub datetime: String,
    pub read: bool,
}

/// What the stand-in has and what was done to it.
#[derive(Debug, Default)]
pub struct State {
    pub inbox: Vec<Message>,
    /// vCards, as it serves them (the owner's own first).
    pub contacts: String,
    /// Its owner has turned on Show Notifications, and Sync Contacts.
    pub messages_allowed: bool,
    pub contacts_allowed: bool,
    /// Its client asked to be told of messages.
    pub notifying: bool,
    /// What was sent through it: (recipients, text).
    pub sent: Vec<(Vec<String>, String)>,
    /// An Android's habits rather than an iPhone's: a listing's subject
    /// is only a message's first words, and what was sent is in its sent
    /// folder.
    pub android: bool,
    next: u64,
}

/// The stand-in: cheap to clone, each clone the same phone.
#[derive(Clone, Default)]
pub struct Phone(pub Arc<Mutex<State>>);

impl Phone {
    /// A phone with made-up people in it, everything allowed.
    pub fn sample() -> Phone {
        let p = Phone::default();
        {
            let mut s = p.0.lock().unwrap();
            (s.messages_allowed, s.contacts_allowed) = (true, true);
            s.contacts = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:My Number\r\nTEL:+15550100\r\nEND:VCARD\r\n\
                BEGIN:VCARD\r\nVERSION:3.0\r\nN:Kovač;Mira\r\nFN:Mira Kovač\r\nTEL;TYPE=CELL:+1 555 010 0142\r\nEND:VCARD\r\n\
                BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Tomas Lind\r\nTEL:+15550100117\r\nEMAIL:tomas@example.org\r\nEND:VCARD\r\n\
                BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Dad\r\nTEL:(555) 010-0108\r\nEND:VCARD\r\n"
                .to_owned();
        }
        p.receive("Mira Kovač", "+15550100142", "Are you still coming on Saturday?", "20261114T101500");
        p.receive("", "+15550100190", "Your parcel arrives tomorrow between 10:00 and 12:00.", "20261115T083000");
        for m in &mut p.0.lock().unwrap().inbox {
            m.read = true;
        }
        p
    }

    /// A message arrives; its handle. (Told to a client with [`tell`].)
    pub fn receive(&self, from_name: &str, from: &str, text: &str, datetime: &str) -> String {
        let mut s = self.0.lock().unwrap();
        s.next += 1;
        let handle = format!("{:08X}", 0x2000_0000u64 + s.next);
        s.inbox.push(Message { handle: handle.clone(), from_name: from_name.into(), from: from.into(), text: text.into(), datetime: datetime.into(), read: false });
        handle
    }

    /// Whether its client asked to be told of messages.
    pub fn notifying(&self) -> bool {
        self.0.lock().unwrap().notifying
    }

    /// Tell the client's Notification Server (on `link`, a channel to
    /// it) that message `handle` came.
    pub fn tell<L: Read + Write>(&self, link: L, handle: &str) -> obex::Result<()> {
        let mut c = Client::connect(link, &map::MNS)?;
        let report = format!("<MAP-event-report version=\"1.0\"><event type=\"NewMessage\" handle=\"{handle}\" folder=\"TELECOM/MSG/INBOX\" old_folder=\"\" msg_type=\"SMS_GSM\"/></MAP-event-report>");
        c.put(map::kind::EVENT, None, &Params::new().byte(map::tag::MAS_INSTANCE, 0), report.as_bytes())?;
        c.disconnect();
        Ok(())
    }

    /// Serve one session of its messages on `link`, to its end.
    pub fn serve_messages<L: Read + Write>(&self, link: L) -> obex::Result<()> {
        let mut server = Server::new(link);
        let mut path: Vec<String> = Vec::new();
        loop {
            match server.next()? {
                Asked::Connect { target } => {
                    if target.as_deref() != Some(&map::MAS[..]) {
                        server.refuse(code::SERVICE_UNAVAILABLE)?;
                    } else if !self.0.lock().unwrap().messages_allowed {
                        // (As an iPhone before Show Notifications is on.)
                        server.refuse(code::FORBIDDEN)?;
                    } else {
                        server.accept(Some(&map::MAS), 1)?;
                    }
                }
                Asked::SetPath { name, up } => {
                    match name {
                        None if up => {
                            path.pop();
                        }
                        None => path.clear(),
                        Some(n) => path.push(n.to_ascii_lowercase()),
                    }
                    server.done(&Headers::new())?;
                }
                Asked::Get { headers } => {
                    let name = headers.get_text(id::NAME).unwrap_or_default().to_owned();
                    let params = headers.get_bytes(id::APP_PARAMS).map(Params::decode).unwrap_or_default();
                    let s = self.0.lock().unwrap();
                    match headers.kind().as_deref() {
                        Some("x-bt/map-msg-listing") if path == ["telecom", "msg"] => {
                            let most = params.get_two(map::tag::MAX_LIST_COUNT).unwrap_or(1024) as usize;
                            // (An iPhone's sent folder is empty, and
                            // its listing has the whole text; an
                            // Android's has a message's first words.)
                            let android = s.android;
                            let subject = |text: &str| if android { text.chars().take(24).collect::<String>() } else { text.to_owned() };
                            let rows: String = if name.eq_ignore_ascii_case("sent") && android {
                                s.sent
                                    .iter()
                                    .rev()
                                    .take(most)
                                    .enumerate()
                                    .map(|(i, (to, text))| {
                                        format!(
                                            "<msg handle=\"{:08X}\" subject=\"{}\" datetime=\"20261116T120000\" recipient_addressing=\"{}\" type=\"SMS_GSM\" read=\"yes\" sent=\"yes\"/>\n",
                                            0x3000_0000 + i,
                                            xml(&subject(text)),
                                            xml(&to.join(";"))
                                        )
                                    })
                                    .collect()
                            } else if name.eq_ignore_ascii_case("inbox") {
                                s.inbox
                                    .iter()
                                    .rev()
                                    .take(most)
                                    .map(|m| {
                                        format!(
                                            "<msg handle=\"{}\" subject=\"{}\" datetime=\"{}\" sender_name=\"{}\" sender_addressing=\"{}\" type=\"SMS_GSM\" size=\"{}\" read=\"{}\" sent=\"no\"/>\n",
                                            m.handle,
                                            xml(&subject(&m.text)),
                                            m.datetime,
                                            xml(&m.from_name),
                                            xml(&m.from),
                                            m.text.len(),
                                            if m.read { "yes" } else { "no" }
                                        )
                                    })
                                    .collect()
                            } else {
                                String::new()
                            };
                            let body = format!("<MAP-msg-listing version=\"1.0\">\n{rows}</MAP-msg-listing>\n");
                            drop(s);
                            server.send(&Params::new(), body.as_bytes())?;
                        }
                        Some("x-bt/message") => match s.inbox.iter().find(|m| m.handle == name) {
                            Some(m) => {
                                let who = if m.from.contains('@') { format!("EMAIL:{}", m.from) } else { format!("TEL:{}", m.from) };
                                let msg = format!("BEGIN:MSG\r\n{}\r\nEND:MSG\r\n", m.text.replace('\n', "\r\n"));
                                let body = format!(
                                    "BEGIN:BMSG\r\nVERSION:1.0\r\nSTATUS:{}\r\nTYPE:SMS_GSM\r\nFOLDER:TELECOM/MSG/INBOX\r\nBEGIN:VCARD\r\nVERSION:2.1\r\nN:{}\r\n{who}\r\nEND:VCARD\r\nBEGIN:BENV\r\nBEGIN:BBODY\r\nCHARSET:UTF-8\r\nLENGTH:{}\r\n{msg}END:BBODY\r\nEND:BENV\r\nEND:BMSG\r\n",
                                    if m.read { "READ" } else { "UNREAD" },
                                    m.from_name,
                                    msg.len()
                                );
                                drop(s);
                                server.send(&Params::new(), body.as_bytes())?;
                            }
                            None => {
                                drop(s);
                                server.refuse(code::NOT_FOUND)?;
                            }
                        },
                        _ => {
                            drop(s);
                            server.refuse(code::BAD_REQUEST)?;
                        }
                    }
                }
                Asked::Put { headers, body } => {
                    let name = headers.get_text(id::NAME).unwrap_or_default().to_owned();
                    let params = headers.get_bytes(id::APP_PARAMS).map(Params::decode).unwrap_or_default();
                    let mut s = self.0.lock().unwrap();
                    let mut answer = Headers::new();
                    let fine = match headers.kind().as_deref() {
                        Some("x-bt/map-notificationregistration") => {
                            s.notifying = params.get_byte(map::tag::NOTIFICATION_STATUS) == Some(1);
                            true
                        }
                        Some("x-bt/messagestatus") => match s.inbox.iter_mut().find(|m| m.handle == name) {
                            Some(m) if params.get_byte(map::tag::STATUS_INDICATOR) == Some(0) => {
                                m.read = params.get_byte(map::tag::STATUS_VALUE) == Some(1);
                                true
                            }
                            _ => false,
                        },
                        Some("x-bt/message") if name.eq_ignore_ascii_case("outbox") && path == ["telecom", "msg"] => match BMessage::parse(&body) {
                            Some(m) if !m.to.is_empty() && m.kind == "SMS_GSM" => {
                                s.next += 1;
                                answer = answer.text(id::NAME, &format!("{:08X}", 0x3000_0000u64 + s.next));
                                s.sent.push((m.to, m.text));
                                true
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    drop(s);
                    if fine { server.done(&answer)? } else { server.refuse(code::BAD_REQUEST)? }
                }
                Asked::Disconnect => return Ok(()),
            }
        }
    }

    /// Serve one session of its contacts on `link`, to its end.
    pub fn serve_contacts<L: Read + Write>(&self, link: L) -> obex::Result<()> {
        let mut server = Server::new(link);
        loop {
            match server.next()? {
                Asked::Connect { target } => {
                    if target.as_deref() != Some(&pbap::PSE[..]) {
                        server.refuse(code::SERVICE_UNAVAILABLE)?;
                    } else if !self.0.lock().unwrap().contacts_allowed {
                        server.refuse(code::FORBIDDEN)?;
                    } else {
                        server.accept(Some(&pbap::PSE), 1)?;
                    }
                }
                Asked::Get { headers } if headers.kind().as_deref() == Some(pbap::PHONEBOOK) && headers.get_text(id::NAME) == Some("telecom/pb.vcf") => {
                    let params = headers.get_bytes(id::APP_PARAMS).map(Params::decode).unwrap_or_default();
                    let skip = params.get_two(pbap::tag::LIST_START_OFFSET).unwrap_or(0) as usize;
                    let all = self.0.lock().unwrap().contacts.clone();
                    // (From entry `skip` on: entry 0 is the owner's own.)
                    let cards: Vec<&str> = all.split_inclusive("END:VCARD\r\n").collect();
                    let body: String = cards.iter().skip(skip).copied().collect();
                    server.send(&Params::new(), body.as_bytes())?;
                }
                Asked::Get { .. } => server.refuse(code::NOT_FOUND)?,
                Asked::Put { .. } | Asked::SetPath { .. } => server.refuse(code::BAD_REQUEST)?,
                Asked::Disconnect => return Ok(()),
            }
        }
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "&#10;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone::map::{Event, Mas};
    use std::os::unix::net::UnixStream;

    /// A client against the stand-in, end to end: refused until allowed;
    /// the inbox listed; told of a message as it comes, and fetching it;
    /// marking it read; sending one; the contacts, and whose a number is.
    #[test]
    fn a_client_and_the_stand_in_phone() {
        let phone = Phone::sample();
        let session = |phone: &Phone| {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let p = phone.clone();
            (ours, std::thread::spawn(move || p.serve_messages(theirs)))
        };

        // Before its owner allows it: refused, as an iPhone refuses.
        phone.0.lock().unwrap().messages_allowed = false;
        let (link, served) = session(&phone);
        assert!(matches!(Mas::open(link), Err(obex::Error::Refused(code::FORBIDDEN))));
        drop(served);
        phone.0.lock().unwrap().messages_allowed = true;

        let (link, served) = session(&phone);
        let mut mas = Mas::open(link).unwrap();
        let listed = mas.listing("inbox", 20).unwrap();
        assert_eq!(listed.iter().map(|m| (m.sender_name.as_str(), m.read)).collect::<Vec<_>>(), [("", true), ("Mira Kovač", true)], "the newest first");
        assert_eq!(listed[0].subject, "Your parcel arrives tomorrow between 10:00 and 12:00.");
        assert_eq!(mas.listing("sent", 20).unwrap(), []);
        assert!(!phone.notifying());
        mas.notify(true).unwrap();
        assert!(phone.notifying());

        // A message comes: the phone tells our Notification Server.
        let handle = phone.receive("", "22395", "Your Acme Bank verification code is 482913.\nIt expires in 10 minutes.", "20261116T221500");
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (p, h) = (phone.clone(), handle.clone());
        let telling = std::thread::spawn(move || p.tell(theirs, &h));
        let mut heard = Vec::new();
        map::serve_notifications(ours, |e| heard.push(e)).unwrap();
        telling.join().unwrap().unwrap();
        assert_eq!(heard, [Event::New { handle: handle.clone(), folder: "TELECOM/MSG/INBOX".into() }]);
        let m = mas.message(&handle).unwrap();
        assert_eq!((m.from.as_str(), m.read, m.kind.as_str()), ("22395", false, "SMS_GSM"));
        assert_eq!(m.text, "Your Acme Bank verification code is 482913.\nIt expires in 10 minutes.");
        assert_eq!(mas.listing("inbox", 1).unwrap()[0].subject, m.text, "an iPhone's listing has the text");
        mas.mark_read(&handle).unwrap();
        assert!(mas.message(&handle).unwrap().read);
        assert!(matches!(mas.message("0BADC0DE"), Err(obex::Error::Refused(code::NOT_FOUND))));
        assert!(matches!(mas.message("../../x"), Err(obex::Error::Malformed(_))));

        // One sent: as the phone takes it.
        let out = BMessage::outgoing(&["+15550100142"], "See you at ten.").unwrap();
        assert!(mas.send(&out).unwrap().is_some());
        assert_eq!(phone.0.lock().unwrap().sent, [(vec!["+15550100142".to_owned()], "See you at ten.".to_owned())]);
        mas.close();
        served.join().unwrap().unwrap();

        // Its contacts (the owner's own card passed over).
        let (ours, theirs) = UnixStream::pair().unwrap();
        let p = phone.clone();
        let served = std::thread::spawn(move || p.serve_contacts(theirs));
        let contacts = pbap::contacts(ours).unwrap();
        served.join().unwrap().unwrap();
        assert_eq!(contacts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Mira Kovač", "Tomas Lind", "Dad"]);
        assert_eq!(pbap::name_of(&contacts, "+15550100142"), Some("Mira Kovač"));
        assert_eq!(pbap::name_of(&contacts, "22395"), None);
        phone.0.lock().unwrap().contacts_allowed = false;
        let (ours, theirs) = UnixStream::pair().unwrap();
        let p = phone.clone();
        let served = std::thread::spawn(move || p.serve_contacts(theirs));
        assert!(matches!(pbap::contacts(ours), Err(obex::Error::Refused(code::FORBIDDEN))));
        drop(served);
    }
}
