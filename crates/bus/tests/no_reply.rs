//! A call whose recipient goes without answering it is answered by the
//! bus (`org.freedesktop.DBus.Error.NoReply`), as dbus-daemon and
//! dbus-broker answer it: a zbus caller has no timeout of its own and
//! would wait forever. A call that was answered isn't answered again.

mod common;

use std::{sync::mpsc, time::Duration};

use zbus::{
    blocking::{Connection, MessageIterator, connection::Builder},
    message,
};

fn bus(name: &str) -> (common::Bus, String) {
    common::bus(name, Duration::from_secs(20)).expect("the bus never said it was ready")
}

/// A service on `address` that takes one call, then `answer`s it or not
/// and goes.
fn service(address: &str, answer: bool) -> std::thread::JoinHandle<()> {
    let service = Builder::address(address).unwrap().name("org.olo.Test.Service").unwrap().build().unwrap();
    std::thread::spawn(move || {
        let call = MessageIterator::from(&service).find_map(|m| m.ok().filter(|m| m.message_type() == message::Type::MethodCall && m.header().member().is_some_and(|n| n == "Hang"))).unwrap();
        if answer {
            service.reply(&call.header(), &("answered",)).unwrap();
        }
        drop(service);
    })
}

fn call(caller: &Connection) -> zbus::Result<zbus::Message> {
    caller.call_method(Some("org.olo.Test.Service"), "/", Some("org.olo.Test"), "Hang", &())
}

#[test]
fn a_call_its_recipient_leaves_unanswered_is_answered_by_the_bus() {
    let (_bus, address) = bus("noreply");
    let gone = service(&address, false);
    let caller = Builder::address(address.as_str()).unwrap().build().unwrap();
    let (tx, rx) = mpsc::channel();
    let c = caller.clone();
    std::thread::spawn(move || _ = tx.send(call(&c)));
    let answer = rx.recv_timeout(Duration::from_secs(10)).expect("the caller was left waiting");
    gone.join().unwrap();
    match answer {
        Err(zbus::Error::MethodError(name, _, _)) => assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.NoReply"),
        other => panic!("not NoReply: {other:?}"),
    }
}

#[test]
fn an_answered_call_is_not_answered_again() {
    let (_bus, address) = bus("answered");
    let gone = service(&address, true);
    let caller = Builder::address(address.as_str()).unwrap().build().unwrap();
    let errors = MessageIterator::from(&caller);
    let reply = call(&caller).expect("its answer");
    assert_eq!(reply.body().deserialize::<String>().unwrap(), "answered");
    gone.join().unwrap();
    // Nothing more for it: the service's going answers nothing.
    let serial = reply.header().reply_serial().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for m in errors.flatten() {
            if m.message_type() == message::Type::Error && m.header().reply_serial() == Some(serial) {
                _ = tx.send(());
            }
        }
    });
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "answered twice");
}
