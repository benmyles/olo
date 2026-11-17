//! `olo_system::blueferry` against `blueferry-fake`, end to end. Needs a
//! bus to meet on: run with `OLO_TEST_BUS=unix:path=…` (a private bus:
//! `olo-bus unix:path=/var/tmp/…/bus.sock`), e.g.
//! `OLO_TEST_BUS=unix:path=/var/tmp/olo-dev/phone/bus.sock cargo test -p olo-system --test blueferry_fake`.
//! Without it the test does nothing.

use std::{
    process::{Child, Command},
    sync::mpsc,
    time::{Duration, Instant},
};

use olo_system::blueferry::{self, Client, Event};

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tell(address: &str, words: &[&str]) {
    let done = Command::new(env!("CARGO_BIN_EXE_blueferry-fake")).args(["--address", address, "--tell"]).args(words).status().expect("blueferry-fake --tell");
    assert!(done.success(), "{words:?}");
}

#[test]
fn against_the_fake() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("OLO_PHONE_BUS", &address) };
    let client = Client::connect().expect("the bus");
    assert!(!client.running(), "nothing has the name yet (and asking doesn't start it)");
    let (tx, heard) = mpsc::channel();
    Client::watch(move |e| {
        let _ = tx.send(e);
    })
    .expect("a watch");
    let next = |want: &Event| {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let e = heard.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap_or_else(|_| panic!("never heard {want:?}"));
            if &e == want {
                return;
            }
        }
    };

    let _fake = Fake(Command::new(env!("CARGO_BIN_EXE_blueferry-fake")).args(["--address", &address]).spawn().expect("blueferry-fake"));
    next(&Event::Up);
    assert!(client.running());

    // What it reads.
    let s = client.status().unwrap();
    assert_eq!((s.api, s.connectivity.as_str(), s.battery, s.map, s.pbap), (2, "ready", Some(81), true, true));
    assert!(s.profile.is_some());
    let threads = client.threads(100).unwrap();
    assert_eq!(threads.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Mira Kovač", "Allotment", "+15550190", "Dad", "22395"], "the newest first");
    assert!(threads.iter().all(|t| t.unread() == 0));
    assert_eq!(client.find_contacts("tomas").unwrap().len(), 2);

    // A message arrives: said, and there.
    tell(&address, &["receive", "Acme Bank", "22395", "Your Acme Bank verification code is 482913."]);
    next(&Event::History);
    let threads = client.threads(100).unwrap();
    assert_eq!((threads[0].name.as_str(), threads[0].unread()), ("22395", 1));
    let last = threads[0].messages.last().unwrap();
    assert!(last.body.ends_with("482913.") && !last.outgoing && !last.read && last.at.ends_with("+00:00"));
    assert_eq!(client.mark_read(&threads[0].key).unwrap(), 1);
    next(&Event::History);

    // An answer, and a new message; a group's only with its members.
    let mira = threads.iter().find(|t| t.name == "Mira Kovač").unwrap();
    client.reply(mira, "See you at ten.").unwrap();
    next(&Event::History);
    let group = threads.iter().find(|t| t.is_group).unwrap();
    assert_eq!(group.group_token(), "\n+15550117\n+15550142\ntomas@example.org");
    client.reply(group, "I can help unload.").unwrap();
    let mut other = group.clone();
    other.recipients.pop();
    let refused = client.reply(&other, "x").unwrap_err();
    assert_eq!(blueferry::error_name(&refused), Some("ConfirmationRequired"));
    client.send("+15550163", "Hello Ines").unwrap();
    let threads = client.threads(100).unwrap();
    let ines = threads.iter().find(|t| t.recipients == ["+15550163"]).expect("a new conversation");
    assert_eq!((ines.name.as_str(), ines.messages.len(), ines.messages[0].outgoing), ("Ines Duarte", 1, true), "a contact's name on it");
    let refused = client.send("nobody", "x").unwrap_err();
    assert_eq!(blueferry::said(&refused), "recipient is not a phone number or email address");

    // The phone goes: said, and sending is refused plainly.
    tell(&address, &["set", "link", "reconnecting"]);
    next(&Event::Status);
    let s = client.status().unwrap();
    assert_eq!((s.connectivity.as_str(), s.battery, s.map), ("reconnecting", None, false));
    let refused = client.reply(mira, "still there?").unwrap_err();
    assert_eq!((blueferry::error_name(&refused), blueferry::said(&refused).as_str()), (Some("NotReady"), "The phone isn't ready yet."));
    assert_eq!(client.reconnect().unwrap(), "started");
    next(&Event::Status);

    // Its own popups and code copying, switched.
    assert_eq!(client.set_notification_policy("none").unwrap(), "none");
    client.set_otp_autocopy(true).unwrap();
    let s = client.status().unwrap();
    assert_eq!((s.notification_policy.as_str(), s.otp_autocopy), ("none", true));

    // The backend goes: said, and nothing answers.
    drop(_fake);
    next(&Event::Down);
    assert!(!client.running());
    assert_eq!(blueferry::said(&client.status().unwrap_err()), "The phone's backend isn't running.");
}
