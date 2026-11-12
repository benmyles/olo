//! An AirPlay receiver, as it announces itself: what it is and what it
//! takes, from its service's TXT record (the names and bits are Apple's,
//! as others have written them down: openairplay.github.io/airplay-spec).

use std::net::IpAddr;

use serde::Serialize;

use crate::browse::Service;

/// What kind of thing a receiver is, by the model it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AppleTv,
    Mac,
    /// A HomePod or another speaker: sound alone.
    Speaker,
    /// A television or a box with AirPlay built in, not Apple's.
    Television,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Receiver {
    /// The name people gave it ("Living Room").
    pub name: String,
    pub kind: Kind,
    /// Its model as it says it ("AppleTV14,1", "Mac16,8").
    pub model: String,
    pub host: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    /// Its id (a MAC address's form), the same whatever it's named.
    pub device_id: String,
    /// What it can do, AirPlay's 64 bits of it.
    pub features: u64,
    /// How it is now (AirPlay's status flags).
    pub flags: u32,
    /// The version of AirPlay it runs (`srcvers`).
    pub version: String,
    /// Plays a video it's given the address of (its picture and sound
    /// as they are: the receiver fetches and decodes it itself).
    pub plays_video: bool,
    /// Shows a screen sent to it (mirroring).
    pub shows_screens: bool,
    /// Plays sound.
    pub plays_sound: bool,
    /// Wants to be paired with before it takes anything (a code it
    /// shows, typed here once), or a password.
    pub asks_to_pair: bool,
}

impl Receiver {
    pub fn of(s: &Service) -> Receiver {
        let get = |k: &str| s.txt.get(k).cloned().unwrap_or_default();
        let (features, flags) = (features(&get("features")), hex(&get("flags")) as u32);
        let model = get("model");
        let bit = |n: u32| features >> n & 1 == 1;
        Receiver {
            name: s.name.clone(),
            kind: kind(&model, features),
            host: s.host.clone(),
            port: s.port,
            addrs: s.addrs.clone(),
            device_id: get("deviceid"),
            version: get("srcvers"),
            // Video (0), by HTTP Live Streaming (4), or the newer way (49).
            plays_video: bit(0) || bit(4) || bit(49),
            shows_screens: bit(7),
            plays_sound: bit(9),
            // A PIN (3), a password (7), pairing once (9), or HomeKit's
            // say on who may (10).
            asks_to_pair: flags & (1 << 3 | 1 << 7 | 1 << 9 | 1 << 10) != 0,
            features,
            flags,
            model,
        }
    }
}

fn kind(model: &str, features: u64) -> Kind {
    if model.starts_with("AppleTV") {
        Kind::AppleTv
    } else if model.starts_with("Mac") || model.starts_with("iMac") {
        Kind::Mac
    } else if model.starts_with("AudioAccessory") || features >> 7 & 1 == 0 && features & 0b1_0001 == 0 {
        // (A HomePod, or anything that shows nothing.)
        Kind::Speaker
    } else {
        Kind::Television
    }
}

/// "0x4A7FDFD5,0x3C177FDE": the low half, then the high.
fn features(said: &str) -> u64 {
    let mut halves = said.split(',');
    let low = hex(halves.next().unwrap_or_default());
    let high = halves.next().map_or(0, hex);
    high << 32 | low & 0xffff_ffff
}

fn hex(s: &str) -> u64 {
    let s = s.trim();
    u64::from_str_radix(s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s), 16).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(name: &str, txt: &[(&str, &str)]) -> Receiver {
        Receiver::of(&Service {
            name: name.into(),
            host: "h.local".into(),
            port: 7000,
            addrs: vec!["10.0.0.7".parse().unwrap()],
            txt: txt.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        })
    }

    /// What three kinds of receiver say of themselves (their TXT records
    /// as such devices send them), read.
    #[test]
    fn what_a_receiver_says_of_itself() {
        let tv = said("Front Room", &[("model", "AppleTV14,1"), ("features", "0x4A7FDFD5,0x3C177FDE"), ("flags", "0x244"), ("srcvers", "1005.13.2"), ("deviceid", "AA:BB:CC:DD:EE:FF")]);
        assert_eq!((tv.kind, tv.plays_video, tv.shows_screens, tv.plays_sound, tv.asks_to_pair), (Kind::AppleTv, true, true, true, true));
        assert_eq!((tv.features, tv.flags, tv.version.as_str(), tv.device_id.as_str()), (0x3C177FDE_4A7FDFD5, 0x244, "1005.13.2", "AA:BB:CC:DD:EE:FF"));
        let mac = said("Studio", &[("model", "Mac16,8"), ("features", "0x4A7FCFD5,0x38174FCE"), ("flags", "0x204")]);
        assert_eq!((mac.kind, mac.plays_video, mac.shows_screens, mac.asks_to_pair), (Kind::Mac, true, true, true));
        // A television with AirPlay built in: no first-version video, but
        // HLS and the newer way.
        let set = said("Den", &[("model", "DU80MD_PML"), ("features", "0x7F8AD0,0x38BCF46"), ("flags", "0x244")]);
        assert_eq!((set.kind, set.plays_video, set.shows_screens), (Kind::Television, true, true));
        // A speaker: sound alone, and nothing asked.
        let speaker = said("Kitchen", &[("model", "AudioAccessory5,1"), ("features", "0x4A7FCA00,0x3C356BD0"), ("flags", "0x4")]);
        assert_eq!((speaker.kind, speaker.shows_screens, speaker.plays_sound, speaker.asks_to_pair), (Kind::Speaker, false, true, false));
        // One that says nothing of itself is still one to list.
        let bare = said("Bare", &[]);
        assert_eq!((bare.features, bare.flags, bare.plays_video, bare.asks_to_pair), (0, 0, false, false));
    }
}
