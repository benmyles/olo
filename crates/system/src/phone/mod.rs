//! The phone: an iPhone's messages and contacts, spoken to in Rust, in
//! process, as Olo speaks to AirPods. The link is BlueZ's (pairing, and
//! the channels it opens for a profile); over it, OBEX ([`obex`]), the
//! Message Access Profile ([`map`]: the messages there are, each as it
//! comes, one sent) and the Phone Book Access Profile ([`pbap`]: names
//! for numbers), and what it's playing ([`music`]: BlueZ's own remote
//! control of the phone's player). Written from the Bluetooth
//! specifications.

pub mod ancs;
pub mod calls;
pub mod link;
pub mod map;
pub mod music;
pub mod obex;
pub mod pbap;
pub mod standin;
