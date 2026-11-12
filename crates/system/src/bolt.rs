//! Thunderbolt's own keeper (boltd, `org.freedesktop.bolt`): a device it
//! is told to remember is let in by itself whenever it's plugged in, where
//! writing the kernel's `authorized` lets it in until it's unplugged or
//! the computer restarts.

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.bolt";

/// Have boltd remember the device with unique id `uid` and let it in now
/// and whenever it comes (its policy `auto`). As root, or as someone
/// polkit lets enroll (`org.freedesktop.bolt.enroll`).
pub fn enroll(uid: &str) -> Result<()> {
    let conn = bus::system()?;
    bus::call::<_, zvariant::OwnedObjectPath>(conn, DEST, "/org/freedesktop/bolt", "org.freedesktop.bolt1.Manager", "EnrollDevice", &(uid, "auto", "none")).map(|_| ())
}
