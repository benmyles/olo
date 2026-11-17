//! Made-up people and messages (`OLO_PHONE_DEMO=1`): what Phone shows
//! with no phone and no service, for the layout lint, the benchmark and
//! pictures. Nobody real: the names, numbers (555 01xx) and words are
//! invented.

use olo_protocol::phone::{Conversation, Kind, Link, Message, Person, PhoneState};

fn message(n: &mut u32, text: &str, at: i64, outgoing: bool, read: bool) -> Message {
    *n += 1;
    Message { handle: format!("{:016X}", *n), text: text.into(), at, outgoing, read }
}

/// The phone's state, its conversations (the latest first) and its
/// contacts, as of `now` (Unix seconds); with `many`, that many more
/// conversations and contacts, and that many more messages (600 at most)
/// before the first conversation's own: what the benchmark scrolls.
pub fn sample(now: i64, many: usize) -> (PhoneState, Vec<Conversation>, Vec<Person>) {
    let (min, hour, day) = (60, 3600, 86_400);
    let mut n = 0;
    let mut m = |text: &str, ago: i64, outgoing: bool, read: bool| message(&mut n, text, now - ago, outgoing, read);
    // (Its count, shared with the many made up below.)
    let talk = |name: &str, address: &str, messages: Vec<Message>| Conversation { key: olo_system::phone::pbap::key(address), name: name.into(), address: address.into(), messages };
    let threads = vec![
        talk(
            "Mira Kovač",
            "+1 (415) 555-0134",
            vec![
                m("Are we still on for Thursday?", day + 3 * hour, false, true),
                m("Yes! The usual place?", day + 3 * hour - 90, true, true),
                m("Perfect. I’ll book a table for seven, they fill up fast on Thursdays and last time we waited almost an hour by the door.", day + 3 * hour - 4 * min, false, true),
                m("Running ten minutes late", 50 * min, false, true),
                m("No rush", 49 * min, true, true),
                m("I’ll grab us a table", 49 * min - 20, true, true),
                m("Here now, by the window ☕", 12 * min, false, false),
                m("Order me a flat white?", 11 * min, false, false),
            ],
        ),
        talk("22395", "22395", vec![m("Your Acme Bank verification code is 482913. It expires in 10 minutes. Don’t share it with anyone.", 26 * min, false, true)]),
        talk(
            "Jun Tanaka",
            "+1 (628) 555-0177",
            vec![
                m("Did the parcel arrive?", 2 * day + 5 * hour, true, true),
                m("This morning! Thank you, it’s exactly the one I wanted.", 2 * day + 4 * hour, false, true),
                m("Photos when it’s set up please", 2 * day + 4 * hour - 60, true, true),
            ],
        ),
        talk("+1 (510) 555-0199", "+1 (510) 555-0199", vec![m("Hi, this is the plumber. I can come by tomorrow between 9 and 11 if that still works for you.", 5 * hour, false, true)]),
        talk("Ada Okafor-Lindqvist", "ada@example.com", vec![m("Sent you the notes from today’s meeting", 9 * day, false, true), m("Got them, thanks", 9 * day - 30 * min, true, true)]),
    ];
    let person =
        |name: &str, phones: &[&str], emails: &[&str]| Person { name: name.into(), phones: phones.iter().map(|p| (*p).into()).collect(), emails: emails.iter().map(|e| (*e).into()).collect() };
    let contacts = vec![
        person("Ada Okafor-Lindqvist", &["+1 (415) 555-0101"], &["ada@example.com"]),
        person("Benedikt von Hohenstaufen-Wittelsbach", &["+49 30 5550 0142"], &[]),
        person("Jun Tanaka", &["+1 (628) 555-0177"], &[]),
        person("Mira Kovač", &["+1 (415) 555-0134", "+1 (415) 555-0135"], &["mira@example.com"]),
        person("Noor Haddad", &["+1 (212) 555-0166"], &[]),
        person("Tomás Ferreira", &["+351 21 555 0123"], &["tomas@example.com"]),
    ];
    let (mut threads, mut contacts) = (threads, contacts);
    if many > 0 {
        const FIRST: [&str; 8] = ["Alba", "Bram", "Chidi", "Dara", "Emre", "Farah", "Goran", "Hana"];
        const LAST: [&str; 7] = ["Arce", "Bianchi", "Cho", "Duval", "Eze", "Falk", "Gashi"];
        const SAID: [&str; 6] = [
            "See you there",
            "Thanks, got it",
            "Can you send me the address again? I can’t find it anywhere in my notes from last week.",
            "On my way",
            "That works for me. Shall we say half past six at the usual place, and I’ll bring the papers?",
            "👍",
        ];
        for i in 0..many {
            let name = format!("{} {} {}", FIRST[i % 8], LAST[i % 7], i / 56 + 1);
            let number = format!("+1 (312) 555-{:04}", 2000 + i % 8000);
            let ago = 10 * day + i as i64 * hour;
            let mut messages = Vec::new();
            for k in 0..3 {
                n += 1;
                messages.push(Message { handle: format!("{n:016X}"), text: SAID[(i + k) % 6].into(), at: now - ago - (3 - k as i64) * min, outgoing: (i + k) % 3 == 0, read: true });
            }
            threads.push(Conversation { key: olo_system::phone::pbap::key(&number), name: name.clone(), address: number.clone(), messages });
            contacts.push(Person { name, phones: vec![number], emails: Vec::new() });
        }
        let mut older = Vec::new();
        for k in 0..many.min(600) {
            n += 1;
            older.push(Message { handle: format!("{n:016X}"), text: SAID[k % 6].into(), at: now - 40 * day + k as i64 * 40 * min, outgoing: k % 3 == 0, read: true });
        }
        older.append(&mut threads[0].messages);
        threads[0].messages = older;
        contacts.sort_by(|a, b| a.name.cmp(&b.name));
    }
    let unread = threads.iter().map(|t| t.unread() as u32).sum();
    let state = PhoneState {
        pid: 1,
        name: "Mira’s iPhone".into(),
        kind: Kind::IPhone,
        link: Link::Ready,
        contacts: true,
        battery: Some(81),
        conversations: threads.len() as u32,
        unread,
        profiles: true,
        ..Default::default()
    };
    (state, threads, contacts)
}
