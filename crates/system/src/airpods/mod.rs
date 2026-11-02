//! AirPods, natively: Apple's accessory protocol over its own L2CAP
//! channel ([`proto`], carried by [`link`]) for their levels, where the
//! buds are, noise control and every setting, and what they broadcast
//! over Bluetooth LE ([`adv`]). Their sound is BlueZ's and the sound
//! server's, as any headphones'.
//!
//! Written from the protocol as it has been documented by people
//! taking it apart (LibrePods' notes and the apple-wireshark dissector),
//! not from anyone's code.

pub mod adv;
pub mod link;
pub mod proto;

/// Apple's Bluetooth vendor id.
pub const APPLE: u16 = 0x004C;

/// What a model of AirPods can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Apple's Bluetooth product id.
    pub product: u16,
    pub name: &'static str,
    /// Noise Cancellation and Transparency.
    pub noise_control: bool,
    /// Adaptive listening, Conversation Awareness, Personalized Volume.
    pub adaptive: bool,
    /// Two buds and a case (not over-ear headphones).
    pub buds: bool,
}

const fn model(product: u16, name: &'static str, noise_control: bool, adaptive: bool, buds: bool) -> Model {
    Model { product, name, noise_control, adaptive, buds }
}

/// The models we know; others from Apple that speak the protocol are
/// treated as the newest.
pub const MODELS: &[Model] = &[
    model(0x2002, "AirPods", false, false, true),
    model(0x200F, "AirPods (2nd generation)", false, false, true),
    model(0x2013, "AirPods (3rd generation)", false, false, true),
    model(0x2019, "AirPods 4", false, false, true),
    model(0x201B, "AirPods 4 with Active Noise Cancellation", true, true, true),
    model(0x200E, "AirPods Pro", true, false, true),
    model(0x2014, "AirPods Pro (2nd generation)", true, true, true),
    model(0x2024, "AirPods Pro (2nd generation, USB-C)", true, true, true),
    model(0x2027, "AirPods Pro 3", true, true, true),
    model(0x202F, "AirPods Pro 3", true, true, true),
    model(0x200A, "AirPods Max", true, false, false),
    model(0x201F, "AirPods Max (USB-C)", true, false, false),
];

impl Model {
    pub fn of(product: u16) -> Model {
        MODELS.iter().copied().find(|m| m.product == product).unwrap_or(Model { product, name: "AirPods", noise_control: true, adaptive: true, buds: true })
    }
}

/// The vendor and product in a BlueZ Modalias ("bluetooth:v004Cp2024d0B5E").
pub fn ids(modalias: &str) -> Option<(u16, u16)> {
    let rest = modalias.strip_prefix("bluetooth:v")?;
    let vendor = u16::from_str_radix(rest.get(..4)?, 16).ok()?;
    let product = u16::from_str_radix(rest.get(4..)?.strip_prefix('p')?.get(..4)?, 16).ok()?;
    Some((vendor, product))
}

/// Whether a device with this Modalias is AirPods (Apple headphones that
/// speak the protocol; Beats speak it too, but not every part of it).
pub fn is_airpods(modalias: &str) -> bool {
    ids(modalias).is_some_and(|(v, p)| v == APPLE && (0x2000..0x2100).contains(&p))
}

#[cfg(test)]
mod tests {
    #[test]
    fn knows_them_by_modalias() {
        assert_eq!(super::ids("bluetooth:v004Cp2024d1D0A"), Some((0x004C, 0x2024)));
        assert!(super::is_airpods("bluetooth:v004Cp2024d1D0A"));
        assert!(!super::is_airpods("bluetooth:v046Dp4082d0001"));
        assert!(!super::is_airpods("usb:v1D6Bp0246d0557"));
        assert_eq!(super::Model::of(0x2024).name, "AirPods Pro (2nd generation, USB-C)");
        assert!(super::Model::of(0x2099).adaptive);
    }
}
