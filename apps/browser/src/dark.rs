//! Dark pages: View ▸ Dark Pages (Automatic follows the desktop, the
//! default; Always; Never). Done as Chrome's "Auto Dark Mode for Web
//! Contents" does it, colour by colour as pages are drawn rather than to
//! the finished picture: WebRender's scene builder (vendor/webrender,
//! `dark.rs`) recolours each thing a page draws by what it is. Light
//! backgrounds go dark and dark text and lines go light, their lightness
//! turned around with hue and saturation kept (a blue link goes light
//! blue); colours already right for a dark page stay as they are, so a
//! page that is dark by itself is left alone; pictures, video and canvases
//! are never touched. A page without any background (a blank tab, plain
//! text) is drawn on Servo's clear colour, which goes dark the same way.

use serde::{Deserialize, Serialize};

/// The preference.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DarkPages {
    /// When the desktop is dark (or night).
    #[default]
    Auto,
    Always,
    Never,
}

impl DarkPages {
    pub fn on(self, desktop_dark: bool) -> bool {
        match self {
            DarkPages::Auto => desktop_dark,
            DarkPages::Always => true,
            DarkPages::Never => false,
        }
    }
}

/// Darken every page from the next frame on (or stop).
pub fn set(on: bool) {
    webrender::dark::set(on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert!(DarkPages::Auto.on(true) && !DarkPages::Auto.on(false));
        assert!(DarkPages::Always.on(false) && !DarkPages::Never.on(true));
        assert_eq!(serde_json::to_string(&DarkPages::Auto).unwrap(), "\"auto\"");
    }
}
