//! Terminal colour schemes: Olo's own (a warm paper white with ink-dark
//! text in light mode and a soft graphite in dark mode, with ANSI colours
//! tuned so every one of the 16 reads clearly on the background: no neon
//! green on black) and Catppuccin's four flavours, as its palette
//! (v1.8.0) gives them. One is chosen for light mode and one for dark
//! (`SchemeId::light` / `dark`); Frappé and Macchiato by default.

use alacritty_terminal::{
    term::color::Colors,
    vte::ansi::{Color, NamedColor, Rgb},
};
use olo_kit::egui::Color32;

#[derive(Debug, Clone, PartialEq)]
pub struct Scheme {
    pub name: &'static str,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// Black, red, green, yellow, blue, magenta, cyan, white; then bright.
    pub ansi: [Rgb; 16],
}

/// A scheme Terminal offers, as kept in its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemeId {
    OloLight,
    OloDark,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl SchemeId {
    /// The ones offered for light mode, and for dark mode (Frappé, the
    /// lightest of Catppuccin's darks, is a light-mode choice here).
    pub const LIGHT: [SchemeId; 3] = [SchemeId::OloLight, SchemeId::Latte, SchemeId::Frappe];
    pub const DARK: [SchemeId; 3] = [SchemeId::OloDark, SchemeId::Macchiato, SchemeId::Mocha];

    pub fn light() -> SchemeId {
        SchemeId::Frappe
    }

    pub fn dark() -> SchemeId {
        SchemeId::Macchiato
    }

    pub fn scheme(self) -> Scheme {
        match self {
            SchemeId::OloLight => Scheme::light(),
            SchemeId::OloDark => Scheme::dark(),
            SchemeId::Latte => Scheme::latte(),
            SchemeId::Frappe => Scheme::frappe(),
            SchemeId::Macchiato => Scheme::macchiato(),
            SchemeId::Mocha => Scheme::mocha(),
        }
    }

    /// Its name in menus.
    pub fn name(self) -> &'static str {
        self.scheme().name
    }

    /// Its menu command, and back.
    pub fn command(self) -> String {
        format!("colors:{}", serde_json::to_value(self).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default())
    }

    pub fn from_command(command: &str) -> Option<SchemeId> {
        serde_json::from_value(serde_json::Value::String(command.strip_prefix("colors:")?.to_owned())).ok()
    }
}

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

impl Scheme {
    pub fn light() -> Scheme {
        Scheme {
            name: "Olo Light",
            foreground: rgb(0x26292e),
            background: rgb(0xfbfaf6),
            cursor: rgb(0x2f6fdb),
            selection: rgb(0xc9dcf7),
            ansi: [
                rgb(0x3b3f45), rgb(0xc4314b), rgb(0x2f8a3e), rgb(0x9a6a00),
                rgb(0x2f6fdb), rgb(0x9d3fb5), rgb(0x0f8494), rgb(0x9a9ca2),
                rgb(0x6a6e75), rgb(0xe0475f), rgb(0x3aa34d), rgb(0xb98400),
                rgb(0x4b8af0), rgb(0xb85bd0), rgb(0x13939f), rgb(0xd4d5d8),
            ],
        }
    }

    pub fn dark() -> Scheme {
        Scheme {
            name: "Olo Dark",
            foreground: rgb(0xd9dce1),
            background: rgb(0x1d2025),
            cursor: rgb(0x7cb4ff),
            selection: rgb(0x2f405a),
            ansi: [
                rgb(0x2c3037), rgb(0xec6a78), rgb(0x8fcf7d), rgb(0xe8c17a),
                rgb(0x6aaef5), rgb(0xc98ae6), rgb(0x5cc4cf), rgb(0xb4b8bf),
                rgb(0x5f6570), rgb(0xff8592), rgb(0xa9e395), rgb(0xf7d595),
                rgb(0x8cc2ff), rgb(0xdca5f2), rgb(0x7ad6df), rgb(0xeceef1),
            ],
        }
    }

    /// A Catppuccin flavour, as its own ports draw it: Text on Base, the
    /// palette's ANSI colours, a Rosewater cursor, and the selection in
    /// Overlay 2 at a quarter over Base (its style guide's 20–30%).
    fn catppuccin(name: &'static str, text: u32, base: u32, overlay2: u32, rosewater: u32, ansi: [u32; 16]) -> Scheme {
        Scheme {
            name,
            foreground: rgb(text),
            background: rgb(base),
            cursor: rgb(rosewater),
            selection: mix(rgb(overlay2), rgb(base), 0.25),
            ansi: ansi.map(rgb),
        }
    }

    pub fn latte() -> Scheme {
        Scheme::catppuccin("Catppuccin Latte", 0x4c4f69, 0xeff1f5, 0x7c7f93, 0xdc8a78, [
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
            0x6c6f85, 0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
        ])
    }

    pub fn frappe() -> Scheme {
        Scheme::catppuccin("Catppuccin Frappé", 0xc6d0f5, 0x303446, 0x949cbb, 0xf2d5cf, [
            0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
            0x626880, 0xe67172, 0x8ec772, 0xd9ba73, 0x7b9ef0, 0xf2a4db, 0x5abfb5, 0xb5bfe2,
        ])
    }

    pub fn macchiato() -> Scheme {
        Scheme::catppuccin("Catppuccin Macchiato", 0xcad3f5, 0x24273a, 0x939ab7, 0xf4dbd6, [
            0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
            0x5b6078, 0xec7486, 0x8ccf7f, 0xe1c682, 0x78a1f6, 0xf2a9dd, 0x63cbc0, 0xb8c0e0,
        ])
    }

    pub fn mocha() -> Scheme {
        Scheme::catppuccin("Catppuccin Mocha", 0xcdd6f4, 0x1e1e2e, 0x9399b2, 0xf5e0dc, [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
            0x585b70, 0xf37799, 0x89d88b, 0xebd391, 0x74a8fc, 0xf2aede, 0x6bd7ca, 0xbac2de,
        ])
    }

    /// The colour `index` of the 256-colour palette.
    fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..16 => self.ansi[i as usize],
            16..232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }

    /// Resolve a cell colour, honouring colours the program changed with
    /// OSC 4/10/11.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(c) => c,
            Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => overrides[n].unwrap_or_else(|| self.named(n)),
        }
    }

    fn named(&self, n: NamedColor) -> Rgb {
        use NamedColor::*;
        match n {
            Foreground | BrightForeground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            DimForeground => mix(self.foreground, self.background, 0.66),
            DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
                let base = n as usize - DimBlack as usize;
                mix(self.ansi[base], self.background, 0.66)
            }
            _ => self.ansi[(n as usize).min(15)],
        }
    }
}

/// `a` weighted `t` against `b`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

pub fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn lum(c: Rgb) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn readable() {
        for s in [Scheme::light(), Scheme::dark()] {
            assert!(contrast(s.foreground, s.background) > 10.0, "{} text", s.name);
            // Colours used as text: everything but the one that matches
            // the background (white in light mode, black in dark mode).
            let skip = if s.name.contains("Light") { [7, 15] } else { [0, 8] };
            for (i, c) in s.ansi.iter().enumerate() {
                if skip.contains(&i) {
                    continue;
                }
                assert!(contrast(*c, s.background) >= 3.0, "{} colour {i}: {:.2}", s.name, contrast(*c, s.background));
            }
            assert!(contrast(s.foreground, s.selection) > 7.0, "{} selection", s.name);
        }
    }

    /// Catppuccin is drawn as its palette gives it, not "improved": its
    /// colours are checked against palette.json, and only for what it
    /// promises (text that reads, a selection that doesn't hide it).
    #[test]
    fn catppuccin_as_its_palette() {
        let s = Scheme::mocha();
        assert_eq!((s.foreground, s.background, s.cursor), (rgb(0xcdd6f4), rgb(0x1e1e2e), rgb(0xf5e0dc)));
        assert_eq!((s.ansi[1], s.ansi[9], s.ansi[15]), (rgb(0xf38ba8), rgb(0xf37799), rgb(0xbac2de)));
        // Overlay 2 at a quarter over Base (Ghostty's port has #353749 at a fifth).
        assert_eq!(s.selection, rgb(0x3b3d4f));
        let l = Scheme::latte();
        assert_eq!((l.foreground, l.background, l.ansi[0], l.ansi[7]), (rgb(0x4c4f69), rgb(0xeff1f5), rgb(0x5c5f77), rgb(0xacb0be)));
        assert_eq!((Scheme::frappe().background, Scheme::macchiato().background), (rgb(0x303446), rgb(0x24273a)));
        for id in SchemeId::LIGHT.into_iter().chain(SchemeId::DARK) {
            let s = id.scheme();
            assert!(contrast(s.foreground, s.background) >= 7.0, "{} text: {:.2}", s.name, contrast(s.foreground, s.background));
            assert!(contrast(s.foreground, s.selection) >= 4.5, "{} selection: {:.2}", s.name, contrast(s.foreground, s.selection));
            assert_ne!(s.cursor, s.background, "{} cursor", s.name);
        }
    }

    #[test]
    fn choices() {
        // Frappé in light mode and Macchiato in dark, unless chosen.
        assert_eq!((SchemeId::light(), SchemeId::dark()), (SchemeId::Frappe, SchemeId::Macchiato));
        assert!(SchemeId::LIGHT.contains(&SchemeId::light()) && SchemeId::DARK.contains(&SchemeId::dark()));
        for id in SchemeId::LIGHT.into_iter().chain(SchemeId::DARK) {
            assert_eq!(SchemeId::from_command(&id.command()), Some(id), "{}", id.command());
        }
        assert_eq!(SchemeId::Frappe.command(), "colors:frappe");
        assert_eq!(SchemeId::OloLight.command(), "colors:olo-light");
        assert_eq!(SchemeId::from_command("colors:nope"), None);
        assert_eq!(SchemeId::Frappe.name(), "Catppuccin Frappé");
    }

    #[test]
    fn cube_and_grays() {
        let s = Scheme::dark();
        assert_eq!(s.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(s.indexed(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(s.indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(s.indexed(255), Rgb { r: 238, g: 238, b: 238 });
    }
}
