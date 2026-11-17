//! The phone: an iPhone's messages and contacts, spoken to in Rust, in
//! process, as Olo speaks to AirPods. The link is BlueZ's (pairing, and
//! the channels it opens for a profile); over it, OBEX ([`obex`]), the
//! Message Access Profile ([`map`]: the messages there are, each as it
//! comes, one sent) and the Phone Book Access Profile ([`pbap`]: names
//! for numbers). Written from the Bluetooth specifications.

pub mod link;
pub mod map;
pub mod obex;
pub mod pbap;
pub mod standin;
