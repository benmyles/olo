//! What a program says first is heard, and one that says nothing holds
//! nobody up.
//!
//! A client sends its `Hello` in the same write as the end of its
//! handshake. The bus used to listen for a peer's messages only after
//! its connection had begun to read them, so a `Hello` was sometimes
//! dropped and its sender waited for ever (the bus's own services'
//! client among them: such a bus never said "ready"); and it kept the
//! list of peers locked while a program was still saying who it was, so
//! one that connected and went quiet stopped all the others. And it sent
//! a signal to everyone with the lists locked, so one program that had
//! stopped reading stopped them too.

mod common;

use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::{fs::MetadataExt as _, net::UnixStream},
    sync::mpsc,
    time::Duration,
};

use common::bus;
use zbus::blocking::connection::Builder;

/// Many buses started at once (each waits for its own services' client,
/// whose `Hello` it must hear), then many programs connecting to one at
/// once: every one is answered. Before, with the machine busy, one start
/// in seventy never said "ready" (measured: 2 of 192 at 32 at once, 21
/// of 768 at 128), so 384 starts all but always showed it.
#[test]
fn every_hello_is_answered() {
    let within = Duration::from_secs(20);
    let mut buses = Vec::new();
    for wave in 0..6 {
        // (The wave before is ended first: 64 at once, not 128.)
        buses.clear();
        let starts: Vec<_> = (0..64).map(|i| std::thread::spawn(move || bus(&format!("start{wave}-{i}"), within))).collect();
        buses = starts.into_iter().map(|t| t.join().unwrap()).collect();
        let silent = buses.iter().filter(|b| b.is_none()).count();
        assert_eq!(silent, 0, "{silent} of 64 buses started at once never said they were ready (wave {wave})");
    }

    let (_bus, address) = buses.pop().flatten().unwrap();
    drop(buses);
    let (tx, rx) = mpsc::channel();
    let (threads, each) = (32, 12);
    for _ in 0..threads {
        let (tx, address) = (tx.clone(), address.clone());
        std::thread::spawn(move || {
            for _ in 0..each {
                let named = Builder::address(address.as_str()).unwrap().build().is_ok_and(|c| c.unique_name().is_some());
                let _ = tx.send(named);
            }
        });
    }
    drop(tx);
    let mut answered = 0;
    while answered < threads * each {
        match rx.recv_timeout(within) {
            Ok(true) => answered += 1,
            Ok(false) => panic!("a program couldn't connect"),
            Err(_) => panic!("only {answered} of {} programs were answered: the others' Hello went unheard", threads * each),
        }
    }
}

/// A program that connects and says nothing (never who it is) doesn't
/// stop another from connecting and being answered.
#[test]
fn a_silent_program_holds_nobody_up() {
    let (bus, address) = bus("silent", Duration::from_secs(20)).expect("the bus never said it was ready");
    let _silent = UnixStream::connect(&bus.1).expect("connect");
    // (Its connection is the bus's to take in before the next one's.)
    std::thread::sleep(Duration::from_millis(200));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let conn = Builder::address(address.as_str()).unwrap().build();
        let _ = tx.send(conn.is_ok_and(|c| c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetId", &()).is_ok()));
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(true), "the bus answered nobody while one program stayed silent");
}

/// A program on the bus that asks for every signal and then reads
/// nothing more (stopped, hung): its end of the connection, to keep.
fn deaf_listener(socket: &str) -> UnixStream {
    let mut s = UnixStream::connect(socket).expect("connect");
    let uid: String = std::fs::metadata("/proc/self").unwrap().uid().to_string().bytes().map(|b| format!("{b:02x}")).collect();
    s.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes()).unwrap();
    let mut ok = String::new();
    BufReader::new(s.try_clone().unwrap()).read_line(&mut ok).unwrap();
    assert!(ok.starts_with("OK "), "the handshake: {ok:?}");
    s.write_all(b"BEGIN\r\n").unwrap();
    fn call(member: &'static str) -> zbus::message::Builder<'static> {
        zbus::Message::method_call("/org/freedesktop/DBus", member).unwrap().destination("org.freedesktop.DBus").unwrap().interface("org.freedesktop.DBus").unwrap()
    }
    s.write_all(call("Hello").build(&()).unwrap().data().bytes()).unwrap();
    s.write_all(call("AddMatch").build(&("type='signal'",)).unwrap().data().bytes()).unwrap();
    s
}

/// A program that has stopped reading, with more signals for it than its
/// socket holds, doesn't stop another from connecting and being answered.
#[test]
fn a_program_that_reads_nothing_holds_nobody_up() {
    let (bus, address) = bus("deaf", Duration::from_secs(20)).expect("the bus never said it was ready");
    let _deaf = deaf_listener(&bus.1);
    std::thread::sleep(Duration::from_millis(200));
    // 16 MB of signals to everyone: far more than a socket takes unread.
    let loud = Builder::address(address.as_str()).unwrap().build().unwrap();
    std::thread::spawn(move || {
        let much = "x".repeat(64 * 1024);
        for _ in 0..256 {
            if loud.emit_signal(None::<&str>, "/org/example", "org.example.Loud", "Much", &(much.as_str(),)).is_err() {
                break;
            }
        }
    });
    // (Long enough for the deaf one's socket to fill.)
    std::thread::sleep(Duration::from_secs(1));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let conn = Builder::address(address.as_str()).unwrap().build();
        let _ = tx.send(conn.is_ok_and(|c| c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetId", &()).is_ok()));
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(true), "the bus answered nobody while one program read nothing");
}

/// One that reads nothing with more waiting for it than the bus keeps
/// for anyone (64 MB) is let go, as other brokers let it go: its
/// connection ends, and the bus answers on.
#[test]
fn a_program_too_far_behind_is_let_go() {
    use std::io::Read as _;

    let (bus, address) = bus("behind", Duration::from_secs(20)).expect("the bus never said it was ready");
    let mut deaf = deaf_listener(&bus.1);
    std::thread::sleep(Duration::from_millis(200));
    let loud = Builder::address(address.as_str()).unwrap().build().unwrap();
    let much = "x".repeat(1 << 20);
    for _ in 0..80 {
        loud.emit_signal(None::<&str>, "/org/example", "org.example.Loud", "Much", &(much.as_str(),)).expect("the bus stopped taking signals");
    }
    // What it is still sent, read to its end: the end comes, and before
    // all 80 MB have.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut read, mut buf) = (0usize, vec![0u8; 1 << 20]);
        while let Ok(n @ 1..) = deaf.read(&mut buf) {
            read += n;
        }
        let _ = tx.send(read);
    });
    let read = rx.recv_timeout(Duration::from_secs(30)).expect("its connection never ended");
    assert!(read < 80 << 20, "it was sent everything ({read} bytes): nothing let it go");
    let answered = loud.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetId", &());
    assert!(answered.is_ok(), "the bus stopped answering: {answered:?}");
}
