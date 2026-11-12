//! Sending to AirPlay receivers (an Apple TV, a Mac, a television with
//! AirPlay built in): Ben, 2026-11-12, "add airplay support to olo so
//! that i can broadcast/stream either a specific window, a specific
//! workspace, an agent screen or a video file to an airplay compatible
//! display/receiver". docs/TODO.md has the plan and what is and isn't
//! open to a sender that isn't Apple's.
//!
//! So far: finding the receivers. They announce themselves by mDNS
//! (`_airplay._tcp`), which this asks itself ([`dns`], [`browse`]: no
//! Avahi, no tool), and each says in its announcement what it is and
//! what it takes ([`Receiver`]).
//!
//! Nothing here is ever tried against a real receiver by a test, nor
//! against the network a test happens to be on: a test answers its own
//! questions (a responder of its own on the loopback, `Via`).

pub mod browse;
pub mod dns;
mod receiver;

pub use browse::{Service, Via, browse};
pub use receiver::{Kind, Receiver};

/// AirPlay's service, as receivers announce it.
pub const SERVICE: &str = "_airplay._tcp";

/// The receivers that answer within `wait` on the networks this
/// computer is on, by name. Blocks for `wait`: off a looper.
pub fn receivers(wait: std::time::Duration) -> std::io::Result<Vec<Receiver>> {
    let mut found: Vec<Receiver> = browse(SERVICE, wait, &Via::lan())?.iter().map(Receiver::of).collect();
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(found)
}
