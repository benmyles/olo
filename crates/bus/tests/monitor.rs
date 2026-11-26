//! A program that asks to watch the bus (`BecomeMonitor`: dbus-monitor,
//! busctl monitor) is answered, and the bus goes on answering the others
//! while it watches.

mod common;

use std::{sync::mpsc, time::Duration};

use common::bus;
use zbus::blocking::{Connection, connection::Builder};

const DBUS: (&str, &str) = ("org.freedesktop.DBus", "/org/freedesktop/DBus");

/// `f` on a thread of its own, its answer if it has one `within`.
fn within<T: Send + 'static>(within: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || _ = tx.send(f()));
    rx.recv_timeout(within).ok()
}

fn become_monitor(conn: &Connection, rules: &[&str]) -> zbus::Result<zbus::Message> {
    conn.call_method(Some(DBUS.0), DBUS.1, Some("org.freedesktop.DBus.Monitoring"), "BecomeMonitor", &(rules, 0u32))
}

fn id(conn: &Connection) -> zbus::Result<zbus::Message> {
    conn.call_method(Some(DBUS.0), DBUS.1, Some("org.freedesktop.DBus"), "GetId", &())
}

#[test]
fn becoming_a_monitor_is_answered() {
    let (_bus, address) = bus("monitor", Duration::from_secs(20)).expect("the bus never said it was ready");
    let conn = Builder::address(address.as_str()).unwrap().build().unwrap();
    let c = conn.clone();
    let answer = within(Duration::from_secs(5), move || become_monitor(&c, &[]).map(|_| ()));
    assert!(matches!(answer, Some(Ok(()))), "BecomeMonitor: {answer:?}");
    // And it sees what passes: another program's call to the bus.
    let seen = zbus::blocking::MessageIterator::from(&conn);
    let other = Builder::address(address.as_str()).unwrap().build().unwrap();
    id(&other).unwrap();
    let saw = within(Duration::from_secs(5), move || seen.flatten().any(|m| m.header().member().is_some_and(|n| n == "GetId")));
    assert_eq!(saw, Some(true), "the monitor never saw another program's call");
}

#[test]
fn the_bus_answers_on_while_a_monitor_with_rules_watches() {
    let (_bus, address) = bus("watched", Duration::from_secs(20)).expect("the bus never said it was ready");
    let monitor = Builder::address(address.as_str()).unwrap().build().unwrap();
    // (Its answer is the other test's: here only that it has become one.)
    let m = monitor.clone();
    let _ = within(Duration::from_secs(2), move || become_monitor(&m, &["type='signal',interface='org.example.Nothing'"]).is_ok());
    // Each program that comes makes a signal to nobody in particular
    // (`NameOwnerChanged`), which the monitor is asked about.
    for n in 0..3 {
        let a = address.clone();
        let answered = within(Duration::from_secs(5), move || Builder::address(a.as_str()).unwrap().build().is_ok_and(|c| id(&c).is_ok()));
        assert_eq!(answered, Some(true), "program {n} wasn't answered with a monitor watching");
    }
    drop(monitor);
}
