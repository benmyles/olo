//! Touchpads: what kind of pad is connected, the profiles that suit each
//! kind, and the settings a profile sets (Preferences ▸ Touchpad).
//!
//! A profile is a starting point for how a pad *feels* (palm and hover
//! filtering, click pressure, acceleration, scrolling, momentum);
//! personal choices (tap to click, natural scrolling, speed) are left as
//! they are. Changing any tuning setting by hand makes the profile
//! "custom". See docs/TOUCHPAD.md for where the numbers come from.

use serde::{Deserialize, Serialize};

/// What app_server knows about a pointing device (`Request::InputDevices`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputDeviceInfo {
    /// The kernel's name for it ("Apple MTP multi-touch").
    pub name: String,
    pub kind: DeviceKind,
    /// Its size in millimetres, when libinput knows it.
    pub size_mm: Option<(f64, f64)>,
    /// It reports each finger's pressure (what Olo's touch filter needs).
    pub pressure: bool,
    /// The profile that suits it.
    pub recommended: String,
    /// The kernel driver's own click thresholds (Apple pads on Apple
    /// silicon, where the driver makes the click from summed pressure):
    /// (press, release). Read only: they belong to the driver.
    pub driver_click: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    /// A MacBook's built-in pad on Apple silicon (M2 and later: MTP).
    AppleMtp,
    /// A MacBook's built-in pad over SPI (M1, and Intel MacBooks).
    AppleSpi,
    /// An external Magic Trackpad.
    MagicTrackpad,
    /// Any other touchpad.
    Touchpad,
    Mouse,
}

impl DeviceKind {
    /// From the device's name and USB/bus ids.
    pub fn detect(name: &str, vendor: u32, product: u32, touchpad: bool) -> DeviceKind {
        let apple = vendor == 0x05ac;
        if name.contains("Apple MTP") {
            DeviceKind::AppleMtp
        } else if name.contains("Apple SPI") || name.contains("Apple Internal Keyboard / Trackpad") || (touchpad && apple && name.contains("SPI")) {
            DeviceKind::AppleSpi
        } else if name.contains("Magic Trackpad") || (apple && matches!(product, 0x030e | 0x0265 | 0x0324)) {
            DeviceKind::MagicTrackpad
        } else if touchpad {
            DeviceKind::Touchpad
        } else {
            DeviceKind::Mouse
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            DeviceKind::AppleMtp => "MacBook trackpad (Apple silicon)",
            DeviceKind::AppleSpi => "MacBook trackpad",
            DeviceKind::MagicTrackpad => "Magic Trackpad",
            DeviceKind::Touchpad => "Touchpad",
            DeviceKind::Mouse => "Mouse",
        }
    }

    /// The profile that suits this kind of pad.
    pub fn recommended(self) -> &'static str {
        match self {
            DeviceKind::AppleMtp | DeviceKind::AppleSpi => "macbook",
            DeviceKind::MagicTrackpad => "magic-trackpad",
            DeviceKind::Touchpad | DeviceKind::Mouse => "standard",
        }
    }
}

/// The profiles, as (id, name, what it is for).
pub const PROFILES: &[(&str, &str, &str)] = &[
    ("macbook", "MacBook", "Palm-proof clicks, hovering fingers ignored, Mac-like scrolling"),
    ("magic-trackpad", "Magic Trackpad", "Mac-like scrolling and momentum, libinput's palm rejection"),
    ("standard", "Standard", "libinput's own behaviour, as on most Linux desktops"),
    ("custom", "Custom", "Your own tuning"),
];

/// Olo's touch filter (a libinput plugin app_server writes and loads):
/// on pads that report each finger's pressure, a touch counts only while
/// it presses, palms are left out, and the click comes from the fingers'
/// pressure alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TouchFilter {
    pub enabled: bool,
    /// A touch starts at this pressure...
    pub touch_on: u32,
    /// ...and ends once it has been below this for `lift_ms`.
    pub touch_off: u32,
    pub lift_ms: u32,
    /// A contact this large (the pad's touch-size units) is a palm.
    pub palm_size: u32,
    /// Make the click from the fingers' pressure (palms left out) instead
    /// of the driver's sum over everything touching.
    pub palm_proof_click: bool,
    /// Summed finger pressure at which a click starts, and ends.
    pub click_press: u32,
    pub click_release: u32,
}

impl Default for TouchFilter {
    fn default() -> Self {
        // Measured on a 16" M2 Max (docs/TOUCHPAD.md): fingers press 14 and
        // up (lightest dips ~6 while moving), palms and hovering fingers
        // 0-12; fingers are at most ~860 across, palms 990-3800. The click
        // thresholds are the driver's own.
        TouchFilter {
            enabled: true,
            touch_on: 13,
            touch_off: 3,
            lift_ms: 60,
            palm_size: 1000,
            palm_proof_click: true,
            click_press: 120,
            click_release: 75,
        }
    }
}

/// Click pressure presets, as macOS has them: (name, press, release).
pub const CLICK_PRESSURES: &[(&str, u32, u32)] = &[("Light", 80, 50), ("Medium", 120, 75), ("Firm", 180, 115)];

impl super::TouchpadSettings {
    /// Set the tuning `profile` stands for (personal choices are kept).
    pub fn apply_profile(&mut self, profile: &str) {
        let standard = profile == "standard";
        let mac = matches!(profile, "macbook" | "magic-trackpad");
        if !mac && !standard {
            self.profile = "custom".into();
            return;
        }
        self.profile = profile.into();
        self.filter = TouchFilter { enabled: profile == "macbook", ..TouchFilter::default() };
        self.accel_profile = "adaptive".into();
        self.scroll_speed = 1.0;
        self.momentum = mac;
        self.glide = 1.0;
        self.drag_lock = "off".into();
        self.three_finger_drag = 0;
        self.dwtp = true;
        self.dwt_timeout_ms = 0;
        self.tap_button_map = "lrm".into();
        self.clickfinger_button_map = "lrm".into();
        self.scroll_method = "two-finger".into();
    }

    /// The tuning as `profile` would set it, to tell whether it still
    /// matches.
    pub fn matches_profile(&self, profile: &str) -> bool {
        let mut probe = self.clone();
        probe.apply_profile(profile);
        probe == *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TouchpadSettings;

    #[test]
    fn kinds() {
        assert_eq!(DeviceKind::detect("Apple MTP multi-touch", 0x05ac, 0x0353, true), DeviceKind::AppleMtp);
        assert_eq!(DeviceKind::detect("Apple SPI Trackpad", 0x05ac, 0x0342, true), DeviceKind::AppleSpi);
        assert_eq!(DeviceKind::detect("Apple Inc. Magic Trackpad", 0x05ac, 0x0265, true), DeviceKind::MagicTrackpad);
        assert_eq!(DeviceKind::detect("SYNA2B46:00 06CB:CDE6 Touchpad", 0x06cb, 0xcde6, true), DeviceKind::Touchpad);
        assert_eq!(DeviceKind::detect("Razer Razer Viper V2 Pro", 0x1532, 0x00a6, false), DeviceKind::Mouse);
        assert_eq!(DeviceKind::AppleMtp.recommended(), "macbook");
    }

    #[test]
    fn profiles_keep_personal_choices() {
        let mut t = TouchpadSettings { tap_to_click: false, natural_scroll: true, speed: 0.4, ..Default::default() };
        t.apply_profile("standard");
        assert!(!t.filter.enabled && !t.momentum);
        assert!(!t.tap_to_click && t.natural_scroll && t.speed == 0.4);
        assert!(t.matches_profile("standard") && !t.matches_profile("macbook"));
        t.apply_profile("macbook");
        assert!(t.filter.enabled && t.momentum && t.matches_profile("macbook"));
        t.scroll_speed = 0.8;
        assert!(!t.matches_profile("macbook"));
    }
}
