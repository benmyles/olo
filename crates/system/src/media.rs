//! Media players on the session bus (MPRIS): which are playing, and
//! pausing and playing them, as headphones' ear detection wants.

use crate::{Result, bus};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// Every player's bus name.
pub fn players() -> Result<Vec<String>> {
    let names: Vec<String> = bus::call(bus::session()?, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", &())?;
    Ok(names.into_iter().filter(|n| n.starts_with(PREFIX)).collect())
}

/// The players playing now.
pub fn playing() -> Result<Vec<String>> {
    let c = bus::session()?;
    Ok(players()?.into_iter().filter(|p| bus::property::<String>(c, p, PATH, PLAYER, "PlaybackStatus").is_ok_and(|s| s == "Playing")).collect())
}

pub fn pause(player: &str) -> Result<()> {
    bus::call::<_, ()>(bus::session()?, player, PATH, PLAYER, "Pause", &())
}

pub fn play(player: &str) -> Result<()> {
    bus::call::<_, ()>(bus::session()?, player, PATH, PLAYER, "Play", &())
}
