//! Keyboards Olo knows, by the name their driver gives them: what each is
//! and has, for Preferences ▸ Keyboard (as `mice` is for mice). Only what
//! is known for certain goes in the table: keyboards seen, named as they
//! came.

/// A keyboard model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// As its maker calls it.
    pub name: &'static str,
    pub maker: &'static str,
    /// Its model number.
    pub model: &'static str,
    /// Found in the name its driver gives it (USB: "Apple Inc. …").
    pub matches: &'static str,
    /// USB vendor and product ids, as seen.
    pub ids: (u32, u32),
    /// Its picture (an Olo icon).
    pub icon: &'static str,
    pub keypad: bool,
    /// A Touch ID sensor, and whether Olo can use it.
    pub touch_id: Option<TouchId>,
    /// How it connects.
    pub connection: &'static str,
}

/// What a keyboard's Touch ID sensor is to Olo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchId {
    /// Paired to a Mac's Secure Enclave, and only that: what it senses is
    /// sealed for it (Apple's CA, ECDHE, AES-GCM), so no other computer
    /// reads a fingerprint from it (TODO's "Keyboards we know").
    SealedToAMac,
}

pub const KNOWN: &[Model] = &[Model {
    name: "Magic Keyboard with Touch ID and Numeric Keypad",
    maker: "Apple",
    model: "A2520",
    matches: "Magic Keyboard with Touch ID and Numeric Keypad",
    ids: (0x05ac, 0x029f),
    icon: "keyboard",
    keypad: true,
    touch_id: Some(TouchId::SealedToAMac),
    connection: "USB-C, or Bluetooth when paired",
}];

/// The model a keyboard named `name` is, if Olo knows it.
pub fn identify(name: &str) -> Option<&'static Model> {
    KNOWN.iter().find(|m| name.contains(m.matches))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_by_the_name_its_driver_gives() {
        let m = identify("Apple Inc. Magic Keyboard with Touch ID and Numeric Keypad").unwrap();
        assert_eq!((m.model, m.ids), ("A2520", (0x05ac, 0x029f)));
        // Its smaller sibling is another keyboard, not known yet.
        assert!(identify("Apple Inc. Magic Keyboard with Touch ID").is_none());
        assert!(identify("AT Translated Set 2 keyboard").is_none());
    }
}
