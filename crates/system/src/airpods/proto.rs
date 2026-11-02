//! The accessory protocol AirPods speak to the computer they're
//! connected to (AACP), over an L2CAP channel of its own: what we send
//! and what they tell us. Only bytes in and out here; [`super::link`]
//! carries them.
//!
//! A message is `04 00 04 00`, a 16-bit opcode (little-endian), then the
//! opcode's own bytes. Nothing is answered until the hello
//! ([`hello`]) has been sent, and nothing is told until notifications
//! are asked for ([`ask_notifications`]).

/// The L2CAP channel (PSM) AirPods listen on.
pub const PSM: u16 = 0x1001;

const HEADER: [u8; 4] = [0x04, 0x00, 0x04, 0x00];

/// What each message is about.
pub mod opcode {
    /// Levels of the buds and case (them to us).
    pub const BATTERY: u16 = 0x04;
    /// Which buds are in an ear (them to us).
    pub const EAR: u16 = 0x06;
    /// A setting, either way ([`super::Setting`]).
    pub const CONTROL: u16 = 0x09;
    /// What the audio is for, and whose (them to us).
    pub const AUDIO_SOURCE: u16 = 0x0E;
    /// Ask to be told of changes (us to them).
    pub const NOTIFY: u16 = 0x0F;
    /// Messages between the computers and phones connected to them,
    /// passed on (either way).
    pub const SMART_ROUTING: u16 = 0x10;
    pub const SMART_ROUTING_REPLY: u16 = 0x11;
    /// Head tracking (either way).
    pub const HEAD_TRACKING: u16 = 0x17;
    /// A press of a stem (them to us).
    pub const STEM_PRESS: u16 = 0x19;
    /// Their name (us to them).
    pub const RENAME: u16 = 0x1A;
    /// Who they are: name, model, serial, firmware (them to us).
    pub const INFO: u16 = 0x1D;
    /// The devices connected to them (them to us).
    pub const CONNECTED_DEVICES: u16 = 0x2E;
    /// Ask for the keys their broadcasts are made with (us to them).
    pub const KEYS: u16 = 0x30;
    pub const KEYS_REPLY: u16 = 0x31;
    /// The wearer is speaking (Conversation Awareness; them to us).
    pub const CONVERSATION: u16 = 0x4B;
    /// What this host can do (us to them).
    pub const HOST_FEATURES: u16 = 0x4D;
}

/// Settings, sent and told under [`opcode::CONTROL`]: an identifier
/// and up to four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Setting {
    /// Which bud's microphone: 0 either, 1 right, 2 left.
    Microphone = 0x01,
    /// This host owns the connection (it is the one playing): 1 or 0.
    OwnsConnection = 0x06,
    /// The buds tell when they go in and out of an ear: 1 on, 2 off.
    EarDetection = 0x0A,
    /// [`ListeningMode`].
    ListeningMode = 0x0D,
    /// Press and hold on the left and right stems (1 noise control, 5 Siri).
    PressAndHold = 0x16,
    /// How quickly a double press must come: 0 default, 1 slower, 2 slowest.
    PressSpeed = 0x17,
    /// How long press and hold takes: 0 default, 1 shorter, 2 shortest.
    HoldDuration = 0x18,
    /// The modes press and hold goes through ([`ListeningMode::bit`]).
    ListeningModes = 0x1A,
    /// Noise Cancellation with one bud in: 1 on, 2 off.
    OneBudAnc = 0x1B,
    /// Personalized Volume: 1 on, 2 off.
    AdaptiveVolume = 0x26,
    /// Conversation Awareness: 1 on, 2 off.
    Conversation = 0x28,
    /// How much Adaptive lets in: 0 (more) to 100 (less).
    AdaptiveStrength = 0x2E,
    /// The case's sounds: 0 on, 1 off.
    CaseSounds = 0x31,
    /// Off may be chosen as a listening mode: 1 yes, 2 no.
    AllowOff = 0x34,
    /// Volume swipe on the stems: 1 on, 2 off.
    VolumeSwipe = 0x25,
}

impl Setting {
    pub fn from_byte(b: u8) -> Option<Setting> {
        use Setting::*;
        [Microphone, OwnsConnection, EarDetection, ListeningMode, PressAndHold, PressSpeed, HoldDuration, ListeningModes, OneBudAnc, AdaptiveVolume, Conversation, AdaptiveStrength, CaseSounds, AllowOff, VolumeSwipe]
            .into_iter()
            .find(|s| *s as u8 == b)
    }
}

/// What the buds do with the sound around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListeningMode {
    Off,
    NoiseCancellation,
    Transparency,
    Adaptive,
}

impl ListeningMode {
    pub const ALL: [ListeningMode; 4] = [ListeningMode::Off, ListeningMode::Transparency, ListeningMode::Adaptive, ListeningMode::NoiseCancellation];

    pub fn byte(self) -> u8 {
        match self {
            ListeningMode::Off => 1,
            ListeningMode::NoiseCancellation => 2,
            ListeningMode::Transparency => 3,
            ListeningMode::Adaptive => 4,
        }
    }

    pub fn from_byte(b: u8) -> Option<ListeningMode> {
        Some(match b {
            1 => ListeningMode::Off,
            2 => ListeningMode::NoiseCancellation,
            3 => ListeningMode::Transparency,
            4 => ListeningMode::Adaptive,
            _ => return None,
        })
    }

    /// Its bit in [`Setting::ListeningModes`].
    pub fn bit(self) -> u8 {
        match self {
            ListeningMode::Off => 0x01,
            ListeningMode::NoiseCancellation => 0x02,
            ListeningMode::Transparency => 0x04,
            ListeningMode::Adaptive => 0x08,
        }
    }
}

/// Which part of the AirPods a level is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// Headphones in one piece (AirPods Max).
    Whole,
    Left,
    Right,
    Case,
}

/// One part's battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub part: Part,
    /// 0–100.
    pub percent: u8,
    pub charging: bool,
}

/// Where a bud is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    InEar,
    OutOfEar,
    InCase,
    /// Not connected (out of range, or its battery ran out).
    Gone,
}

impl Placement {
    fn from_byte(b: u8) -> Placement {
        match b {
            0 => Placement::InEar,
            2 => Placement::InCase,
            3 => Placement::Gone,
            _ => Placement::OutOfEar,
        }
    }
}

/// Who the AirPods say they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    /// Apple's model number ("A3048").
    pub model: String,
    pub manufacturer: String,
    pub serial: String,
    pub firmware: String,
}

/// The keys the AirPods' broadcasts are made with: the identity key
/// (IRK) resolves the private addresses they broadcast from, the
/// encryption key opens the levels in them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys {
    pub irk: Option<[u8; 16]>,
    pub encryption: Option<[u8; 16]>,
}

/// What kind of stem press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Single,
    Double,
    Triple,
    Long,
}

/// A message from the AirPods.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Battery(Vec<Level>),
    /// The bud whose microphone is used first, then the other.
    Ear { primary: Placement, secondary: Placement },
    Setting { setting: Option<Setting>, id: u8, value: [u8; 4] },
    /// Conversation Awareness: the wearer started speaking (low levels:
    /// turn the sound down) or stopped (8 and up: back to normal).
    Conversation { level: u8 },
    Info(Info),
    Keys(Keys),
    StemPress { press: Press, left: bool },
    /// Anything else, kept whole for whoever wants it.
    Other { opcode: u16, data: Vec<u8> },
}

/// A message with this opcode and body.
pub fn message(opcode: u16, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(6 + body.len());
    m.extend_from_slice(&HEADER);
    m.extend_from_slice(&opcode.to_le_bytes());
    m.extend_from_slice(body);
    m
}

/// The first thing sent: nothing is answered before it.
pub fn hello() -> Vec<u8> {
    vec![0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
}

/// What this host can do: all of it, which turns on Conversation
/// Awareness while sound plays and Adaptive.
pub fn host_features() -> Vec<u8> {
    message(opcode::HOST_FEATURES, &[0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
}

/// Ask to be told of every change.
pub fn ask_notifications() -> Vec<u8> {
    message(opcode::NOTIFY, &[0xFF, 0xFF, 0xFF, 0xFF])
}

/// Ask for the broadcast keys (the identity key and the encryption key).
pub fn ask_keys() -> Vec<u8> {
    message(opcode::KEYS, &[0x01 | 0x04, 0x00])
}

/// Change a setting.
pub fn set(setting: Setting, value: &[u8]) -> Vec<u8> {
    let mut body = [0u8; 5];
    body[0] = setting as u8;
    for (to, from) in body[1..].iter_mut().zip(value) {
        *to = *from;
    }
    message(opcode::CONTROL, &body)
}

/// Change a yes-or-no setting that says yes with 1 and no with 2.
pub fn set_on(setting: Setting, on: bool) -> Vec<u8> {
    set(setting, &[if on { 1 } else { 2 }])
}

pub fn set_listening_mode(mode: ListeningMode) -> Vec<u8> {
    set(Setting::ListeningMode, &[mode.byte()])
}

/// Rename them (what every device they connect to sees). At most 32 bytes
/// of UTF-8, cut at a character's edge.
pub fn rename(name: &str) -> Vec<u8> {
    let mut end = name.len().min(32);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name.as_bytes()[..end];
    let mut body = vec![0x01, name.len() as u8, 0x00];
    body.extend_from_slice(name);
    message(opcode::RENAME, &body)
}

/// Read a message from the AirPods; None for what isn't one.
pub fn parse(packet: &[u8]) -> Option<Message> {
    if packet.len() < 6 || packet[..4] != HEADER {
        return None;
    }
    let opcode = u16::from_le_bytes([packet[4], packet[5]]);
    let data = &packet[6..];
    let other = || Some(Message::Other { opcode, data: data.to_vec() });
    match opcode {
        opcode::BATTERY => {
            let count = *data.first()? as usize;
            let levels = data[1..]
                .as_chunks::<5>()
                .0
                .iter()
                .take(count)
                .filter_map(|c| {
                    let part = match c[0] {
                        0x01 => Part::Whole,
                        0x02 => Part::Right,
                        0x04 => Part::Left,
                        0x08 => Part::Case,
                        _ => return None,
                    };
                    // Status 4: that part isn't there (a bud out of range,
                    // the case's level once it's closed and away).
                    (c[3] != 0x04).then_some(Level { part, percent: c[2].min(100), charging: c[3] == 0x01 })
                })
                .collect();
            Some(Message::Battery(levels))
        }
        opcode::EAR if data.len() >= 2 => Some(Message::Ear { primary: Placement::from_byte(data[0]), secondary: Placement::from_byte(data[1]) }),
        opcode::CONTROL if !data.is_empty() => {
            let mut value = [0u8; 4];
            for (to, from) in value.iter_mut().zip(&data[1..]) {
                *to = *from;
            }
            Some(Message::Setting { setting: Setting::from_byte(data[0]), id: data[0], value })
        }
        // 02 00 01 LEVEL
        opcode::CONVERSATION if data.len() >= 4 => Some(Message::Conversation { level: data[3] }),
        opcode::INFO => parse_info(data).map(Message::Info).or_else(other),
        opcode::KEYS_REPLY => parse_keys(data).map(Message::Keys).or_else(other),
        opcode::STEM_PRESS if data.len() >= 2 => {
            let press = match data[0] {
                0x05 => Press::Single,
                0x06 => Press::Double,
                0x07 => Press::Triple,
                0x08 => Press::Long,
                _ => return other(),
            };
            Some(Message::StemPress { press, left: data[1] == 0x01 })
        }
        _ => other(),
    }
}

/// Two bytes we don't know, then strings each ended by a zero: name,
/// model number, manufacturer, serial, firmware, …
fn parse_info(data: &[u8]) -> Option<Info> {
    let start = data.iter().position(|b| b.is_ascii_graphic())?;
    let mut fields = data[start..].split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned());
    Some(Info {
        name: fields.next()?,
        model: fields.next().unwrap_or_default(),
        manufacturer: fields.next().unwrap_or_default(),
        serial: fields.next().unwrap_or_default(),
        firmware: fields.next().unwrap_or_default(),
    })
}

/// A count, then for each key its kind (1 identity, 4 encryption), a
/// byte, its length, a byte and the key.
fn parse_keys(data: &[u8]) -> Option<Keys> {
    let count = *data.first()? as usize;
    let mut keys = Keys::default();
    let mut at = 1;
    for _ in 0..count {
        let (kind, len) = (*data.get(at)?, *data.get(at + 2)? as usize);
        let key: [u8; 16] = data.get(at + 4..at + 4 + len)?.try_into().ok()?;
        match kind {
            0x01 => keys.irk = Some(key),
            0x04 => keys.encryption = Some(key),
            _ => {}
        }
        at += 4 + len;
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn writes_messages() {
        assert_eq!(ask_notifications(), hex("04 00 04 00 0F 00 FF FF FF FF"));
        assert_eq!(set_listening_mode(ListeningMode::Adaptive), hex("04 00 04 00 09 00 0D 04 00 00 00"));
        assert_eq!(set_on(Setting::Conversation, false), hex("04 00 04 00 09 00 28 02 00 00 00"));
        assert_eq!(set(Setting::PressAndHold, &[1, 5]), hex("04 00 04 00 09 00 16 01 05 00 00"));
        assert_eq!(rename("Ben"), hex("04 00 04 00 1A 00 01 03 00 42 65 6E"));
        assert_eq!(ask_keys(), hex("04 00 04 00 30 00 05 00"));
        // Cut at a character, not in one.
        assert_eq!(rename(&"é".repeat(20)).len(), 6 + 3 + 32);
    }

    #[test]
    fn reads_levels() {
        let m = parse(&hex("04 00 04 00 04 00 03 02 01 64 02 01 04 01 63 01 01 08 01 11 02 01")).unwrap();
        assert_eq!(
            m,
            Message::Battery(vec![
                Level { part: Part::Right, percent: 100, charging: false },
                Level { part: Part::Left, percent: 99, charging: true },
                Level { part: Part::Case, percent: 17, charging: false },
            ])
        );
        // The case away: only the buds.
        let m = parse(&hex("04 00 04 00 04 00 03 02 01 50 02 01 04 01 4B 02 01 08 01 00 04 01")).unwrap();
        assert_eq!(m, Message::Battery(vec![Level { part: Part::Right, percent: 80, charging: false }, Level { part: Part::Left, percent: 75, charging: false }]));
    }

    #[test]
    fn reads_the_rest() {
        assert_eq!(parse(&hex("04 00 04 00 06 00 00 02")), Some(Message::Ear { primary: Placement::InEar, secondary: Placement::InCase }));
        assert_eq!(
            parse(&hex("04 00 04 00 09 00 0D 02 00 00 00")),
            Some(Message::Setting { setting: Some(Setting::ListeningMode), id: 0x0D, value: [2, 0, 0, 0] })
        );
        assert_eq!(parse(&hex("04 00 04 00 4B 00 02 00 01 02")), Some(Message::Conversation { level: 2 }));
        assert_eq!(parse(&hex("04 00 04 00 19 00 06 01")), Some(Message::StemPress { press: Press::Double, left: true }));
        assert_eq!(parse(&hex("04 00 04 00 77 00 01 02")), Some(Message::Other { opcode: 0x77, data: vec![1, 2] }));
        assert_eq!(parse(&hex("00 00 04 00")), None);
    }

    #[test]
    fn reads_who_they_are() {
        // As AirPods Pro (2nd generation, USB-C) say it.
        let mut p = hex("04 00 04 00 1D 00 02 D5 00 04 00");
        for s in ["AirPods Pro", "A3048", "Apple Inc.", "QXNRHHYXP6", "61.1868040002000000.2713"] {
            p.extend_from_slice(s.as_bytes());
            p.push(0);
        }
        let Some(Message::Info(info)) = parse(&p) else { panic!() };
        assert_eq!((info.name.as_str(), info.model.as_str(), info.serial.as_str()), ("AirPods Pro", "A3048", "QXNRHHYXP6"));
    }

    #[test]
    fn reads_keys() {
        let mut p = hex("04 00 04 00 31 00 02 01 00 10 00");
        p.extend([0x11; 16]);
        p.extend(hex("04 00 10 00"));
        p.extend([0x44; 16]);
        assert_eq!(parse(&p), Some(Message::Keys(Keys { irk: Some([0x11; 16]), encryption: Some([0x44; 16]) })));
    }
}
