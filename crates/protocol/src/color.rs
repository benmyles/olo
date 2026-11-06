//! Displays' colours: what each can show (from its EDID), and how Olo
//! shows colours on it (Preferences ▸ Display; app_server converts).

use olo_color::Primaries;
use serde::{Deserialize, Serialize};

/// What a display says it can show.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DisplayColor {
    /// Its primaries and white: the panel's own where it gives them
    /// (DisplayID), else its EDID's. None: not said (taken for sRGB).
    pub primaries: Option<Primaries>,
    /// Its gamma, if it says (2.2 for most).
    pub gamma: Option<f64>,
    /// Light in cd/m²: the most on a small part of the screen (HDR's
    /// peak), the most over all of it, and its black.
    pub peak_nits: Option<f64>,
    pub full_nits: Option<f64>,
    pub black_nits: Option<f64>,
    /// HDR signals it says it takes (CTA-861): PQ (HDR10), HLG.
    pub pq: bool,
    pub hlg: bool,
    /// It takes BT.2020 RGB, HDR's colours (CTA-861).
    pub bt2020: bool,
}

impl DisplayColor {
    /// Its primaries, if they're believable.
    pub fn primaries(&self) -> Option<Primaries> {
        self.primaries.filter(Primaries::plausible)
    }

    /// Notably wider than sRGB: what's meant for sRGB is converted into
    /// its colours (`ColorMode::Accurate`).
    pub fn wide(&self) -> bool {
        self.primaries().is_some_and(|p| p.wide())
    }

    /// The widest standard gamut it covers, and how much of it.
    pub fn gamut(&self) -> Option<(&'static str, f64)> {
        self.primaries().and_then(|p| p.widest_covered())
    }
}

/// How a wide-gamut display shows colours meant for sRGB (what nearly
/// everything is drawn in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// As they're meant: converted into the display's colours, so an
    /// sRGB red is sRGB's red. Wider colours are for what says it has
    /// them.
    #[default]
    Accurate,
    /// Stretched over the display's own colours: brighter, more
    /// saturated, and not as anything was made.
    Vivid,
}

/// SDR's white in HDR by default, cd/m² (BT.2408's reference white).
pub const SDR_NITS: f32 = 203.0;

/// SDR's white in HDR, cd/m², at least and at most: what app_server
/// takes and Preferences' slider offers (a bright panel's white is about
/// 600, so SDR can be as bright in HDR as without it).
pub const MIN_SDR_NITS: f32 = 80.0;
pub const MAX_SDR_NITS: f32 = 600.0;
