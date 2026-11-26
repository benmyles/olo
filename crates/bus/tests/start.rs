//! What a program says first is heard, and one that says nothing holds
//! nobody up.
//!
//! A client sends its `Hello` in the same write as the end of its
//! handshake. The bus used to listen for a peer's messages only after
//! its connection had begun to read them, so a `Hello` was sometimes
//! dropped and its sender waited for ever (the bus's own services'
//! client among them: such a bus never said "ready"); and it kept the
//! list of peers locked while a program was still saying who it was, so
//! one that connected and went quiet stopped all the others.

mod common;

use std::{os::unix::net::UnixStream, sync::mpsc, time::Duration};

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
