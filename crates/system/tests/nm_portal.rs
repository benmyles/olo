//! Captive portals (`olo_system::network::Portal`) against `nm-fake`, end
//! to end: a network that wants its sign-in page first, seen as
//! NetworkManager's connectivity check says (the link's and the
//! computer's), with the page the network names (DHCP option 114) or the
//! check's address, and gone once signed in. Needs a bus to meet on: run
//! with `OLO_TEST_BUS=unix:path=…` (a private dbus-broker or busd, a
//! test session's), e.g.
//! `OLO_TEST_BUS=unix:path=/var/tmp/olo-dev/NAME/bus cargo test -p olo-system --test nm_portal`.
//! Without it the test does nothing.

use std::{
    process::{Child, Command},
    sync::mpsc,
    time::{Duration, Instant},
};

use olo_system::network::{self, Portal};

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Tells the running fake `what`, as a test would the world.
fn tell(address: &str, what: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_nm-fake")).args(["--address", address, "--tell", what]).output().expect("nm-fake --tell");
    assert!(out.status.success(), "nm-fake --tell {what}: {}", String::from_utf8_lossy(&out.stderr));
}

fn wait_for<T>(what: &str, mut read: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = read() {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_captive_portal_comes_and_goes() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("OLO_NM_BUS", &address) };
    let check = "http://check.olo.test/hotspot.txt";
    let _fake = Fake(Command::new(env!("CARGO_BIN_EXE_nm-fake")).args(["--address", &address, "--wifi", "Airport Wi-Fi", "--check-uri", check]).spawn().expect("nm-fake"));
    let bus = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).unwrap();
    wait_for("nm-fake", || dbus.name_has_owner("org.freedesktop.NetworkManager".try_into().unwrap()).unwrap_or(false).then_some(()));

    // Its signals, as Deskbar follows them.
    let (tx, rx) = mpsc::channel::<Option<Portal>>();
    network::watch_links(move |l| _ = tx.send(l.portal)).expect("watch_links");
    let heard = |want: Option<&str>| {
        let start = Instant::now();
        loop {
            let left = Duration::from_secs(10).saturating_sub(start.elapsed());
            let p = rx.recv_timeout(left).unwrap_or_else(|_| panic!("no word of the portal being {want:?}"));
            if p.as_ref().map(|p| p.url.as_str()) == want {
                return p;
            }
        }
    };
    assert_eq!(heard(None), None, "no portal at first");
    let wifi = wait_for("the link", || network::status().ok().filter(|s| s.devices.iter().any(|d| d.connected())));
    assert_eq!(wifi.portal, None);
    assert!(!network::check_portal().unwrap());

    // Behind a portal that names no page: the check's address.
    tell(&address, "portal");
    let p = heard(Some(check)).expect("a portal");
    assert_eq!((p.network.as_str(), p.named), ("Airport Wi-Fi", false));
    let s = wait_for("the panel's portal", || network::status().ok().and_then(|s| s.portal));
    assert_eq!((s.url.as_str(), s.named, s.device.as_str()), (check, false, wifi.devices[0].path.as_str()));
    assert!(network::check_portal().unwrap());

    // Signed in.
    tell(&address, "portal off");
    assert_eq!(heard(None), None);
    assert_eq!(network::status().unwrap().portal, None);
    assert!(!network::check_portal().unwrap());

    // Behind one that names its page (DHCP option 114, RFC 8910).
    let page = "https://wifi.airport.test/login?ap=12";
    tell(&address, &format!("portal {page}"));
    let p = heard(Some(page)).expect("a portal");
    assert!(p.named);
    // (Not a web page: never taken for the page.)
    tell(&address, "portal off");
    heard(None);
    tell(&address, "portal javascript:alert(1)");
    let p = heard(Some(check)).expect("a portal");
    assert!(!p.named);
}
