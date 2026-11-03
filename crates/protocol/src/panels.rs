//! What kind of panel each display is, and how Olo draws for it: its
//! density, the order of its colour stripes, its technology, and the
//! text rendering that follows from them, each with why. app_server
//! works them out (`OutputInfo::panel`), apps draw by them (the GPUI
//! kit) and Preferences ▸ Display shows them, with the person's
//! corrections (`OutputConfig::subpixel`, `technology`, `text`).

use serde::{Deserialize, Serialize};

/// The order of a display's colour stripes, as built (before rotation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Subpixel {
    #[default]
    Unknown,
    /// No stripes to use: OLED's triangles or pentile, or a projector.
    None,
    /// Red, green, blue, left to right.
    Rgb,
    Bgr,
    /// Stacked: red at the top.
    VerticalRgb,
    VerticalBgr,
}

impl Subpixel {
    pub fn label(self) -> &'static str {
        match self {
            Subpixel::Unknown => "Unknown",
            Subpixel::None => "None (no stripes to use)",
            Subpixel::Rgb => "RGB stripes",
            Subpixel::Bgr => "BGR stripes",
            Subpixel::VerticalRgb => "Vertical RGB stripes",
            Subpixel::VerticalBgr => "Vertical BGR stripes",
        }
    }
}

/// What makes the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Technology {
    #[default]
    Unknown,
    /// Liquid crystal, lit from behind; stripes of red, green and blue.
    Lcd,
    /// LCD lit by thousands of dimmable zones (Apple's XDR).
    MiniLed,
    /// Each pixel its own light; subpixels in triangles or pentile.
    Oled,
}

impl Technology {
    pub fn label(self) -> &'static str {
        match self {
            Technology::Unknown => "Unknown",
            Technology::Lcd => "LCD",
            Technology::MiniLed => "Mini-LED LCD",
            Technology::Oled => "OLED",
        }
    }
}

/// How a display's text is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    /// In a setting: Olo's own choice.
    Auto,
    /// Grey-scale, hinted vertically: stems and the baseline on pixels.
    Hinted,
    /// Grey-scale, the outlines as designed (macOS's way on Retina).
    Unhinted,
    /// A coverage for each colour stripe (ClearType's way), in this order.
    Subpixel(Subpixel),
}

impl TextMode {
    pub fn label(self) -> &'static str {
        match self {
            TextMode::Auto => "Automatic",
            TextMode::Hinted => "Grey-scale, hinted",
            TextMode::Unhinted => "Grey-scale, as designed",
            TextMode::Subpixel(_) => "Subpixel (ClearType-style)",
        }
    }
}

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The display said (its EDID, the kernel).
    Display,
    /// Worked out from what else is known.
    #[default]
    Guess,
    /// Set in Preferences ▸ Display.
    Setting,
}

/// A display's panel and what Olo does for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Panel {
    /// Pixels per inch across, if its size is known.
    pub ppi: Option<f32>,
    pub internal: bool,
    pub technology: Technology,
    pub technology_from: Source,
    pub subpixel: Subpixel,
    pub subpixel_from: Source,
    /// Refresh rate in hertz.
    pub refresh_hz: f32,
    pub text: Option<TextMode>,
    pub text_from: Source,
    /// Why, in a sentence for Preferences.
    pub text_why: String,
}

/// Pixels per inch from a mode's width and the physical width.
pub fn ppi(width_px: i32, width_mm: i32) -> Option<f32> {
    (width_px > 0 && width_mm > 0).then(|| width_px as f32 * 25.4 / width_mm as f32)
}

/// What a display is made of, from its maker and model: OLED by name, the
/// MacBook's XDR panel, else LCD (most monitors and laptops).
pub fn guess_technology(make: &str, model: &str, internal: bool, ppi: Option<f32>) -> Technology {
    let name = format!("{make} {model}").to_ascii_lowercase();
    if name.contains("oled") || OLED_MODELS.iter().any(|m| name.contains(m)) {
        return Technology::Oled;
    }
    // Apple's MacBook Pro panels (14" and 16", 254 ppi) are mini-LED.
    if internal && ppi.is_some_and(|p| (250.0..258.0).contains(&p)) {
        return Technology::MiniLed;
    }
    Technology::Lcd
}

/// Monitors with OLED panels whose names don't say so.
const OLED_MODELS: &[&str] =
    &["aw3423", "aw2725", "aw3225", "pg27aqdm", "pg32ucdm", "pg34wcdm", "lg ultragear 27gr95", "27gs95", "32gs95", "45gr95", "mpg 271qr", "mag 271qpx", "mpg 321urx", "odyssey oled", "g85sb", "g95sc"];

/// The stripes' order when the display doesn't say: an LCD's are RGB
/// across (nearly all are), an OLED's can't be used.
pub fn guess_subpixel(technology: Technology) -> Subpixel {
    match technology {
        Technology::Oled => Subpixel::None,
        _ => Subpixel::Rgb,
    }
}

/// How text is best drawn on a display, and why: Retina density gets
/// the outlines as designed (pixels too small to need hinting, as macOS);
/// a low-density display with stripes across, a coverage per stripe
/// (a third of a pixel's sharpness across); anything else grey-scale,
/// hinted so stems and the baseline land on pixels. `rotation` is the
/// display's, clockwise degrees.
pub fn text_mode(ppi: Option<f32>, scale: f64, subpixel: Subpixel, rotation: u16) -> (TextMode, String) {
    if let Some(p) = ppi {
        if p >= 190.0 {
            return (TextMode::Unhinted, format!("At {p:.0} pixels per inch, pixels are too small to see: text is drawn exactly as designed, as macOS does on Retina displays."));
        }
    } else if scale >= 2.0 {
        return (TextMode::Unhinted, "Shown at 2× or more: text is drawn exactly as designed, as on Retina displays.".into());
    }
    let turned = |s: Subpixel| match (s, rotation) {
        (Subpixel::Rgb, 0) | (Subpixel::Bgr, 180) => Some(Subpixel::Rgb),
        (Subpixel::Bgr, 0) | (Subpixel::Rgb, 180) => Some(Subpixel::Bgr),
        (Subpixel::VerticalRgb, 90) | (Subpixel::VerticalBgr, 270) => Some(Subpixel::Rgb),
        (Subpixel::VerticalBgr, 90) | (Subpixel::VerticalRgb, 270) => Some(Subpixel::Bgr),
        _ => None,
    };
    let density = ppi.map_or_else(|| "this density".to_string(), |p| format!("{p:.0} pixels per inch"));
    if ppi.is_some_and(|p| p < 150.0) {
        if let Some(across) = turned(subpixel) {
            return (TextMode::Subpixel(across), format!("At {density}, each pixel's red, green and blue stripes are drawn separately, as ClearType does: a third of a pixel's sharpness across."));
        }
        let why = match subpixel {
            Subpixel::None => "its pixels have no stripes to draw with",
            Subpixel::Unknown => "the order of its colour stripes isn't known",
            _ => "it's turned so its stripes run down",
        };
        return (TextMode::Hinted, format!("At {density}, text is hinted so its stems and baseline fall on whole pixels; not drawn per stripe, as {why}."));
    }
    (TextMode::Hinted, format!("At {density}, text is hinted so its stems and baseline fall on whole pixels, crisp without colour fringes."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bens_displays() {
        // The MacBook Pro 16" XDR, a Studio Display, a Dell S2425HS.
        let xdr = ppi(3456, 346);
        assert!(xdr.is_some_and(|p| (253.0..255.0).contains(&p)));
        assert_eq!(guess_technology("Unknown", "Unknown", true, xdr), Technology::MiniLed);
        assert_eq!(text_mode(xdr, 2.0, Subpixel::Unknown, 0).0, TextMode::Unhinted);
        let studio = ppi(5120, 600);
        assert_eq!(guess_technology("Apple Computer Inc", "StudioDisplay", false, studio), Technology::Lcd);
        assert_eq!(text_mode(studio, 2.0, Subpixel::Rgb, 0).0, TextMode::Unhinted);
        let dell = ppi(1920, 530);
        let tech = guess_technology("Dell Inc.", "DELL S2425HS", false, dell);
        assert_eq!(tech, Technology::Lcd);
        assert_eq!(text_mode(dell, 1.0, guess_subpixel(tech), 0).0, TextMode::Subpixel(Subpixel::Rgb));
        // Turned upside down, its stripes run the other way.
        assert_eq!(text_mode(dell, 1.0, Subpixel::Rgb, 180).0, TextMode::Subpixel(Subpixel::Bgr));
        // On its side, they run down.
        assert_eq!(text_mode(dell, 1.0, Subpixel::Rgb, 90).0, TextMode::Hinted);
    }

    #[test]
    fn oled_has_no_stripes() {
        let t = guess_technology("Dell Inc.", "AW3423DWF", false, ppi(3440, 800));
        assert_eq!(t, Technology::Oled);
        assert_eq!(text_mode(ppi(3440, 800), 1.0, guess_subpixel(t), 0).0, TextMode::Hinted);
    }
}
