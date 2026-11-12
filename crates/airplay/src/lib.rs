//! Sending to AirPlay receivers (an Apple TV, a Mac, a television with
//! AirPlay built in): Ben, 2026-11-12, "add airplay support to olo so
//! that i can broadcast/stream either a specific window, a specific
//! workspace, an agent screen or a video file to an airplay compatible
//! display/receiver". docs/TODO.md has the plan and what is and isn't
//! open to a sender that isn't Apple's.
//!
//! So far: finding the receivers, pairing with one, and playing a film
//! on one. They announce themselves by mDNS (`_airplay._tcp`), which
//! this asks itself ([`dns`], [`browse`]: no Avahi, no tool), and each
//! says in its announcement what it is and what it takes ([`Receiver`]).
//! Every one of today's wants to be paired with before it takes anything
//! ([`pairing`]: HomeKit's, with the code it shows). A film is played by
//! its address ([`play`]): the receiver fetches it, from this computer
//! or from the web, and plays the file's own picture and sound.
//!
//! Nothing here is ever tried against a real receiver by a test, nor
//! against the network a test happens to be on: a test answers its own
//! questions (a responder of its own on the loopback, `Via`).

pub mod browse;
pub mod dns;
pub mod kept;
pub mod pairing;
pub mod play;
mod receiver;

pub use browse::{Service, Via, browse};
pub use receiver::{Kind, Receiver};

/// AirPlay's service, as receivers announce it.
pub const SERVICE: &str = "_airplay._tcp";

/// A test's own receivers, in place of the network's:
/// "Front Room=127.0.0.1:7000;Den=127.0.0.1:7001". With it set the
/// network is never asked (a window looked at headless finds a receiver
/// of the test's own, and nobody else's).
pub const RECEIVERS_ENV: &str = "OLO_AIRPLAY_RECEIVERS";

/// The receivers that answer within `wait` on the networks this
/// computer is on, by name. Blocks for `wait`: off a looper.
pub fn receivers(wait: std::time::Duration) -> std::io::Result<Vec<Receiver>> {
    let mut found: Vec<Receiver> = match std::env::var(RECEIVERS_ENV) {
        Ok(given) => of_a_test(&given),
        Err(_) => browse(SERVICE, wait, &Via::lan())?.iter().map(Receiver::of).collect(),
    };
    found.sort_by_key(|a| a.name.to_lowercase());
    Ok(found)
}

fn of_a_test(given: &str) -> Vec<Receiver> {
    use std::net::ToSocketAddrs as _;
    given.split(';').filter_map(|one| one.split_once('=')).filter_map(|(name, at)| Some(Receiver::of_a_test(name.trim(), at.trim().to_socket_addrs().ok()?.next()?))).collect()
}

/// What a receiver lists this computer as.
pub fn our_name() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned()).unwrap_or_default();
    if host.is_empty() { "Olo".into() } else { format!("Olo on {host}") }
}

/// A pairing begun: the receiver is showing its code, and waits for it
/// to be typed here ([`Pairing::finish`]). Dropped, the receiver takes
/// its code down.
pub struct Pairing {
    http: pairing::Http,
    setup: pairing::Setup,
    device_id: String,
    name: String,
}

impl Pairing {
    /// Ask the receiver at `at` to show its code. Blocks while it
    /// answers (`wait` at most for each step): off a looper.
    pub fn begin(at: std::net::SocketAddr, device_id: &str, name: &str, wait: std::time::Duration) -> Result<Pairing, pairing::Error> {
        let mut http = pairing::Http::connect(at, wait)?;
        let setup = pairing::Setup::begin(&mut http)?;
        Ok(Pairing { http, setup, device_id: device_id.to_owned(), name: name.to_owned() })
    }

    /// The code it shows, typed: paired, and the pairing kept. Whether
    /// the receiver signed for its key as HomeKit says (one that didn't
    /// is paired with all the same). Blocks: off a looper.
    pub fn finish(mut self, code: &str) -> Result<bool, String> {
        let paired = self.setup.finish(&mut self.http, code, &our_name()).map_err(|e| e.to_string())?;
        // (One that announced no id: the one it gave in pairing.)
        let device_id = if self.device_id.is_empty() { String::from_utf8_lossy(&paired.credentials.receiver_id).into_owned() } else { self.device_id };
        kept::keep(kept::Kept { device_id, name: self.name, credentials: paired.credentials.to_line() }).map_err(|e| format!("paired, but the pairing couldn't be kept: {e}"))?;
        Ok(paired.signed)
    }
}

/// Whether the receiver at `at`, paired with (`kept`), still knows this
/// computer and is the one that was paired with. Blocks: off a looper.
pub fn check(at: std::net::SocketAddr, kept: &kept::Kept, wait: std::time::Duration) -> Result<(), String> {
    let credentials = kept.credentials().ok_or("its pairing here can't be read: pair with it again")?;
    let mut http = pairing::Http::connect(at, wait).map_err(|e| e.to_string())?;
    pairing::verify(&mut http, &credentials).map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tests_own_receivers() {
        let found = of_a_test("Front Room=127.0.0.1:7000; Den = 127.0.0.1:7001;nothing;Gone=nowhere");
        assert_eq!(found.iter().map(|r| (r.name.as_str(), r.port)).collect::<Vec<_>>(), [("Front Room", 7000), ("Den", 7001)]);
        assert!(found[0].device_id != found[1].device_id && found[0].device_id.len() == 17);
        assert!(found[0].plays_video && found[0].asks_to_pair);
        assert_eq!(found[0].at(), Some("127.0.0.1:7000".parse().unwrap()));
        // Its IPv4 address is where it's reached, when it has both.
        let mut both = found[0].clone();
        both.addrs = vec!["fe80::1".parse().unwrap(), "10.0.0.7".parse().unwrap()];
        assert_eq!(both.at(), Some("10.0.0.7:7000".parse().unwrap()));
    }
}
