//! Mice: which one is connected (its maker and model, from its USB or
//! Bluetooth ids and the name its driver gives it), the settings that
//! suit it (a profile), and what is known about how it behaves (its
//! quirks), for Preferences ▸ Touchpad & Mouse and app_server.
//!
//! Each mouse can follow the other mice's settings (Settings::mouse_speed
//! and touchpad.mouse_accel_profile), its model's recommended profile,
//! or settings of its own (`Settings::mice`, by `key`).
//!
//! Only what is known for certain goes in the table: ids seen on real
//! devices, and names as their drivers give them (a receiver's mouse is
//! named by the mouse, not the receiver).

use serde::{Deserialize, Serialize};

/// A mouse's settings, as `Settings::mice` keeps them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseTuning {
    /// "recommended" (its model's profile) or "custom" (these values).
    pub profile: String,
    /// libinput's pointer speed, -1 to 1.
    pub speed: f64,
    /// "adaptive" or "flat".
    pub accel: String,
}

impl Default for MouseTuning {
    fn default() -> Self {
        MouseTuning { profile: "recommended".into(), speed: 0.0, accel: "adaptive".into() }
    }
}

/// The key a mouse's settings are kept under: its vendor and product ids
/// ("046d:40b1").
pub fn key(vendor: u32, product: u32) -> String {
    format!("{vendor:04x}:{product:04x}")
}

/// What a mouse is for, which decides the settings that suit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// A gaming mouse: a high-resolution sensor, a high report rate.
    Gaming,
    /// An everyday mouse (office, travel, Apple's).
    Everyday,
}

impl Class {
    /// The profile that suits it.
    pub fn tuning(self) -> MouseTuning {
        match self {
            // The pointer goes exactly as far as the hand: what such a
            // sensor is made for, and how its owners set it up.
            Class::Gaming => MouseTuning { profile: "recommended".into(), speed: 0.0, accel: "flat".into() },
            // Faster for a flick, finer for a slow move, as on a Mac.
            Class::Everyday => MouseTuning { profile: "recommended".into(), speed: 0.0, accel: "adaptive".into() },
        }
    }

    /// The profile, in a few words.
    pub fn short(self) -> &'static str {
        match self {
            Class::Gaming => "no acceleration",
            Class::Everyday => "adaptive acceleration",
        }
    }

    /// The profile, in words.
    pub fn describe(self) -> &'static str {
        match self {
            Class::Gaming => "No acceleration: the pointer goes exactly as far as the hand, which is what a gaming sensor is made for",
            Class::Everyday => "Adaptive acceleration: faster for a flick, finer for a slow move, as on a Mac",
        }
    }
}

/// Something a mouse does that matters to how it is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Quirk {
    /// The node that reports its movement also has keys (for its
    /// programmable buttons), so a key remapper grabs it as a keyboard.
    KeysOnMotionNode,
    /// It talks through a USB receiver, which USB power management may
    /// put to sleep.
    Receiver,
    /// It reports this many times a second, more often than a display
    /// draws.
    Reports { hz: u32 },
    /// Its wheel can spin freely.
    FreeSpinWheel,
    /// A second wheel under the thumb scrolls sideways.
    ThumbWheel,
    /// No wheel: its top is a touch surface that scrolls.
    TouchSurface,
    /// No middle button.
    NoMiddleButton,
}

impl Quirk {
    /// What it means, and what Olo does about it.
    pub fn describe(self) -> String {
        match self {
            Quirk::KeysOnMotionNode => {
                "Its movement and its programmable buttons share one input node, so a key remapper such as keyd grabs it as a keyboard, and settings made here no longer reach it.".into()
            }
            Quirk::Receiver => "It talks through a USB receiver, which loses the first movement after a pause if it is let sleep. Olo's input rule keeps receivers awake.".into(),
            Quirk::Reports { hz } => {
                format!("It reports up to {} times a second, faster than a display draws: Olo resamples the pointer onto each display's frame clock, so it moves evenly.", thousands(hz))
            }
            Quirk::FreeSpinWheel => "Its wheel can spin freely: a flick scrolls a long way; the button below the wheel switches it to notched steps.".into(),
            Quirk::ThumbWheel => "The wheel under the thumb scrolls sideways.".into(),
            Quirk::TouchSurface => "It has no wheel: its top is a touch surface that scrolls.".into(),
            Quirk::NoMiddleButton => "It has no middle button.".into(),
        }
    }
}

/// "25,600".
pub fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A model of mouse.
#[derive(Debug, PartialEq)]
pub struct Model {
    pub maker: &'static str,
    /// Its name, without the maker's ("G309 LIGHTSPEED").
    pub name: &'static str,
    pub vendor: u32,
    /// Its product ids, wired, through its receiver or over Bluetooth.
    pub products: &'static [u32],
    /// What its driver's name for it contains (any one of these).
    pub names: &'static [&'static str],
    pub class: Class,
    pub sensor: Option<&'static str>,
    pub max_dpi: Option<u32>,
    pub quirks: &'static [Quirk],
}

impl Model {
    /// "Logitech G309 LIGHTSPEED".
    pub fn full_name(&self) -> String {
        format!("{} {}", self.maker, self.name)
    }

    /// The settings that suit it.
    pub fn tuning(&self) -> MouseTuning {
        self.class.tuning()
    }
}

const LOGITECH: u32 = 0x046d;
const RAZER: u32 = 0x1532;
const APPLE: u32 = 0x05ac;

/// The mice Olo knows. A model is found by its ids first, then by its
/// driver's name for it, with the maker's vendor id.
pub const MODELS: &[Model] = &[
    Model {
        maker: "Logitech",
        name: "G309 LIGHTSPEED",
        vendor: LOGITECH,
        products: &[0x40b1],
        names: &["G309"],
        class: Class::Gaming,
        sensor: Some("HERO 25K"),
        max_dpi: Some(25_600),
        quirks: &[Quirk::KeysOnMotionNode, Quirk::Receiver, Quirk::Reports { hz: 1000 }],
    },
    Model {
        maker: "Razer",
        name: "Viper V2 Pro",
        vendor: RAZER,
        products: &[0x00a5, 0x00a6],
        names: &["Viper V2 Pro"],
        class: Class::Gaming,
        sensor: Some("Focus Pro 30K"),
        max_dpi: Some(30_000),
        quirks: &[Quirk::Receiver, Quirk::Reports { hz: 1000 }],
    },
    Model {
        maker: "Logitech",
        name: "MX Master 3S",
        vendor: LOGITECH,
        products: &[],
        names: &["MX Master 3S"],
        class: Class::Everyday,
        sensor: Some("Darkfield"),
        max_dpi: Some(8000),
        quirks: &[Quirk::FreeSpinWheel, Quirk::ThumbWheel],
    },
    Model {
        maker: "Logitech",
        name: "MX Master 3",
        vendor: LOGITECH,
        products: &[],
        names: &["MX Master 3"],
        class: Class::Everyday,
        sensor: Some("Darkfield"),
        max_dpi: Some(4000),
        quirks: &[Quirk::FreeSpinWheel, Quirk::ThumbWheel],
    },
    Model {
        maker: "Logitech",
        name: "MX Master 2S",
        vendor: LOGITECH,
        products: &[],
        names: &["MX Master 2S"],
        class: Class::Everyday,
        sensor: Some("Darkfield"),
        max_dpi: Some(4000),
        quirks: &[Quirk::FreeSpinWheel, Quirk::ThumbWheel],
    },
    Model {
        maker: "Logitech",
        name: "MX Anywhere 3",
        vendor: LOGITECH,
        products: &[],
        names: &["MX Anywhere 3"],
        class: Class::Everyday,
        sensor: Some("Darkfield"),
        max_dpi: None,
        quirks: &[Quirk::FreeSpinWheel],
    },
    Model {
        maker: "Apple",
        name: "Magic Mouse 2",
        vendor: APPLE,
        products: &[0x0269],
        names: &["Magic Mouse 2"],
        class: Class::Everyday,
        sensor: None,
        max_dpi: None,
        quirks: &[Quirk::TouchSurface, Quirk::NoMiddleButton],
    },
    Model {
        maker: "Apple",
        name: "Magic Mouse",
        vendor: APPLE,
        products: &[0x030d],
        names: &["Magic Mouse"],
        class: Class::Everyday,
        sensor: None,
        max_dpi: None,
        quirks: &[Quirk::TouchSurface, Quirk::NoMiddleButton],
    },
];

/// The makers of mice, by USB vendor id.
const MAKERS: &[(u32, &str)] = &[
    (LOGITECH, "Logitech"),
    (RAZER, "Razer"),
    (APPLE, "Apple"),
    (0x1038, "SteelSeries"),
    (0x1b1c, "Corsair"),
    (0x045e, "Microsoft"),
    (0x0b05, "ASUS"),
    (0x413c, "Dell"),
    (0x17ef, "Lenovo"),
    (0x03f0, "HP"),
    (0x0951, "HyperX"),
    (0x1e7d, "ROCCAT"),
    (0x3434, "Keychron"),
    (0x2dc8, "8BitDo"),
    (0x046a, "Cherry"),
    (0x093a, "PixArt"),
    (0x258a, "Sino Wealth"),
    (0x04d9, "Holtek"),
];

/// Which model a mouse is.
pub fn identify(name: &str, vendor: u32, product: u32) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.vendor == vendor && m.products.contains(&product)).or_else(|| {
        // The longest name that fits ("MX Master 3S" before "MX Master 3").
        MODELS
            .iter()
            .filter(|m| m.vendor == vendor || vendor == 0)
            .filter(|m| m.names.iter().any(|n| name.contains(n)))
            .max_by_key(|m| m.names.iter().filter(|n| name.contains(*n)).map(|n| n.len()).max())
    })
}

/// Who made a mouse, by its vendor id.
pub fn maker(vendor: u32) -> Option<&'static str> {
    MAKERS.iter().find(|(v, _)| *v == vendor).map(|(_, n)| *n)
}

/// The speed and acceleration a mouse gets: its own settings, its
/// model's (or its class's) profile, or what the other mice have.
pub fn effective(settings: &crate::Settings, name: &str, vendor: u32, product: u32) -> (f64, String) {
    match settings.mice.get(&key(vendor, product)) {
        Some(t) if t.profile == "custom" => (t.speed, t.accel.clone()),
        Some(_) => {
            let t = identify(name, vendor, product).map_or_else(|| Class::Everyday.tuning(), Model::tuning);
            (t.speed, t.accel)
        }
        None => (settings.mouse_speed, settings.touchpad.mouse_accel_profile.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_found_by_ids_then_names() {
        let g = identify("Logitech G309", LOGITECH, 0x40b1).unwrap();
        assert_eq!(g.full_name(), "Logitech G309 LIGHTSPEED");
        assert!(g.quirks.contains(&Quirk::KeysOnMotionNode));
        assert_eq!(identify("Razer Razer Viper V2 Pro", RAZER, 0x00a6).unwrap().name, "Viper V2 Pro");
        // By name: the longest that fits.
        assert_eq!(identify("Logitech MX Master 3S", LOGITECH, 0xb034).unwrap().name, "MX Master 3S");
        assert_eq!(identify("Logitech MX Master 3", LOGITECH, 0x4082).unwrap().name, "MX Master 3");
        assert_eq!(identify("Apple Inc. Magic Mouse 2", APPLE, 0x0269).unwrap().name, "Magic Mouse 2");
        // Another maker's mouse of the same name isn't it.
        assert!(identify("Acme MX Master 3", 0x1234, 1).is_none());
        assert!(identify("Some Mouse", LOGITECH, 0xc077).is_none());
        assert_eq!(maker(0x1038), Some("SteelSeries"));
    }

    #[test]
    fn each_mouse_gets_its_own_its_profile_or_the_others() {
        let mut s = crate::Settings { mouse_speed: 0.3, ..Default::default() };
        s.touchpad.mouse_accel_profile = "adaptive".into();
        assert_eq!(effective(&s, "Logitech G309", LOGITECH, 0x40b1), (0.3, "adaptive".to_owned()));
        s.mice.insert(key(LOGITECH, 0x40b1), MouseTuning::default());
        assert_eq!(effective(&s, "Logitech G309", LOGITECH, 0x40b1), (0.0, "flat".to_owned()), "a gaming mouse's profile");
        s.mice.insert(key(LOGITECH, 0x40b1), MouseTuning { profile: "custom".into(), speed: -0.2, accel: "adaptive".into() });
        assert_eq!(effective(&s, "Logitech G309", LOGITECH, 0x40b1), (-0.2, "adaptive".to_owned()));
        // An unknown mouse's recommended profile is an everyday one.
        s.mice.insert(key(0x1234, 5), MouseTuning::default());
        assert_eq!(effective(&s, "Acme", 0x1234, 5), (0.0, "adaptive".to_owned()));
    }

    #[test]
    fn numbers_read_well() {
        assert_eq!(thousands(25_600), "25,600");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(800), "800");
        assert_eq!(key(0x46d, 0x40b1), "046d:40b1");
    }
}
