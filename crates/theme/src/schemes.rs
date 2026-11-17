//! Colour schemes from elsewhere, as colour sets for either theme
//! ([`crate::looks::ColorSet::Scheme`]) and as colours for Terminal
//! ([`crate::looks::terminals`]): Catppuccin's flavours, and the ten
//! colour themes most installed in the editors' marketplace (VS Code's,
//! which Cursor shares; counted 2026-11-17, icon themes, language packs
//! and the withdrawn Material Theme left out): GitHub, One Dark Pro,
//! Dracula, Ayu, Monokai, Winter is Coming, Night Owl, Tokyo Night,
//! Palenight and SynthWave '84.
//!
//! Each is a pair, light and dark, as Olo's looks are. Where its makers
//! publish both, those are the pair (GitHub Light and Dark, One Light and
//! One Dark, Dracula and its Alucard, Ayu, Winter is Coming, Night Owl and
//! Light Owl, Tokyo Night and its Light; Catppuccin's three dark flavours
//! each with Latte). Where they publish one, the other is Olo's, drawn
//! from the same hues (Monokai, Palenight, SynthWave '84).
//!
//! **Tuned for Olo.** A scheme is a handful of colours here ([`Variant`]):
//! its grounds, its ink, its accents. The palette is built from them in
//! BeOS's proportions as a theme's own is ([`crate::looks::build`]), and
//! what wouldn't read is moved until it does (text at 4.5 to 1, a
//! terminal's colours at 3 to 1 on its ground, its text at 7 to 1:
//! [`crate::readable`]), so a colour here is not always the hex its
//! makers list. The schemes are their makers' (all under the MIT licence,
//! but Monokai, whose colours are Wimer Hazenberg's); the tuning is Olo's.

use crate::{Hues, Rgba, Terminal, ThemeMode, WHITE, contrast, hex, looks::Seeds, mix, readable};

/// A scheme in one mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// Its makers' name for it ("Latte", "Mocha", "Alucard"; "Light",
    /// "Dark" where it has none of its own).
    pub name: &'static str,
    /// Documents, lists and the terminal (an editor's ground).
    pub ground: Rgba,
    /// Panels: windows, toolbars, menus (an editor's side bar).
    pub panel: Rgba,
    /// Controls' faces (an editor's inputs and hovered rows).
    pub raised: Rgba,
    pub ink: Rgba,
    /// Comments: the scheme's grey.
    pub muted: Rgba,
    /// What is chosen: selections, the default button.
    pub accent: Rgba,
    /// The focused window's tab: the scheme's own colour.
    pub tab: Rgba,
    pub red: Rgba,
    pub orange: Rgba,
    pub yellow: Rgba,
    pub green: Rgba,
    pub teal: Rgba,
    pub blue: Rgba,
    pub purple: Rgba,
    pub pink: Rgba,
    /// A terminal's black and bright black, white and bright white (its
    /// other twelve are the hues above, the bright ones a little
    /// brighter).
    pub blacks: [Rgba; 2],
    pub whites: [Rgba; 2],
}

/// A scheme: its pair.
#[derive(Debug, PartialEq, Eq)]
pub struct Scheme {
    /// What settings hold (`Appearance::colors`), and the stem of its
    /// terminals' ids (`dracula-light`, `dracula-dark`).
    pub id: &'static str,
    pub name: &'static str,
    /// A line about it, for Preferences.
    pub about: &'static str,
    pub light: Variant,
    pub dark: Variant,
}

impl Scheme {
    pub fn variant(&self, mode: ThemeMode) -> &Variant {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }

    /// Its terminal's name in `mode`, as its makers name that variant:
    /// "Catppuccin Latte", "Dracula Alucard", "GitHub Dark", "One Light",
    /// "Light Owl", "Tokyo Night" (its night is the scheme itself).
    pub fn variant_name(&self, mode: ThemeMode) -> String {
        let v = self.variant(mode).name;
        let family = self.name.split(' ').next().unwrap_or(self.name);
        match v {
            // (A name of its own that says whose it is.)
            "One Light" | "One Dark" | "Light Owl" => v.to_owned(),
            // (The scheme is named for it.)
            "Night" => self.name.to_owned(),
            _ if self.name.ends_with(v) => self.name.to_owned(),
            // (Catppuccin's flavours go by the family's name.)
            "Latte" => format!("{family} {v}"),
            _ => format!("{} {v}", self.name),
        }
    }

    /// The seeds of its palette in `mode` (`looks::build`).
    pub fn seeds(&self, mode: ThemeMode) -> Seeds {
        let v = self.variant(mode);
        let light = mode == ThemeMode::Light;
        // (Whichever of the scheme's own two ends reads better on a fill.)
        let on = |fill: Rgba| {
            let (a, b) = if light { (v.ground, v.ink) } else { (v.ink, v.ground) };
            let best = [a, b, WHITE, hex(0x000000)].into_iter().max_by(|x, y| contrast(*x, fill).total_cmp(&contrast(*y, fill))).unwrap_or(a);
            readable(if contrast(a, fill) >= 4.5 { a } else { best }, fill, 4.5)
        };
        Seeds {
            panel: v.panel,
            ink: readable(v.ink, v.panel, 7.0),
            document: v.ground,
            control: v.raised,
            highlight: v.accent,
            on_highlight: on(v.accent),
            navigation: readable(v.blue, v.panel, 3.0),
            tab: v.tab,
            on_tab: on(v.tab),
            tooltip: if light { mix(v.ground, v.yellow, 0.12) } else { mix(v.raised, v.ink, 0.06) },
            status: v.blue,
            success: readable(v.green, v.panel, 3.0),
            failure: readable(v.red, v.panel, 3.0),
            warning: readable(v.yellow, v.panel, 3.0),
            hues: Hues {
                red: v.red,
                orange: v.orange,
                yellow: v.yellow,
                green: v.green,
                teal: v.teal,
                blue: v.blue,
                purple: v.purple,
                pink: v.pink,
                brown: mix(v.orange, v.muted, 0.5),
                gray: v.muted,
            },
            border: Some(if light { mix(v.panel, v.ink, 0.38) } else { mix(v.panel, hex(0x000000), 0.45) }),
            gloss: 0,
        }
    }

    /// Its terminal's colours in `mode`: the scheme's hues as the sixteen,
    /// each kept readable on its ground.
    pub fn terminal(&self, mode: ThemeMode) -> Terminal {
        let v = self.variant(mode);
        let light = mode == ThemeMode::Light;
        let bg = v.ground;
        // (A bright colour: a step towards the ink's end, as the schemes'
        // own terminals have them.)
        let bright = |c: Rgba| mix(c, if light { hex(0x000000) } else { WHITE }, if light { 0.14 } else { 0.16 });
        let six = [v.red, v.green, v.yellow, v.blue, v.purple, v.teal];
        let mut ansi = [bg; 16];
        (ansi[0], ansi[8], ansi[7], ansi[15]) = (v.blacks[0], v.blacks[1], v.whites[0], v.whites[1]);
        for (i, c) in six.into_iter().enumerate() {
            ansi[1 + i] = readable(c, bg, 3.0);
            ansi[9 + i] = readable(bright(c), bg, 3.0);
        }
        // (The pair at the ground's own end is drawn on other grounds: a
        // bright black still shows on it, as comments do.)
        let (near, far) = if light { (15, 7) } else { (8, 0) };
        ansi[near] = readable(ansi[near], bg, 3.0);
        let _ = far;
        // (The pair at the ink's end reads as text does.)
        for i in if light { [0, 8] } else { [7, 15] } {
            ansi[i] = readable(ansi[i], bg, 4.5);
        }
        let foreground = readable(v.ink, bg, 7.0);
        let mut selection = mix(bg, v.accent, if light { 0.24 } else { 0.34 });
        // (Text stays readable on it.)
        let mut t = 0.0;
        while contrast(foreground, selection) < 4.5 && t < 1.0 {
            t += 0.1;
            selection = mix(selection, bg, t);
        }
        // (Latte is the light of three: one terminal, one id.)
        let id = if v.name == LATTE.name { "catppuccin-latte".to_owned() } else { format!("{}-{}", self.id, if light { "light" } else { "dark" }) };
        Terminal { id, name: self.variant_name(mode), mode, foreground, background: bg, cursor: readable(v.tab, bg, 3.0), selection, ansi }
    }
}

/// Scheme `id`, if there is one.
pub fn get(id: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|s| s.id == id)
}

/// Catppuccin's Latte (v1.8.0): the light of each of its flavours.
const LATTE: Variant = Variant {
    name: "Latte",
    ground: hex(0xeff1f5),
    panel: hex(0xdce0e8),
    raised: hex(0xe6e9ef),
    ink: hex(0x4c4f69),
    muted: hex(0x7c7f93),
    accent: hex(0x1e66f5),
    tab: hex(0x8839ef),
    red: hex(0xd20f39),
    orange: hex(0xfe640b),
    yellow: hex(0xdf8e1d),
    green: hex(0x40a02b),
    teal: hex(0x179299),
    blue: hex(0x1e66f5),
    purple: hex(0x8839ef),
    pink: hex(0xea76cb),
    blacks: [hex(0x5c5f77), hex(0x6c6f85)],
    whites: [hex(0xacb0be), hex(0xbcc0cc)],
};

/// Every scheme, in the order Preferences shows them: Catppuccin's, then
/// the ten by how many have them installed.
pub const SCHEMES: &[Scheme] = &[
    Scheme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        about: "Catppuccin's darkest flavour, and Latte by day.",
        light: LATTE,
        dark: Variant {
            name: "Mocha",
            ground: hex(0x181825),
            panel: hex(0x1e1e2e),
            raised: hex(0x313244),
            ink: hex(0xcdd6f4),
            muted: hex(0x7f849c),
            accent: hex(0x89b4fa),
            tab: hex(0xcba6f7),
            red: hex(0xf38ba8),
            orange: hex(0xfab387),
            yellow: hex(0xf9e2af),
            green: hex(0xa6e3a1),
            teal: hex(0x94e2d5),
            blue: hex(0x89b4fa),
            purple: hex(0xcba6f7),
            pink: hex(0xf5c2e7),
            blacks: [hex(0x45475a), hex(0x585b70)],
            whites: [hex(0xbac2de), hex(0xa6adc8)],
        },
    },
    Scheme {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        about: "Catppuccin's middle flavour, and Latte by day.",
        light: LATTE,
        dark: Variant {
            name: "Macchiato",
            ground: hex(0x1e2030),
            panel: hex(0x24273a),
            raised: hex(0x363a4f),
            ink: hex(0xcad3f5),
            muted: hex(0x8087a2),
            accent: hex(0x8aadf4),
            tab: hex(0xc6a0f6),
            red: hex(0xed8796),
            orange: hex(0xf5a97f),
            yellow: hex(0xeed49f),
            green: hex(0xa6da95),
            teal: hex(0x8bd5ca),
            blue: hex(0x8aadf4),
            purple: hex(0xc6a0f6),
            pink: hex(0xf5bde6),
            blacks: [hex(0x494d64), hex(0x5b6078)],
            whites: [hex(0xb8c0e0), hex(0xa5adcb)],
        },
    },
    Scheme {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappé",
        about: "Catppuccin's softest dark flavour, and Latte by day.",
        light: LATTE,
        dark: Variant {
            name: "Frappé",
            ground: hex(0x292c3c),
            panel: hex(0x303446),
            raised: hex(0x414559),
            ink: hex(0xc6d0f5),
            muted: hex(0x838ba7),
            accent: hex(0x8caaee),
            tab: hex(0xca9ee6),
            red: hex(0xe78284),
            orange: hex(0xef9f76),
            yellow: hex(0xe5c890),
            green: hex(0xa6d189),
            teal: hex(0x81c8be),
            blue: hex(0x8caaee),
            purple: hex(0xca9ee6),
            pink: hex(0xf4b8e4),
            blacks: [hex(0x51576d), hex(0x626880)],
            whites: [hex(0xb5bfe2), hex(0xa5adce)],
        },
    },
    Scheme {
        id: "github",
        name: "GitHub",
        about: "GitHub's own, as on its site: Light and Dark.",
        light: Variant {
            name: "Light",
            ground: hex(0xffffff),
            panel: hex(0xf6f8fa),
            raised: hex(0xffffff),
            ink: hex(0x1f2328),
            muted: hex(0x6e7781),
            accent: hex(0x0969da),
            tab: hex(0x0969da),
            red: hex(0xcf222e),
            orange: hex(0xbc4c00),
            yellow: hex(0x9a6700),
            green: hex(0x1a7f37),
            teal: hex(0x1b7c83),
            blue: hex(0x0969da),
            purple: hex(0x8250df),
            pink: hex(0xbf3989),
            blacks: [hex(0x24292f), hex(0x57606a)],
            whites: [hex(0x6e7781), hex(0x8c959f)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x0d1117),
            panel: hex(0x161b22),
            raised: hex(0x21262d),
            ink: hex(0xe6edf3),
            muted: hex(0x8b949e),
            accent: hex(0x1f6feb),
            tab: hex(0x1f6feb),
            red: hex(0xff7b72),
            orange: hex(0xffa657),
            yellow: hex(0xd29922),
            green: hex(0x3fb950),
            teal: hex(0x39c5cf),
            blue: hex(0x58a6ff),
            purple: hex(0xbc8cff),
            pink: hex(0xf778ba),
            blacks: [hex(0x484f58), hex(0x6e7681)],
            whites: [hex(0xb1bac4), hex(0xf0f6fc)],
        },
    },
    Scheme {
        id: "one-dark",
        name: "One Dark Pro",
        about: "Atom's One Dark, and One Light by day.",
        light: Variant {
            name: "One Light",
            ground: hex(0xfafafa),
            panel: hex(0xeaeaeb),
            raised: hex(0xffffff),
            ink: hex(0x383a42),
            muted: hex(0xa0a1a7),
            accent: hex(0x4078f2),
            tab: hex(0x4078f2),
            red: hex(0xe45649),
            orange: hex(0xc18401),
            yellow: hex(0xb07a00),
            green: hex(0x50a14f),
            teal: hex(0x0184bc),
            blue: hex(0x4078f2),
            purple: hex(0xa626a4),
            pink: hex(0xca1243),
            blacks: [hex(0x383a42), hex(0x696c77)],
            whites: [hex(0xa0a1a7), hex(0xc5c5c8)],
        },
        dark: Variant {
            name: "One Dark",
            ground: hex(0x282c34),
            panel: hex(0x21252b),
            raised: hex(0x333842),
            ink: hex(0xabb2bf),
            muted: hex(0x5c6370),
            accent: hex(0x4d78cc),
            tab: hex(0x61afef),
            red: hex(0xe06c75),
            orange: hex(0xd19a66),
            yellow: hex(0xe5c07b),
            green: hex(0x98c379),
            teal: hex(0x56b6c2),
            blue: hex(0x61afef),
            purple: hex(0xc678dd),
            pink: hex(0xe06c9f),
            blacks: [hex(0x3f4451), hex(0x5c6370)],
            whites: [hex(0xabb2bf), hex(0xd7dae0)],
        },
    },
    Scheme {
        id: "dracula",
        name: "Dracula",
        about: "Dracula's purple night, and Alucard, its light of day.",
        light: Variant {
            name: "Alucard",
            ground: hex(0xfffbeb),
            panel: hex(0xefead3),
            raised: hex(0xfffdf3),
            ink: hex(0x1f1f1f),
            muted: hex(0x6c664b),
            accent: hex(0x644ac9),
            tab: hex(0x644ac9),
            red: hex(0xcb3a2a),
            orange: hex(0xa34d14),
            yellow: hex(0x846e15),
            green: hex(0x14710a),
            teal: hex(0x036a96),
            blue: hex(0x2a5fb8),
            purple: hex(0x644ac9),
            pink: hex(0xa3144d),
            blacks: [hex(0x1f1f1f), hex(0x6c664b)],
            whites: [hex(0x9d9882), hex(0xcfcab0)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x282a36),
            panel: hex(0x21222c),
            raised: hex(0x343746),
            ink: hex(0xf8f8f2),
            muted: hex(0x6272a4),
            accent: hex(0xbd93f9),
            tab: hex(0xbd93f9),
            red: hex(0xff5555),
            orange: hex(0xffb86c),
            yellow: hex(0xf1fa8c),
            green: hex(0x50fa7b),
            teal: hex(0x8be9fd),
            blue: hex(0x8aa9ff),
            purple: hex(0xbd93f9),
            pink: hex(0xff79c6),
            blacks: [hex(0x21222c), hex(0x6272a4)],
            whites: [hex(0xf8f8f2), hex(0xffffff)],
        },
    },
    Scheme {
        id: "ayu",
        name: "Ayu",
        about: "Ayu's deep ink and warm gold: Light and Dark.",
        light: Variant {
            name: "Light",
            ground: hex(0xfcfcfc),
            panel: hex(0xf0f1f2),
            raised: hex(0xffffff),
            ink: hex(0x5c6166),
            muted: hex(0x8a9199),
            accent: hex(0xffaa33),
            tab: hex(0xffaa33),
            red: hex(0xe65050),
            orange: hex(0xfa8d3e),
            yellow: hex(0xe6a117),
            green: hex(0x86b300),
            teal: hex(0x4cbf99),
            blue: hex(0x399ee6),
            purple: hex(0xa37acc),
            pink: hex(0xf07171),
            blacks: [hex(0x3d4247), hex(0x686f77)],
            whites: [hex(0xabb0b6), hex(0xd1d5da)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x0b0e14),
            panel: hex(0x11151c),
            raised: hex(0x1b212b),
            ink: hex(0xbfbdb6),
            muted: hex(0x636a76),
            accent: hex(0xe6b450),
            tab: hex(0xe6b450),
            red: hex(0xf26d78),
            orange: hex(0xff8f40),
            yellow: hex(0xffb454),
            green: hex(0xaad94c),
            teal: hex(0x95e6cb),
            blue: hex(0x59c2ff),
            purple: hex(0xd2a6ff),
            pink: hex(0xf07178),
            blacks: [hex(0x1b212b), hex(0x686868)],
            whites: [hex(0xc7c7c7), hex(0xffffff)],
        },
    },
    Scheme {
        id: "monokai",
        name: "Monokai",
        about: "Monokai's vivid six on olive black; by day on warm paper, Olo's own.",
        light: Variant {
            name: "Light",
            ground: hex(0xfaf9f2),
            panel: hex(0xecebe0),
            raised: hex(0xfffef8),
            ink: hex(0x272822),
            muted: hex(0x8e8a73),
            accent: hex(0xd81b60),
            tab: hex(0x7fb000),
            red: hex(0xd81b60),
            orange: hex(0xc96200),
            yellow: hex(0x9c8700),
            green: hex(0x5c8a00),
            teal: hex(0x0e8fa8),
            blue: hex(0x1c7fc4),
            purple: hex(0x7a52d6),
            pink: hex(0xd81b60),
            blacks: [hex(0x272822), hex(0x75715e)],
            whites: [hex(0xa8a48b), hex(0xd4d1bd)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x272822),
            panel: hex(0x1e1f1c),
            raised: hex(0x3a3b33),
            ink: hex(0xf8f8f2),
            muted: hex(0x88846f),
            accent: hex(0xf92672),
            tab: hex(0xa6e22e),
            red: hex(0xf92672),
            orange: hex(0xfd971f),
            yellow: hex(0xe6db74),
            green: hex(0xa6e22e),
            teal: hex(0x66d9ef),
            blue: hex(0x62a8f0),
            purple: hex(0xae81ff),
            pink: hex(0xf92672),
            blacks: [hex(0x1e1f1c), hex(0x75715e)],
            whites: [hex(0xf8f8f2), hex(0xffffff)],
        },
    },
    Scheme {
        id: "winter-is-coming",
        name: "Winter is Coming",
        about: "John Papa's midnight blue, and its light.",
        light: Variant {
            name: "Light",
            ground: hex(0xffffff),
            panel: hex(0xeef3f9),
            raised: hex(0xffffff),
            ink: hex(0x1d3b5c),
            muted: hex(0x6b7c93),
            accent: hex(0x236ebf),
            tab: hex(0x219fd5),
            red: hex(0xc4314b),
            orange: hex(0xb5560b),
            yellow: hex(0x9a6b00),
            green: hex(0x357b42),
            teal: hex(0x0991b6),
            blue: hex(0x236ebf),
            purple: hex(0x7a3fb5),
            pink: hex(0xa44185),
            blacks: [hex(0x174781), hex(0x4f6787)],
            whites: [hex(0x93a4ba), hex(0xc4d0de)],
        },
        dark: Variant {
            name: "Dark Blue",
            // (Its makers' ground is Night Owl's, #011627: a shade bluer
            // here, so the two are told apart.)
            ground: hex(0x03203b),
            panel: hex(0x092c4b),
            raised: hex(0x143e63),
            ink: hex(0xd6e6f5),
            muted: hex(0x7a8ea3),
            accent: hex(0x219fd5),
            tab: hex(0x219fd5),
            red: hex(0xf2637b),
            orange: hex(0xf7a35c),
            yellow: hex(0xf7ecb5),
            green: hex(0x8dec95),
            teal: hex(0x7fdbca),
            blue: hex(0x6dbdfa),
            purple: hex(0xd29ffc),
            pink: hex(0xff8ac5),
            blacks: [hex(0x0b2942), hex(0x5f7e97)],
            whites: [hex(0xa7dbf7), hex(0xffffff)],
        },
    },
    Scheme {
        id: "night-owl",
        name: "Night Owl",
        about: "Sarah Drasner's Night Owl, and Light Owl by day.",
        light: Variant {
            name: "Light Owl",
            ground: hex(0xfbfbfb),
            panel: hex(0xf0f0f0),
            raised: hex(0xffffff),
            ink: hex(0x403f53),
            muted: hex(0x8c93a6),
            accent: hex(0x2aa298),
            tab: hex(0x2aa298),
            red: hex(0xde3d3b),
            orange: hex(0xc96765),
            yellow: hex(0xc49000),
            green: hex(0x08916a),
            teal: hex(0x0c969b),
            blue: hex(0x288ed7),
            purple: hex(0x994cc3),
            pink: hex(0xd6438a),
            blacks: [hex(0x403f53), hex(0x6f6e85)],
            whites: [hex(0xa3a8b8), hex(0xd3d6de)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x011627),
            panel: hex(0x0b2942),
            raised: hex(0x173a59),
            ink: hex(0xd6deeb),
            muted: hex(0x637777),
            accent: hex(0x7e57c2),
            tab: hex(0x7e57c2),
            red: hex(0xef5350),
            orange: hex(0xf78c6c),
            yellow: hex(0xecc48d),
            green: hex(0xaddb67),
            teal: hex(0x7fdbca),
            blue: hex(0x82aaff),
            purple: hex(0xc792ea),
            pink: hex(0xff6ac1),
            blacks: [hex(0x010e1a), hex(0x575656)],
            whites: [hex(0xd6deeb), hex(0xffffff)],
        },
    },
    Scheme {
        id: "tokyo-night",
        name: "Tokyo Night",
        about: "Downtown Tokyo's lights at night, and its Light.",
        light: Variant {
            name: "Light",
            ground: hex(0xe6e7ed),
            panel: hex(0xd6d8df),
            raised: hex(0xeeeff4),
            ink: hex(0x343b58),
            muted: hex(0x6c6e75),
            accent: hex(0x2959aa),
            tab: hex(0x2959aa),
            red: hex(0x8c4351),
            orange: hex(0x965027),
            yellow: hex(0x8f5e15),
            green: hex(0x385f0d),
            teal: hex(0x166775),
            blue: hex(0x2959aa),
            purple: hex(0x5a3e8e),
            pink: hex(0x8f3f71),
            blacks: [hex(0x343b58), hex(0x565a6e)],
            whites: [hex(0x8b8fa3), hex(0xb7bac6)],
        },
        dark: Variant {
            name: "Night",
            ground: hex(0x1a1b26),
            panel: hex(0x16161e),
            raised: hex(0x292e42),
            ink: hex(0xc0caf5),
            muted: hex(0x565f89),
            accent: hex(0x3d59a1),
            tab: hex(0x7aa2f7),
            red: hex(0xf7768e),
            orange: hex(0xff9e64),
            yellow: hex(0xe0af68),
            green: hex(0x9ece6a),
            teal: hex(0x7dcfff),
            blue: hex(0x7aa2f7),
            purple: hex(0xbb9af7),
            pink: hex(0xff7ac6),
            blacks: [hex(0x15161e), hex(0x414868)],
            whites: [hex(0xa9b1d6), hex(0xc0caf5)],
        },
    },
    Scheme {
        id: "palenight",
        name: "Palenight",
        about: "Material's Palenight violet dusk; by day on Material's Lighter.",
        light: Variant {
            name: "Lighter",
            ground: hex(0xfafafa),
            panel: hex(0xeceff1),
            raised: hex(0xffffff),
            ink: hex(0x435761),
            muted: hex(0x90a4ae),
            accent: hex(0x7c4dff),
            tab: hex(0x7c4dff),
            red: hex(0xe53935),
            orange: hex(0xf76d47),
            yellow: hex(0xe2931d),
            green: hex(0x6f9a37),
            teal: hex(0x39adb5),
            blue: hex(0x6182b8),
            purple: hex(0x7c4dff),
            pink: hex(0xff5370),
            blacks: [hex(0x37474f), hex(0x607d8b)],
            whites: [hex(0xa7b6bd), hex(0xcfd8dc)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x292d3e),
            panel: hex(0x1f2233),
            raised: hex(0x34394f),
            ink: hex(0xbfc7d5),
            muted: hex(0x676e95),
            accent: hex(0x7e57c2),
            tab: hex(0xc792ea),
            red: hex(0xff5370),
            orange: hex(0xf78c6c),
            yellow: hex(0xffcb6b),
            green: hex(0xc3e88d),
            teal: hex(0x89ddff),
            blue: hex(0x82aaff),
            purple: hex(0xc792ea),
            pink: hex(0xff75b5),
            blacks: [hex(0x1b1e2b), hex(0x676e95)],
            whites: [hex(0xbfc7d5), hex(0xffffff)],
        },
    },
    Scheme {
        id: "synthwave-84",
        name: "SynthWave ’84",
        about: "Neon on a violet night; by day a soft dawn of the same, Olo's own.",
        light: Variant {
            name: "Dawn",
            ground: hex(0xfcf5fb),
            panel: hex(0xefe1f0),
            raised: hex(0xfffaff),
            ink: hex(0x2a2139),
            muted: hex(0x8478a8),
            accent: hex(0xc2189b),
            tab: hex(0xe0409f),
            red: hex(0xd3243f),
            orange: hex(0xc9532c),
            yellow: hex(0xa37a00),
            green: hex(0x0f8f62),
            teal: hex(0x008fa6),
            blue: hex(0x2f6fd0),
            purple: hex(0x7b47b5),
            pink: hex(0xc2189b),
            blacks: [hex(0x2a2139), hex(0x614d85)],
            whites: [hex(0xa99ac4), hex(0xd8cbe6)],
        },
        dark: Variant {
            name: "Dark",
            ground: hex(0x262335),
            panel: hex(0x241b2f),
            raised: hex(0x34294f),
            ink: hex(0xf4f0ff),
            muted: hex(0x848bbd),
            accent: hex(0xff7edb),
            tab: hex(0xff7edb),
            red: hex(0xfe4450),
            orange: hex(0xf97e72),
            yellow: hex(0xfede5d),
            green: hex(0x72f1b8),
            teal: hex(0x36f9f6),
            blue: hex(0x6fa8ff),
            purple: hex(0xb893ce),
            pink: hex(0xff7edb),
            blacks: [hex(0x1a1626), hex(0x614d85)],
            whites: [hex(0xdcd3f0), hex(0xffffff)],
        },
    },
];
