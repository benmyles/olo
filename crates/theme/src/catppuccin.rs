//! Catppuccin's four flavours, as its palette (v1.8.0) gives them: Latte
//! (light), Frappé, Macchiato and Mocha (darker each), each with its
//! fourteen accents, its ramp from Text down to Crust, and the 16 ANSI
//! colours its terminal ports use. [`crate::looks`] makes Olo palettes
//! of them; terminals take them as they are ([`Flavour::terminal`]).

use crate::{Rgba, Terminal, ThemeMode, hex, mix};

/// One of Catppuccin's accent colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Rosewater,
    Flamingo,
    Pink,
    Mauve,
    Red,
    Maroon,
    Peach,
    Yellow,
    Green,
    Teal,
    Sky,
    Sapphire,
    Blue,
    Lavender,
}

impl Accent {
    pub fn name(self) -> &'static str {
        match self {
            Accent::Rosewater => "Rosewater",
            Accent::Flamingo => "Flamingo",
            Accent::Pink => "Pink",
            Accent::Mauve => "Mauve",
            Accent::Red => "Red",
            Accent::Maroon => "Maroon",
            Accent::Peach => "Peach",
            Accent::Yellow => "Yellow",
            Accent::Green => "Green",
            Accent::Teal => "Teal",
            Accent::Sky => "Sky",
            Accent::Sapphire => "Sapphire",
            Accent::Blue => "Blue",
            Accent::Lavender => "Lavender",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flavour {
    /// What Terminal's settings keep for it.
    pub id: &'static str,
    pub name: &'static str,
    pub mode: ThemeMode,
    pub rosewater: Rgba,
    pub flamingo: Rgba,
    pub pink: Rgba,
    pub mauve: Rgba,
    pub red: Rgba,
    pub maroon: Rgba,
    pub peach: Rgba,
    pub yellow: Rgba,
    pub green: Rgba,
    pub teal: Rgba,
    pub sky: Rgba,
    pub sapphire: Rgba,
    pub blue: Rgba,
    pub lavender: Rgba,
    pub text: Rgba,
    pub subtext1: Rgba,
    pub subtext0: Rgba,
    pub overlay2: Rgba,
    pub overlay1: Rgba,
    pub overlay0: Rgba,
    pub surface2: Rgba,
    pub surface1: Rgba,
    pub surface0: Rgba,
    pub base: Rgba,
    pub mantle: Rgba,
    pub crust: Rgba,
    /// Its terminal ports' ANSI colours.
    pub ansi: [Rgba; 16],
}

impl Flavour {
    pub fn accent(&self, a: Accent) -> Rgba {
        match a {
            Accent::Rosewater => self.rosewater,
            Accent::Flamingo => self.flamingo,
            Accent::Pink => self.pink,
            Accent::Mauve => self.mauve,
            Accent::Red => self.red,
            Accent::Maroon => self.maroon,
            Accent::Peach => self.peach,
            Accent::Yellow => self.yellow,
            Accent::Green => self.green,
            Accent::Teal => self.teal,
            Accent::Sky => self.sky,
            Accent::Sapphire => self.sapphire,
            Accent::Blue => self.blue,
            Accent::Lavender => self.lavender,
        }
    }

    /// As its own ports draw a terminal: Text on Base, a Rosewater
    /// cursor, and the selection in Overlay 2 at a quarter over Base (its
    /// style guide's 20–30%).
    pub fn terminal(&self) -> Terminal {
        Terminal {
            id: self.id.into(),
            name: self.name.into(),
            mode: self.mode,
            foreground: self.text,
            background: self.base,
            cursor: self.rosewater,
            selection: mix(self.base, self.overlay2, 0.25),
            ansi: self.ansi,
        }
    }
}

pub const LATTE: Flavour = Flavour {
    id: "latte",
    name: "Catppuccin Latte",
    mode: ThemeMode::Light,
    rosewater: hex(0xdc8a78),
    flamingo: hex(0xdd7878),
    pink: hex(0xea76cb),
    mauve: hex(0x8839ef),
    red: hex(0xd20f39),
    maroon: hex(0xe64553),
    peach: hex(0xfe640b),
    yellow: hex(0xdf8e1d),
    green: hex(0x40a02b),
    teal: hex(0x179299),
    sky: hex(0x04a5e5),
    sapphire: hex(0x209fb5),
    blue: hex(0x1e66f5),
    lavender: hex(0x7287fd),
    text: hex(0x4c4f69),
    subtext1: hex(0x5c5f77),
    subtext0: hex(0x6c6f85),
    overlay2: hex(0x7c7f93),
    overlay1: hex(0x8c8fa1),
    overlay0: hex(0x9ca0b0),
    surface2: hex(0xacb0be),
    surface1: hex(0xbcc0cc),
    surface0: hex(0xccd0da),
    base: hex(0xeff1f5),
    mantle: hex(0xe6e9ef),
    crust: hex(0xdce0e8),
    ansi: [
        hex(0x5c5f77), hex(0xd20f39), hex(0x40a02b), hex(0xdf8e1d),
        hex(0x1e66f5), hex(0xea76cb), hex(0x179299), hex(0xacb0be),
        hex(0x6c6f85), hex(0xde293e), hex(0x49af3d), hex(0xeea02d),
        hex(0x456eff), hex(0xfe85d8), hex(0x2d9fa8), hex(0xbcc0cc),
    ],
};

pub const FRAPPE: Flavour = Flavour {
    id: "frappe",
    name: "Catppuccin Frappé",
    mode: ThemeMode::Dark,
    rosewater: hex(0xf2d5cf),
    flamingo: hex(0xeebebe),
    pink: hex(0xf4b8e4),
    mauve: hex(0xca9ee6),
    red: hex(0xe78284),
    maroon: hex(0xea999c),
    peach: hex(0xef9f76),
    yellow: hex(0xe5c890),
    green: hex(0xa6d189),
    teal: hex(0x81c8be),
    sky: hex(0x99d1db),
    sapphire: hex(0x85c1dc),
    blue: hex(0x8caaee),
    lavender: hex(0xbabbf1),
    text: hex(0xc6d0f5),
    subtext1: hex(0xb5bfe2),
    subtext0: hex(0xa5adce),
    overlay2: hex(0x949cbb),
    overlay1: hex(0x838ba7),
    overlay0: hex(0x737994),
    surface2: hex(0x626880),
    surface1: hex(0x51576d),
    surface0: hex(0x414559),
    base: hex(0x303446),
    mantle: hex(0x292c3c),
    crust: hex(0x232634),
    ansi: [
        hex(0x51576d), hex(0xe78284), hex(0xa6d189), hex(0xe5c890),
        hex(0x8caaee), hex(0xf4b8e4), hex(0x81c8be), hex(0xa5adce),
        hex(0x626880), hex(0xe67172), hex(0x8ec772), hex(0xd9ba73),
        hex(0x7b9ef0), hex(0xf2a4db), hex(0x5abfb5), hex(0xb5bfe2),
    ],
};

pub const MACCHIATO: Flavour = Flavour {
    id: "macchiato",
    name: "Catppuccin Macchiato",
    mode: ThemeMode::Dark,
    rosewater: hex(0xf4dbd6),
    flamingo: hex(0xf0c6c6),
    pink: hex(0xf5bde6),
    mauve: hex(0xc6a0f6),
    red: hex(0xed8796),
    maroon: hex(0xee99a0),
    peach: hex(0xf5a97f),
    yellow: hex(0xeed49f),
    green: hex(0xa6da95),
    teal: hex(0x8bd5ca),
    sky: hex(0x91d7e3),
    sapphire: hex(0x7dc4e4),
    blue: hex(0x8aadf4),
    lavender: hex(0xb7bdf8),
    text: hex(0xcad3f5),
    subtext1: hex(0xb8c0e0),
    subtext0: hex(0xa5adcb),
    overlay2: hex(0x939ab7),
    overlay1: hex(0x8087a2),
    overlay0: hex(0x6e738d),
    surface2: hex(0x5b6078),
    surface1: hex(0x494d64),
    surface0: hex(0x363a4f),
    base: hex(0x24273a),
    mantle: hex(0x1e2030),
    crust: hex(0x181926),
    ansi: [
        hex(0x494d64), hex(0xed8796), hex(0xa6da95), hex(0xeed49f),
        hex(0x8aadf4), hex(0xf5bde6), hex(0x8bd5ca), hex(0xa5adcb),
        hex(0x5b6078), hex(0xec7486), hex(0x8ccf7f), hex(0xe1c682),
        hex(0x78a1f6), hex(0xf2a9dd), hex(0x63cbc0), hex(0xb8c0e0),
    ],
};

pub const MOCHA: Flavour = Flavour {
    id: "mocha",
    name: "Catppuccin Mocha",
    mode: ThemeMode::Dark,
    rosewater: hex(0xf5e0dc),
    flamingo: hex(0xf2cdcd),
    pink: hex(0xf5c2e7),
    mauve: hex(0xcba6f7),
    red: hex(0xf38ba8),
    maroon: hex(0xeba0ac),
    peach: hex(0xfab387),
    yellow: hex(0xf9e2af),
    green: hex(0xa6e3a1),
    teal: hex(0x94e2d5),
    sky: hex(0x89dceb),
    sapphire: hex(0x74c7ec),
    blue: hex(0x89b4fa),
    lavender: hex(0xb4befe),
    text: hex(0xcdd6f4),
    subtext1: hex(0xbac2de),
    subtext0: hex(0xa6adc8),
    overlay2: hex(0x9399b2),
    overlay1: hex(0x7f849c),
    overlay0: hex(0x6c7086),
    surface2: hex(0x585b70),
    surface1: hex(0x45475a),
    surface0: hex(0x313244),
    base: hex(0x1e1e2e),
    mantle: hex(0x181825),
    crust: hex(0x11111b),
    ansi: [
        hex(0x45475a), hex(0xf38ba8), hex(0xa6e3a1), hex(0xf9e2af),
        hex(0x89b4fa), hex(0xf5c2e7), hex(0x94e2d5), hex(0xa6adc8),
        hex(0x585b70), hex(0xf37799), hex(0x89d88b), hex(0xebd391),
        hex(0x74a8fc), hex(0xf2aede), hex(0x6bd7ca), hex(0xbac2de),
    ],
};

/// Every flavour, lightest first.
pub const FLAVOURS: [&Flavour; 4] = [&LATTE, &FRAPPE, &MACCHIATO, &MOCHA];

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked against Catppuccin's palette.json: drawn as it gives
    /// them, not "improved".
    #[test]
    fn as_its_palette() {
        let t = MOCHA.terminal();
        assert_eq!((t.foreground, t.background, t.cursor), (hex(0xcdd6f4), hex(0x1e1e2e), hex(0xf5e0dc)));
        assert_eq!((t.ansi[1], t.ansi[9], t.ansi[15]), (hex(0xf38ba8), hex(0xf37799), hex(0xbac2de)));
        // Overlay 2 at a quarter over Base (Ghostty's port has #353749 at a fifth).
        assert_eq!(t.selection, hex(0x3b3d4f));
        let l = LATTE.terminal();
        assert_eq!((l.foreground, l.background, l.ansi[0], l.ansi[7]), (hex(0x4c4f69), hex(0xeff1f5), hex(0x5c5f77), hex(0xacb0be)));
        assert_eq!((FRAPPE.base, MACCHIATO.base), (hex(0x303446), hex(0x24273a)));
        for f in FLAVOURS {
            assert!(crate::luminance(f.crust) < crate::luminance(f.base));
            assert_eq!(f.accent(Accent::Blue), f.blue);
        }
    }
}
