//! The phone's music: what it's playing, and playing, pausing and
//! skipping it from here. A connected phone is a player and this
//! computer its remote control (AVRCP), which BlueZ keeps as a
//! `MediaPlayer1` under the phone's device (`MediaControl1.Player` says
//! which): its `Status`, its `Track` (title, artist, album) and `Play`,
//! `Pause`, `Next`, `Previous`. Read and driven over BlueZ's own D-Bus
//! interface; nothing of the phone's sound is touched.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue};

use crate::Result;

const DEST: &str = "org.bluez";
const CONTROL: &str = "org.bluez.MediaControl1";
pub(crate) const PLAYER: &str = "org.bluez.MediaPlayer1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// What the phone is playing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Music {
    /// The player's object with BlueZ.
    pub player: String,
    /// Playing now (else paused or stopped).
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// What a remote control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Play,
    Pause,
    Next,
    Previous,
}

impl Action {
    fn method(self) -> &'static str {
        match self {
            Action::Play => "Play",
            Action::Pause => "Pause",
            Action::Next => "Next",
            Action::Previous => "Previous",
        }
    }
}

/// A track's words out of BlueZ's `Track` (a missing one empty).
pub fn track(track: &HashMap<String, OwnedValue>) -> (String, String, String) {
    let text = |k: &str| track.get(k).and_then(|v| <&str>::try_from(v).ok()).unwrap_or_default().trim().to_owned();
    (text("Title"), text("Artist"), text("Album"))
}

/// What `phone` (its device with BlueZ) is playing: none if it has no
/// player (not connected, or it offers none), or one with nothing in it
/// (no title: nothing has played since it connected).
pub fn music(conn: &Connection, phone: &str) -> Option<Music> {
    let get = |path: &str, interface: &str, name: &str| conn.call_method(Some(DEST), path, Some(PROPERTIES), "Get", &(interface, name)).ok().and_then(|m| m.body().deserialize::<OwnedValue>().ok());
    let player = OwnedObjectPath::try_from(get(phone, CONTROL, "Player")?).ok()?.as_str().to_owned();
    let status = get(&player, PLAYER, "Status").and_then(|v| String::try_from(v).ok()).unwrap_or_default();
    let (title, artist, album) = get(&player, PLAYER, "Track").and_then(|v| <HashMap<String, OwnedValue>>::try_from(v).ok()).map(|t| track(&t)).unwrap_or_default();
    (!title.is_empty()).then_some(Music { player, playing: status == "playing", title, artist, album })
}

/// Have the phone's `player` do `action`.
pub fn act(conn: &Connection, player: &str, action: Action) -> Result<()> {
    conn.call_method(Some(DEST), player, Some(PLAYER), action.method(), &()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tracks_words() {
        let value = |s: &str| OwnedValue::try_from(zvariant::Value::from(s)).unwrap();
        let mut t = HashMap::new();
        t.insert("Title".to_owned(), value(" So What "));
        t.insert("Artist".to_owned(), value("Miles Davis"));
        t.insert("Duration".to_owned(), OwnedValue::from(545_000u32));
        assert_eq!(track(&t), ("So What".into(), "Miles Davis".into(), String::new()));
        assert_eq!(track(&HashMap::new()), Default::default());
    }
}
