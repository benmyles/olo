//! `olo_system::phone` against `phone-standin`, end to end: Olo's
//! profiles registered with a stand-in BlueZ, channels opened through
//! it, and the profiles spoken over them to a stand-in phone. Needs a
//! bus to meet on: run with `OLO_TEST_BUS=unix:path=…` (a private bus:
//! `olo-bus unix:path=/var/tmp/…/bus.sock`). Without it the test does
//! nothing.

use std::{
    process::{Child, Command},
    sync::mpsc,
    time::{Duration, Instant},
};

use olo_system::phone::{
    link::{Channel, Kind, Opened, Profiles},
    map::{self, BMessage, Event, Mas},
    obex, pbap,
};

struct Standin(Child);

impl Drop for Standin {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tell(address: &str, words: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_phone-standin")).args(["--address", address, "--tell"]).args(words).output().expect("phone-standin --tell");
    assert!(out.status.success(), "{words:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn standin(address: &str, more: &[&str]) -> Standin {
    let child = Standin(Command::new(env!("CARGO_BIN_EXE_phone-standin")).args(["--address", address]).args(more).spawn().expect("phone-standin"));
    let bus = zbus::blocking::connection::Builder::address(address).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).unwrap();
    let start = Instant::now();
    while !dbus.name_has_owner("org.bluez".try_into().unwrap()).unwrap_or(false) {
        assert!(start.elapsed() < Duration::from_secs(10), "phone-standin didn't start");
        std::thread::sleep(Duration::from_millis(30));
    }
    child
}

#[test]
fn against_the_standin() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("OLO_PHONE_BLUEZ", &address) };
    for android in [false, true] {
        let _standin = standin(&address, if android { &["--android"] } else { &[] });
        let (tx, opened) = mpsc::channel::<Opened>();
        let profiles = Profiles::register(move |o| {
            let _ = tx.send(o);
        })
        .expect("the profiles registered");
        let next = |want: Channel| {
            let o = opened.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("no {want:?} channel came"));
            assert_eq!(o.channel, want);
            o
        };
        // (A second set can't be registered: the first has them.)
        let again = Profiles::register(|_| {}).err().expect("refused");
        assert!(again.to_string().contains("AlreadyExists"), "{again}");

        let phones = profiles.phones().unwrap();
        assert_eq!(phones.len(), 1);
        let phone = &phones[0];
        assert_eq!((phone.kind, phone.connected, phone.contacts), (if android { Kind::Other } else { Kind::IPhone }, true, true));
        assert_eq!(phone.name, if android { "Pixel 11" } else { "Mira’s iPhone" });
        let (changes, changed) = mpsc::channel();
        profiles
            .watch(move || {
                let _ = changes.send(());
            })
            .unwrap();

        // Its messages: refused until the phone's own switch is on.
        tell(&address, &["set", "messages", "no"]);
        profiles.open(&phone.path, Channel::Messages).unwrap();
        assert!(matches!(Mas::open(next(Channel::Messages).stream), Err(obex::Error::Refused(obex::code::FORBIDDEN))));
        tell(&address, &["set", "messages", "yes"]);
        profiles.open(&phone.path, Channel::Messages).unwrap();
        let mut mas = Mas::open(next(Channel::Messages).stream).unwrap();
        let listed = mas.listing("inbox", 20).unwrap();
        assert_eq!(listed.len(), 2);
        mas.notify(true).unwrap();

        // A message comes: the phone opens a channel to Olo's own
        // Notification Server and tells it; the message is fetched whole
        // (an Android's listing has only its first words).
        let text = "Your Acme Bank verification code is 482913. It expires in 10 minutes.";
        tell(&address, &["receive", "", "22395", text]);
        let notifications = next(Channel::Notifications);
        let mut heard = Vec::new();
        map::serve_notifications(notifications.stream, |e| heard.push(e)).unwrap();
        let [Event::New { handle, .. }] = heard.as_slice() else { panic!("{heard:?}") };
        let m = mas.message(handle).unwrap();
        assert_eq!((m.text.as_str(), m.from.as_str(), m.read), (text, "22395", false));
        let subject = mas.listing("inbox", 1).unwrap().remove(0).subject;
        assert_eq!(subject, if android { "Your Acme Bank verificat" } else { text });

        // One sent, and what the phone then has of it.
        mas.send(&BMessage::outgoing(&["+15550100142"], "See you at ten.").unwrap()).unwrap();
        assert_eq!(tell(&address, &["sent"]), "+15550100142: See you at ten.\n");
        assert_eq!(mas.listing("sent", 5).unwrap().len(), usize::from(android), "an iPhone's sent folder is empty");
        mas.close();

        // Its contacts.
        profiles.open(&phone.path, Channel::Contacts).unwrap();
        let contacts = pbap::contacts(next(Channel::Contacts).stream).unwrap();
        assert_eq!(pbap::name_of(&contacts, "+1 (555) 010-0142"), Some("Mira Kovač"));

        // Out of reach: said, and nothing opens.
        while changed.try_recv().is_ok() {}
        tell(&address, &["set", "connected", "no"]);
        changed.recv_timeout(Duration::from_secs(10)).expect("told that the phone went");
        assert!(!profiles.phones().unwrap()[0].connected);
        assert!(profiles.open(&phone.path, Channel::Messages).is_err());
        // Unpaired: no phone.
        tell(&address, &["set", "paired", "no"]);
        assert_eq!(profiles.phones().unwrap(), []);
    }
}
