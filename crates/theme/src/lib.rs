//! The Tabula look: BeOS R5 colours and metrics, in light and dark variants.
//!
//! Both app_server (window tabs and frames) and the Interface Kit (every
//! widget) draw from [`Palette`], so a theme switch changes everything in
//! lock-step. Colours are sRGB `[r, g, b, a]` bytes.

use serde::{Deserialize, Serialize};

pub mod toolkit;

pub type Rgba = [u8; 4];

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// Every colour the desktop draws with. Names follow the Haiku/BeOS
/// `color_which` constants where one exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub mode: ThemeMode,
    // Interface Kit (B_PANEL_BACKGROUND_COLOR and friends)
    pub panel_background: Rgba,
    pub panel_text: Rgba,
    pub document_background: Rgba,
    pub document_text: Rgba,
    pub control_background: Rgba,
    pub control_text: Rgba,
    pub control_border: Rgba,
    pub control_highlight: Rgba,
    pub control_mark: Rgba,
    pub navigation_base: Rgba,
    pub navigation_pulse: Rgba,
    pub shine: Rgba,
    pub shadow: Rgba,
    pub menu_background: Rgba,
    pub menu_selected_background: Rgba,
    pub menu_item_text: Rgba,
    pub menu_selected_item_text: Rgba,
    pub menu_selected_border: Rgba,
    pub tooltip_background: Rgba,
    pub tooltip_text: Rgba,
    pub list_background: Rgba,
    pub list_selected_background: Rgba,
    pub list_item_text: Rgba,
    pub list_selected_item_text: Rgba,
    pub success: Rgba,
    pub failure: Rgba,
    pub keyboard_navigation: Rgba,
    // Desktop
    pub desktop_text: Rgba,
    pub desktop_text_shadow: Rgba,
    pub status_bar: Rgba,
    // Window decorator (app_server)
    pub tab_focused: Rgba,
    pub tab_focused_light: Rgba,
    pub tab_focused_shadow: Rgba,
    pub tab_unfocused: Rgba,
    pub tab_unfocused_light: Rgba,
    pub tab_unfocused_shadow: Rgba,
    pub tab_text_focused: Rgba,
    pub tab_text_unfocused: Rgba,
    /// Frame bevel, outermost line first. Index 0..5.
    pub frame_focused: [Rgba; 6],
    pub frame_unfocused: [Rgba; 6],
}

impl Palette {
    pub fn for_mode(mode: ThemeMode) -> Palette {
        match mode {
            ThemeMode::Light => Palette::light(),
            ThemeMode::Dark => Palette::dark(),
        }
    }

    /// BeOS R5 as shipped.
    pub fn light() -> Palette {
        Palette {
            mode: ThemeMode::Light,
            panel_background: rgb(216, 216, 216),
            panel_text: rgb(0, 0, 0),
            document_background: rgb(255, 255, 255),
            document_text: rgb(0, 0, 0),
            control_background: rgb(245, 245, 245),
            control_text: rgb(0, 0, 0),
            control_border: rgb(96, 96, 96),
            control_highlight: rgb(102, 152, 203),
            control_mark: rgb(0, 0, 0),
            navigation_base: rgb(0, 0, 229),
            navigation_pulse: rgb(0, 0, 0),
            shine: rgb(255, 255, 255),
            shadow: rgb(0, 0, 0),
            menu_background: rgb(216, 216, 216),
            menu_selected_background: rgb(152, 152, 152),
            menu_item_text: rgb(0, 0, 0),
            menu_selected_item_text: rgb(255, 255, 255),
            menu_selected_border: rgb(96, 96, 96),
            tooltip_background: rgb(255, 255, 214),
            tooltip_text: rgb(0, 0, 0),
            list_background: rgb(255, 255, 255),
            list_selected_background: rgb(190, 190, 190),
            list_item_text: rgb(0, 0, 0),
            list_selected_item_text: rgb(0, 0, 0),
            success: rgb(0, 160, 0),
            failure: rgb(220, 0, 0),
            keyboard_navigation: rgb(0, 0, 229),
            desktop_text: rgb(255, 255, 255),
            desktop_text_shadow: [0, 0, 0, 160],
            status_bar: rgb(50, 150, 255),
            tab_focused: rgb(255, 203, 0),
            tab_focused_light: rgb(255, 230, 128),
            tab_focused_shadow: rgb(176, 136, 0),
            tab_unfocused: rgb(232, 232, 232),
            tab_unfocused_light: rgb(250, 250, 250),
            tab_unfocused_shadow: rgb(168, 168, 168),
            tab_text_focused: rgb(0, 0, 0),
            tab_text_unfocused: rgb(80, 80, 80),
            frame_focused: [
                rgb(108, 108, 108),
                rgb(255, 255, 255),
                rgb(216, 216, 216),
                rgb(216, 216, 216),
                rgb(152, 152, 152),
                rgb(108, 108, 108),
            ],
            frame_unfocused: [
                rgb(136, 136, 136),
                rgb(250, 250, 250),
                rgb(224, 224, 224),
                rgb(224, 224, 224),
                rgb(168, 168, 168),
                rgb(136, 136, 136),
            ],
        }
    }

    /// The same design language at night: dark panels, the yellow tab kept
    /// (it is the identity), slightly deepened so it doesn't glare.
    pub fn dark() -> Palette {
        Palette {
            mode: ThemeMode::Dark,
            panel_background: rgb(43, 43, 43),
            panel_text: rgb(224, 224, 224),
            document_background: rgb(28, 28, 28),
            document_text: rgb(230, 230, 230),
            control_background: rgb(58, 58, 58),
            control_text: rgb(230, 230, 230),
            control_border: rgb(20, 20, 20),
            control_highlight: rgb(80, 130, 190),
            control_mark: rgb(240, 240, 240),
            navigation_base: rgb(96, 144, 255),
            navigation_pulse: rgb(240, 240, 240),
            shine: rgb(90, 90, 90),
            shadow: rgb(0, 0, 0),
            menu_background: rgb(50, 50, 50),
            menu_selected_background: rgb(90, 90, 90),
            menu_item_text: rgb(230, 230, 230),
            menu_selected_item_text: rgb(255, 255, 255),
            menu_selected_border: rgb(20, 20, 20),
            tooltip_background: rgb(70, 70, 54),
            tooltip_text: rgb(240, 240, 230),
            list_background: rgb(32, 32, 32),
            list_selected_background: rgb(72, 72, 72),
            list_item_text: rgb(225, 225, 225),
            list_selected_item_text: rgb(255, 255, 255),
            success: rgb(80, 200, 80),
            failure: rgb(255, 90, 80),
            keyboard_navigation: rgb(96, 144, 255),
            desktop_text: rgb(240, 240, 240),
            desktop_text_shadow: [0, 0, 0, 200],
            status_bar: rgb(60, 140, 230),
            tab_focused: rgb(232, 180, 0),
            tab_focused_light: rgb(250, 208, 70),
            tab_focused_shadow: rgb(140, 104, 0),
            tab_unfocused: rgb(70, 70, 70),
            tab_unfocused_light: rgb(96, 96, 96),
            tab_unfocused_shadow: rgb(36, 36, 36),
            tab_text_focused: rgb(0, 0, 0),
            tab_text_unfocused: rgb(190, 190, 190),
            frame_focused: [
                rgb(10, 10, 10),
                rgb(96, 96, 96),
                rgb(58, 58, 58),
                rgb(58, 58, 58),
                rgb(30, 30, 30),
                rgb(10, 10, 10),
            ],
            frame_unfocused: [
                rgb(16, 16, 16),
                rgb(80, 80, 80),
                rgb(50, 50, 50),
                rgb(50, 50, 50),
                rgb(32, 32, 32),
                rgb(16, 16, 16),
            ],
        }
    }
}

/// Linear blend, BeOS `tint_color`-style helpers.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2]), l(a[3], b[3])]
}

/// `tint > 1` darkens, `< 1` lightens, as in Haiku's `tint_color()`
/// (B_DARKEN_1_TINT = 1.147, B_LIGHTEN_1_TINT = 0.590, ...).
pub fn tint(c: Rgba, tint: f32) -> Rgba {
    if tint >= 1.0 {
        mix(c, [0, 0, 0, c[3]], (tint - 1.0).min(1.0))
    } else {
        mix(c, [255, 255, 255, c[3]], 1.0 - tint)
    }
}

pub const B_LIGHTEN_MAX_TINT: f32 = 0.0;
pub const B_LIGHTEN_2_TINT: f32 = 0.385;
pub const B_LIGHTEN_1_TINT: f32 = 0.590;
pub const B_NO_TINT: f32 = 1.0;
pub const B_DARKEN_1_TINT: f32 = 1.147;
pub const B_DARKEN_2_TINT: f32 = 1.295;
pub const B_DARKEN_3_TINT: f32 = 1.407;
pub const B_DARKEN_4_TINT: f32 = 1.555;
pub const B_DARKEN_MAX_TINT: f32 = 2.0;

/// Window decorator metrics, in logical pixels.
pub mod deco {
    /// Width of the 3D frame around the content.
    pub const BORDER: i32 = 5;
    pub const TAB_HEIGHT: i32 = 21;
    pub const BOX_SIZE: i32 = 13;
    /// Inset of the close/zoom boxes from the tab's ends.
    pub const BOX_INSET: i32 = 5;
    pub const TITLE_GAP: i32 = 7;
    pub const MIN_TAB_WIDTH: i32 = 90;
    /// Corner region (along each edge) that resizes diagonally.
    pub const CORNER: i32 = 22;
}

pub mod fonts {
    /// Closest freely available face to BeOS's Swiss 721 (a Helvetica).
    pub const UI: &[&str] = &["Nimbus Sans", "Liberation Sans", "DejaVu Sans"];
    pub const UI_BOLD_WEIGHT: u16 = 700;
    /// A deliberate modernisation: BeOS used a Courier; JetBrains Mono is
    /// far more legible on HiDPI screens.
    pub const MONO: &[&str] = &["JetBrains Mono", "DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono"];
    /// Points; BeOS's be_plain_font was 10pt.
    pub const PLAIN_SIZE: f32 = 12.0;
    pub const TAB_SIZE: f32 = 12.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_matches_direction() {
        let base = rgb(216, 216, 216);
        assert!(tint(base, B_DARKEN_2_TINT)[0] < base[0]);
        assert!(tint(base, B_LIGHTEN_1_TINT)[0] > base[0]);
        assert_eq!(tint(base, B_NO_TINT), base);
    }
}
