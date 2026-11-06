//! The RemoteDesktop portal as a program uses it, against olo-portal on a
//! test's own bus: a session for the keyboard and pointer, started (the
//! person answers the dialog), "hi" typed with the Notify calls and the
//! pointer moved 100 points right, then a libei connection
//! (ConnectToEIS) that types "x" and moves the pointer 50 points down.
//!
//! `cargo run -p olo-portal --example remote_test -- unix:path=BUS [HOLD-SECS]`

use std::{collections::HashMap, os::unix::net::UnixStream, time::Duration};

use reis::{PendingRequestResult, ei};
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.impl.portal.desktop.olo";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.RemoteDesktop";

type Results = (u32, HashMap<String, OwnedValue>);

fn opts(pairs: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
    pairs.iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap())).collect()
}

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p.to_owned()).unwrap()
}

fn main() {
    let bus = std::env::args().nth(1).expect("the bus's address");
    let conn = zbus::blocking::connection::Builder::address(bus.as_str()).unwrap().build().unwrap();
    let portal = zbus::blocking::Proxy::new(&conn, DEST, PATH, IFACE).unwrap();
    let session = path("/org/freedesktop/portal/desktop/session/test/remote");
    let app = "olo.remote-test";
    let (r, _): Results = portal.call("CreateSession", &(path("/r/1"), &session, app, opts(&[]))).unwrap();
    assert_eq!(r, 0, "CreateSession");
    let (r, _): Results = portal.call("SelectDevices", &(path("/r/2"), &session, app, opts(&[("types", Value::U32(3))]))).unwrap();
    assert_eq!(r, 0, "SelectDevices");
    // OLO_REMOTE_TEST_SCREEN=1: a screen to see too (the picker asks).
    if std::env::var_os("OLO_REMOTE_TEST_SCREEN").is_some() {
        let cast = zbus::blocking::Proxy::new(&conn, DEST, PATH, "org.freedesktop.impl.portal.ScreenCast").unwrap();
        let (r, _): Results = cast.call("SelectSources", &(path("/r/4"), &session, app, opts(&[("types", Value::U32(1))]))).unwrap();
        assert_eq!(r, 0, "SelectSources");
    }
    println!("starting: answer the dialog");
    let (r, results): Results = portal.call("Start", &(path("/r/3"), &session, app, "", opts(&[]))).unwrap();
    println!("Start: {r}, {results:?}");
    if r != 0 {
        return;
    }
    // Notify calls: "hi", and the pointer 100 right.
    for keysym in [0x68u32, 0x69] {
        let _: () = portal.call("NotifyKeyboardKeysym", &(&session, opts(&[]), keysym as i32, 1u32)).unwrap();
        let _: () = portal.call("NotifyKeyboardKeysym", &(&session, opts(&[]), keysym as i32, 0u32)).unwrap();
    }
    let _: () = portal.call("NotifyPointerMotion", &(&session, opts(&[]), 100.0f64, 0.0f64)).unwrap();
    println!("typed \"hi\" and moved the pointer by the Notify calls");
    std::thread::sleep(Duration::from_millis(300));

    let fd: OwnedFd = portal.call("ConnectToEIS", &(&session, app, opts(&[]))).unwrap();
    // The Notify calls are refused once the session has its connection
    // (they do nothing: the portal ignores them).
    let _: () = portal.call("NotifyKeyboardKeysym", &(&session, opts(&[]), 0x71i32, 1u32)).unwrap();
    let _: () = portal.call("NotifyKeyboardKeysym", &(&session, opts(&[]), 0x71i32, 0u32)).unwrap();
    let stream = UnixStream::from(std::os::fd::OwnedFd::from(fd));
    let context = ei::Context::new(stream).unwrap();
    let mut serial = 0;
    let mut sequence = 0;
    let mut sent = (false, false);
    let start = std::time::Instant::now();
    let mut devices: HashMap<ei::Device, HashMap<String, reis::Object>> = HashMap::new();
    while start.elapsed() < Duration::from_secs(5) && !(sent.0 && sent.1) {
        let mut fds = [libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&context), events: libc::POLLIN, revents: 0 }];
        // SAFETY: one pollfd of ours.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, 200) };
        if fds[0].revents != 0 && matches!(context.read(), Ok(0) | Err(_)) {
            println!("the connection closed");
            break;
        }
        while let Some(result) = context.pending_event() {
            let PendingRequestResult::Request(event) = result else { continue };
            match event {
                ei::Event::Handshake(handshake, ei::handshake::Event::HandshakeVersion { .. }) => {
                    handshake.handshake_version(1);
                    handshake.name("olo-remote-test");
                    handshake.context_type(ei::handshake::ContextType::Sender);
                    for (interface, version) in
                        [("ei_callback", 1), ("ei_connection", 1), ("ei_seat", 1), ("ei_device", 2), ("ei_pingpong", 1), ("ei_pointer", 1), ("ei_keyboard", 1), ("ei_button", 1)]
                    {
                        handshake.interface_version(interface, version);
                    }
                    handshake.finish();
                }
                ei::Event::Handshake(_, ei::handshake::Event::Connection { serial: s, .. }) => serial = s,
                ei::Event::Connection(_, ei::connection::Event::Ping { ping }) => ping.done(0),
                ei::Event::Seat(seat, ei::seat::Event::Capability { mask, interface }) => {
                    println!("seat offers {interface}");
                    if interface == "ei_pointer" || interface == "ei_keyboard" {
                        seat.bind(mask);
                    }
                }
                ei::Event::Seat(_, ei::seat::Event::Device { device }) => {
                    devices.insert(device, HashMap::new());
                }
                ei::Event::Device(device, ei::device::Event::Interface { object }) => {
                    devices.entry(device).or_default().insert(object.interface().to_owned(), object);
                }
                ei::Event::Device(device, ei::device::Event::Resumed { serial: s }) => {
                    serial = s;
                    let interfaces = devices.get(&device).cloned().unwrap_or_default();
                    if let Some(pointer) = interfaces.get("ei_pointer").and_then(|o| o.clone().downcast::<ei::Pointer>()) {
                        device.start_emulating(serial, sequence);
                        sequence += 1;
                        pointer.motion_relative(0.0, 50.0);
                        device.frame(serial, 1);
                        device.stop_emulating(serial);
                        sent.0 = true;
                        println!("moved the pointer over libei");
                    }
                    if let Some(keyboard) = interfaces.get("ei_keyboard").and_then(|o| o.clone().downcast::<ei::Keyboard>()) {
                        device.start_emulating(serial, sequence);
                        sequence += 1;
                        // KEY_X.
                        keyboard.key(45, ei::keyboard::KeyState::Press);
                        device.frame(serial, 1);
                        keyboard.key(45, ei::keyboard::KeyState::Released);
                        device.frame(serial, 2);
                        device.stop_emulating(serial);
                        sent.1 = true;
                        println!("typed \"x\" over libei");
                    }
                }
                _ => {}
            }
        }
        let _ = context.flush();
    }
    println!("done: pointer {} keyboard {}", sent.0, sent.1);
    // Held (a second argument: seconds) until the portal's side closes it
    // (Stop in Deskbar), as a program keeps it.
    let hold = std::env::args().nth(2).and_then(|h| h.parse::<u64>().ok()).unwrap_or(0);
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(hold) {
        let mut fds = [libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&context), events: libc::POLLIN, revents: 0 }];
        // SAFETY: one pollfd of ours.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, 200) };
        if fds[0].revents == 0 {
            continue;
        }
        match context.read() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(0) | Err(_) => {
                println!("the connection was closed after {:.1} s", start.elapsed().as_secs_f32());
                return;
            }
            Ok(_) => {
                while let Some(PendingRequestResult::Request(event)) = context.pending_event() {
                    if let ei::Event::Connection(_, ei::connection::Event::Disconnected { reason, explanation, .. }) = event {
                        println!("disconnected: {reason:?} {explanation:?}");
                    }
                }
            }
        }
    }
    std::thread::sleep(Duration::from_millis(300));
}
