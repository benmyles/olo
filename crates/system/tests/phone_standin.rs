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
    unsafe {
        std::env::set_var("OLO_PHONE_BLUEZ", &address);
        std::env::set_var("OLO_PHONE_TELEPHONY", &address);
    }
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
        // What it plays: nothing until it has a player; then the track,
        // and this computer its remote control.
        assert_eq!(profiles.music(&phone.path), None);
        tell(&address, &["music", "playing", "So What", "Miles Davis"]);
        let playing = |want: bool| {
            let start = std::time::Instant::now();
            loop {
                match profiles.music(&phone.path) {
                    Some(m) if m.playing == want => return m,
                    _ => assert!(start.elapsed() < Duration::from_secs(5), "its music never came to playing: {want}"),
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let m = playing(true);
        assert_eq!((m.title.as_str(), m.artist.as_str(), m.album.as_str()), ("So What", "Miles Davis", ""));
        profiles.music_act(&m.player, olo_system::phone::music::Action::Pause).unwrap();
        playing(false);
        profiles.music_act(&m.player, olo_system::phone::music::Action::Next).unwrap();
        assert_eq!(tell(&address, &["pressed"]).split_whitespace().collect::<Vec<_>>(), ["pause", "next"]);
        tell(&address, &["music", "none"]);
        assert_eq!(profiles.music(&phone.path), None);

        let (changes, changed) = mpsc::channel();
        profiles
            .watch(move |_| {
                let _ = changes.send(());
            })
            .unwrap();

        // Its messages: refused until the phone's own switch is on.
        tell(&address, &["set", "messages", "no"]);
        profiles.open(&phone.path, Channel::Messages).unwrap();
        assert!(matches!(Mas::open(next(Channel::Messages).stream), Err(obex::Error::Refused(obex::code::FORBIDDEN))));
        // (An iPhone has the switch to turn on only while the computer
        // advertises for it.)
        let _asking = (!android).then(|| profiles.solicit(&phone.path).unwrap());
        let start = std::time::Instant::now();
        while !android && tell(&address, &["soliciting"]).trim() != "yes" {
            assert!(start.elapsed() < Duration::from_secs(5), "the stand-in never heard the advertisement");
            std::thread::sleep(Duration::from_millis(20));
        }
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

        // An iPhone's other notifications: there once its owner allows
        // them (it then connects over low energy to the computer
        // advertising for them), each told of as it comes, what it says
        // asked for and spliced from the answer's packets, and its
        // action done on the phone.
        if !android {
            use std::io::Read as _;

            use olo_system::phone::ancs;
            assert!(profiles.notifications(&phone.path).unwrap().is_none(), "not before it's allowed");
            let before: u32 = tell(&address, &["notify", "com.apple.reminders", "Reminders", "Water the plants", "Today"]).trim().parse().unwrap();
            tell(&address, &["set", "notifications", "yes"]);
            let start = std::time::Instant::now();
            let mut link = loop {
                match profiles.notifications(&phone.path).unwrap() {
                    Some(link) => break link,
                    None => assert!(start.elapsed() < Duration::from_secs(5), "its notification service never came"),
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(profiles.model(&phone.path).as_deref(), Some("iPhone17,3"));
            let (mut source, mut data) = link.streams().unwrap();
            let write = link.writer();
            let mut told = || {
                let mut packet = [0u8; 64];
                let n = source.read(&mut packet).unwrap();
                ancs::told(&packet[..n]).expect("a Notification Source packet")
            };
            let mut reader = ancs::Reader::default();
            let mut answer = || loop {
                let mut packet = [0u8; 64];
                let n = data.read(&mut packet).unwrap();
                assert!(n > 0 && n <= 20, "a packet of {n} bytes");
                if let Some(answer) = reader.push(&packet[..n]).pop() {
                    return answer;
                }
            };
            // (What was on the phone already: said to be so.)
            let old = told();
            assert!(old.pre_existing && old.uid == before);
            let uid: u32 = tell(&address, &["notify", "com.apple.mobilemail", "Mail", "Jun Tanaka", "The photos are up, have a look when you can."]).trim().parse().unwrap();
            let new = told();
            assert_eq!((new.event, new.uid, new.pre_existing, new.positive, new.negative), (ancs::Event::Added, uid, false, false, true));
            write(&ancs::get_notification(uid)).unwrap();
            let says = ancs::Notification {
                uid,
                app: "com.apple.mobilemail".into(),
                title: "Jun Tanaka".into(),
                subtitle: String::new(),
                message: "The photos are up, have a look when you can.".into(),
                positive: String::new(),
                negative: "Clear".into(),
            };
            assert_eq!(answer(), ancs::Answer::Notification(says));
            write(&ancs::get_app("com.apple.mobilemail")).unwrap();
            assert_eq!(answer(), ancs::Answer::App("com.apple.mobilemail".into(), "Mail".into()));
            // (Nonsense, and a notification that isn't: refused.)
            assert!(write(&[9]).is_err() && write(&ancs::get_notification(7)).is_err());
            // Cleared from here: done on the phone, which removes it.
            write(&ancs::act(uid, false)).unwrap();
            assert_eq!((told().event, tell(&address, &["acted"]).trim().to_owned()), (ancs::Event::Removed, "negative Jun Tanaka".to_owned()));
            // The low-energy link goes: its streams end.
            tell(&address, &["set", "low-energy", "no"]);
            let mut packet = [0u8; 64];
            assert_eq!(source.read(&mut packet).unwrap_or(0), 0);
            assert!(profiles.notifications(&phone.path).unwrap().is_none());
        }

        // Its calls taken here: its hands-free side connected, the sound
        // server's telephony service has the phone and the call ringing;
        // it's answered and ended, and one is made.
        {
            use olo_system::phone::calls::{Call, Gateway, State, Telephony};
            let t = Telephony::open().unwrap();
            assert_eq!(t.survey(), (Vec::new(), Vec::new()), "not before it's connected hands-free");
            let until = |what: &str, want: &dyn Fn(&(Vec<Gateway>, Vec<Call>)) -> bool| {
                let start = std::time::Instant::now();
                loop {
                    let now = t.survey();
                    if want(&now) {
                        return now;
                    }
                    assert!(start.elapsed() < Duration::from_secs(5), "never: {what} ({now:?})");
                    std::thread::sleep(Duration::from_millis(20));
                }
            };
            tell(&address, &["call", "+15550100142", "Mira", "Kovač"]);
            profiles.hands_free(&phone.path, true);
            let (gateways, calls) = until("the call ringing there", &|s| s.1.len() == 1);
            assert_eq!(gateways.iter().map(|g| g.address.as_str()).collect::<Vec<_>>(), [phone.address.as_str()]);
            assert_eq!((calls[0].state, calls[0].number.as_str(), calls[0].name.as_str(), calls[0].gateway()), (State::Incoming, "+15550100142", "Mira Kovač", gateways[0].path.as_str()));
            t.answer(&calls[0].path).unwrap();
            until("answered", &|s| s.1.first().is_some_and(|c| c.state == State::Active));
            t.hang_up(&calls[0].path).unwrap();
            until("ended", &|s| s.1.is_empty());
            t.dial(&gateways[0].path, "+15550100190").unwrap();
            let (_, calls) = until("dialling", &|s| s.1.first().is_some_and(|c| c.state == State::Dialing));
            assert_eq!(calls[0].number, "+15550100190");
            tell(&address, &["picked-up"]);
            until("picked up", &|s| s.1.first().is_some_and(|c| c.state == State::Active));
            tell(&address, &["hung-up"]);
            until("over", &|s| s.1.is_empty());
            assert!(tell(&address, &["acted"]).ends_with("answered here Mira Kovač\nhung up Mira Kovač\ndialed +15550100190\n"));
            profiles.hands_free(&phone.path, false);
            until("let go", &|s| s.0.is_empty());
        }

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
