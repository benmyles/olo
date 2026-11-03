//! The Olo look: every colour the desktop draws with ([`Palette`]), in
//! whole looks ([`looks`]: each theme's own colours, BeOS's and
//! Catppuccin's, light and dark), BeOS's tint helpers and metrics.
//!
//! Both app_server (window tabs and frames) and the Interface Kit (every
//! widget) draw from [`Palette`], so a change of look changes everything
//! in lock-step; GTK, libadwaita and Chrome take it too ([`toolkit`]).
//! Colours are sRGB `[r, g, b, a]` bytes, written `#rrggbb` (or
//! `#rrggbbaa`) when a palette goes to agents as JSON.

use serde::{Deserialize, Serialize};

pub mod catppuccin;
pub mod looks;
pub mod toolkit;

pub type Rgba = [u8; 4];

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

/// `0xrrggbb` as an opaque colour.
pub const fn hex(h: u32) -> Rgba {
    [(h >> 16) as u8, (h >> 8) as u8, h as u8, 255]
}

pub const WHITE: Rgba = [255, 255, 255, 255];
pub const BLACK: Rgba = [0, 0, 0, 255];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// The named colours of a look, for telling things apart: graph lines,
/// chart slices, tab groups, file systems, syntax. Vivid enough to fill
/// with; as text, take [`Palette::hue_text`], which keeps them readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hue {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
    Brown,
    Gray,
}

impl Hue {
    pub const ALL: [Hue; 10] = [Hue::Red, Hue::Orange, Hue::Yellow, Hue::Green, Hue::Teal, Hue::Blue, Hue::Purple, Hue::Pink, Hue::Brown, Hue::Gray];

    /// Its name in menus ("Red").
    pub fn name(self) -> &'static str {
        match self {
            Hue::Red => "Red",
            Hue::Orange => "Orange",
            Hue::Yellow => "Yellow",
            Hue::Green => "Green",
            Hue::Teal => "Teal",
            Hue::Blue => "Blue",
            Hue::Purple => "Purple",
            Hue::Pink => "Pink",
            Hue::Brown => "Brown",
            Hue::Gray => "Gray",
        }
    }
}

/// A look's [`Hue`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hues {
    #[serde(with = "hex_color")]
    pub red: Rgba,
    #[serde(with = "hex_color")]
    pub orange: Rgba,
    #[serde(with = "hex_color")]
    pub yellow: Rgba,
    #[serde(with = "hex_color")]
    pub green: Rgba,
    #[serde(with = "hex_color")]
    pub teal: Rgba,
    #[serde(with = "hex_color")]
    pub blue: Rgba,
    #[serde(with = "hex_color")]
    pub purple: Rgba,
    #[serde(with = "hex_color")]
    pub pink: Rgba,
    #[serde(with = "hex_color")]
    pub brown: Rgba,
    #[serde(with = "hex_color")]
    pub gray: Rgba,
}

impl Hues {
    pub fn get(&self, hue: Hue) -> Rgba {
        match hue {
            Hue::Red => self.red,
            Hue::Orange => self.orange,
            Hue::Yellow => self.yellow,
            Hue::Green => self.green,
            Hue::Teal => self.teal,
            Hue::Blue => self.blue,
            Hue::Purple => self.purple,
            Hue::Pink => self.pink,
            Hue::Brown => self.brown,
            Hue::Gray => self.gray,
        }
    }
}

/// A terminal's colours: a look's own (`Palette::terminal`), or one of
/// the others Terminal offers ([`looks::terminals`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    /// What Terminal's settings keep (`bebox-light`, `beos-dark`,
    /// `latte`, …; see [`looks::terminal`]).
    pub id: String,
    /// Its name in menus ("BeBox Light", "Catppuccin Latte").
    pub name: String,
    /// The mode it is made for.
    pub mode: ThemeMode,
    #[serde(with = "hex_color")]
    pub foreground: Rgba,
    #[serde(with = "hex_color")]
    pub background: Rgba,
    #[serde(with = "hex_color")]
    pub cursor: Rgba,
    #[serde(with = "hex_color")]
    pub selection: Rgba,
    /// Black, red, green, yellow, blue, magenta, cyan, white; then bright.
    #[serde(with = "hex_colors")]
    pub ansi: [Rgba; 16],
}

/// Every colour the desktop draws with. Names follow the Haiku/BeOS
/// `color_which` constants where one exists. A palette is one look in
/// one mode: [`looks::palette`] makes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub mode: ThemeMode,
    /// The look it is: a theme's id ([`looks::THEMES`]) and its colour
    /// set's ([`looks::ColorSet::id`]).
    pub theme: String,
    pub colors: String,
    // Interface Kit (B_PANEL_BACKGROUND_COLOR and friends)
    #[serde(with = "hex_color")]
    pub panel_background: Rgba,
    #[serde(with = "hex_color")]
    pub panel_text: Rgba,
    #[serde(with = "hex_color")]
    pub document_background: Rgba,
    #[serde(with = "hex_color")]
    pub document_text: Rgba,
    #[serde(with = "hex_color")]
    pub control_background: Rgba,
    #[serde(with = "hex_color")]
    pub control_text: Rgba,
    #[serde(with = "hex_color")]
    pub control_border: Rgba,
    #[serde(with = "hex_color")]
    pub control_highlight: Rgba,
    #[serde(with = "hex_color")]
    pub control_mark: Rgba,
    #[serde(with = "hex_color")]
    pub navigation_base: Rgba,
    #[serde(with = "hex_color")]
    pub navigation_pulse: Rgba,
    #[serde(with = "hex_color")]
    pub shine: Rgba,
    #[serde(with = "hex_color")]
    pub shadow: Rgba,
    #[serde(with = "hex_color")]
    pub menu_background: Rgba,
    #[serde(with = "hex_color")]
    pub menu_selected_background: Rgba,
    #[serde(with = "hex_color")]
    pub menu_item_text: Rgba,
    #[serde(with = "hex_color")]
    pub menu_selected_item_text: Rgba,
    #[serde(with = "hex_color")]
    pub menu_selected_border: Rgba,
    #[serde(with = "hex_color")]
    pub tooltip_background: Rgba,
    #[serde(with = "hex_color")]
    pub tooltip_text: Rgba,
    #[serde(with = "hex_color")]
    pub list_background: Rgba,
    #[serde(with = "hex_color")]
    pub list_selected_background: Rgba,
    #[serde(with = "hex_color")]
    pub list_item_text: Rgba,
    #[serde(with = "hex_color")]
    pub list_selected_item_text: Rgba,
    #[serde(with = "hex_color")]
    pub success: Rgba,
    #[serde(with = "hex_color")]
    pub failure: Rgba,
    /// Cautions (Logs' warnings, a weak password): readable on documents.
    #[serde(with = "hex_color")]
    pub warning: Rgba,
    #[serde(with = "hex_color")]
    pub keyboard_navigation: Rgba,
    // Desktop
    #[serde(with = "hex_color")]
    pub desktop_text: Rgba,
    #[serde(with = "hex_color")]
    pub desktop_text_shadow: Rgba,
    #[serde(with = "hex_color")]
    pub status_bar: Rgba,
    // Depth and overlays
    /// Laid over a picture or the screen to set something on it (a
    /// screenshot's region, a viewer's controls); translucent.
    #[serde(with = "hex_color")]
    pub scrim: Rgba,
    /// A soft shadow under something lifted (a slider's thumb, a popup);
    /// translucent.
    #[serde(with = "hex_color")]
    pub drop_shadow: Rgba,
    /// The light along the top of a coloured fill (a progress bar, a
    /// colour well); translucent, and clear in flat looks.
    #[serde(with = "hex_color")]
    pub gloss: Rgba,
    // Window decorator (app_server)
    #[serde(with = "hex_color")]
    pub tab_focused: Rgba,
    #[serde(with = "hex_color")]
    pub tab_focused_light: Rgba,
    #[serde(with = "hex_color")]
    pub tab_focused_shadow: Rgba,
    #[serde(with = "hex_color")]
    pub tab_unfocused: Rgba,
    #[serde(with = "hex_color")]
    pub tab_unfocused_light: Rgba,
    #[serde(with = "hex_color")]
    pub tab_unfocused_shadow: Rgba,
    #[serde(with = "hex_color")]
    pub tab_text_focused: Rgba,
    #[serde(with = "hex_color")]
    pub tab_text_unfocused: Rgba,
    /// Frame bevel, outermost line first. Index 0..5.
    #[serde(with = "hex_colors")]
    pub frame_focused: [Rgba; 6],
    #[serde(with = "hex_colors")]
    pub frame_unfocused: [Rgba; 6],
    /// Colours for telling things apart.
    pub hues: Hues,
    /// Terminal's colours in this look.
    pub terminal: Terminal,
}

/// Olo's hues on light grounds: vivid enough for fills and lines.
pub const HUES_LIGHT: Hues = Hues {
    red: hex(0xd63b32),
    orange: hex(0xe88018),
    yellow: hex(0xdeb014),
    green: hex(0x34a048),
    teal: hex(0x1896a0),
    blue: hex(0x2c6cd6),
    purple: hex(0x9250c4),
    pink: hex(0xd64a96),
    brown: hex(0xa0643a),
    gray: hex(0x7a7c82),
};

/// Olo's hues on dark grounds.
pub const HUES_DARK: Hues = Hues {
    red: hex(0xe85c52),
    orange: hex(0xf09840),
    yellow: hex(0xecc440),
    green: hex(0x54ba62),
    teal: hex(0x3ab4be),
    blue: hex(0x548cec),
    purple: hex(0xae70de),
    pink: hex(0xe670b0),
    brown: hex(0xc08454),
    gray: hex(0x96989e),
};

/// Olo's ANSI colours for terminals on light grounds: every one of the
/// 16 reads clearly on a paper white (no neon green on black).
pub const ANSI_LIGHT: [Rgba; 16] = [
    hex(0x3b3f45),
    hex(0xc4314b),
    hex(0x2f8a3e),
    hex(0x9a6a00),
    hex(0x2f6fdb),
    hex(0x9d3fb5),
    hex(0x0f8494),
    hex(0x9a9ca2),
    hex(0x6a6e75),
    hex(0xe0475f),
    hex(0x3aa34d),
    hex(0xb98400),
    hex(0x4b8af0),
    hex(0xb85bd0),
    hex(0x13939f),
    hex(0xd4d5d8),
];

/// Olo's ANSI colours on dark grounds.
pub const ANSI_DARK: [Rgba; 16] = [
    hex(0x2c3037),
    hex(0xec6a78),
    hex(0x8fcf7d),
    hex(0xe8c17a),
    hex(0x6aaef5),
    hex(0xc98ae6),
    hex(0x5cc4cf),
    hex(0xb4b8bf),
    hex(0x5f6570),
    hex(0xff8592),
    hex(0xa9e395),
    hex(0xf7d595),
    hex(0x8cc2ff),
    hex(0xdca5f2),
    hex(0x7ad6df),
    hex(0xeceef1),
];

impl Palette {
    /// `rgba` tinted as `tint_color` does, darkening further in dark mode
    /// (where a tint's step shows less): how the kit draws its etched
    /// lines, `shade(panel_background, 1.22)`.
    pub fn shade(&self, rgba: Rgba, tint: f32) -> Rgba {
        let t = if self.mode == ThemeMode::Dark && tint > 1.0 { 1.0 + (tint - 1.0) * 1.9 } else { tint };
        self::tint(rgba, t)
    }

    /// The hairline round a selection plate (the kit's
    /// `style::selection_plate`, Twitcher's chosen app): the list
    /// selection's ground taken towards the highlight colour, so the
    /// plate's shape stays crisp on any panel.
    pub fn selection_edge(&self) -> Rgba {
        mix(self.list_selected_background, self.control_highlight, 0.6)
    }

    /// BeOS R5 as shipped (the BeOS colours of every theme, in light
    /// mode), with a paper-white terminal.
    pub fn light() -> Palette {
        Palette {
            mode: ThemeMode::Light,
            theme: looks::DEFAULT.into(),
            colors: looks::ColorSet::Beos.id().into(),
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
            warning: rgb(150, 100, 0),
            keyboard_navigation: rgb(0, 0, 229),
            desktop_text: rgb(255, 255, 255),
            desktop_text_shadow: [0, 0, 0, 160],
            status_bar: rgb(50, 150, 255),
            scrim: [0, 0, 0, 100],
            drop_shadow: [0, 0, 0, 40],
            gloss: [255, 255, 255, 70],
            tab_focused: rgb(255, 203, 0),
            tab_focused_light: rgb(255, 230, 128),
            tab_focused_shadow: rgb(176, 136, 0),
            tab_unfocused: rgb(232, 232, 232),
            tab_unfocused_light: rgb(250, 250, 250),
            tab_unfocused_shadow: rgb(168, 168, 168),
            tab_text_focused: rgb(0, 0, 0),
            tab_text_unfocused: rgb(80, 80, 80),
            frame_focused: [rgb(108, 108, 108), rgb(255, 255, 255), rgb(216, 216, 216), rgb(216, 216, 216), rgb(152, 152, 152), rgb(108, 108, 108)],
            frame_unfocused: [rgb(136, 136, 136), rgb(250, 250, 250), rgb(224, 224, 224), rgb(224, 224, 224), rgb(168, 168, 168), rgb(136, 136, 136)],
            hues: HUES_LIGHT,
            terminal: Terminal {
                id: "beos-light".into(),
                name: "BeOS Light".into(),
                mode: ThemeMode::Light,
                foreground: hex(0x26292e),
                background: hex(0xfbfaf6),
                cursor: hex(0x2f6fdb),
                selection: hex(0xc9dcf7),
                ansi: ANSI_LIGHT,
            },
        }
    }

    /// The same design language at night: dark panels, the yellow tab kept
    /// (it is the identity), slightly deepened so it doesn't glare.
    pub fn dark() -> Palette {
        Palette {
            mode: ThemeMode::Dark,
            theme: looks::DEFAULT.into(),
            colors: looks::ColorSet::Beos.id().into(),
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
            warning: rgb(250, 208, 70),
            keyboard_navigation: rgb(96, 144, 255),
            desktop_text: rgb(240, 240, 240),
            desktop_text_shadow: [0, 0, 0, 200],
            status_bar: rgb(60, 140, 230),
            scrim: [0, 0, 0, 140],
            drop_shadow: [0, 0, 0, 90],
            gloss: [255, 255, 255, 40],
            tab_focused: rgb(232, 180, 0),
            tab_focused_light: rgb(250, 208, 70),
            tab_focused_shadow: rgb(140, 104, 0),
            tab_unfocused: rgb(70, 70, 70),
            tab_unfocused_light: rgb(96, 96, 96),
            tab_unfocused_shadow: rgb(36, 36, 36),
            tab_text_focused: rgb(0, 0, 0),
            tab_text_unfocused: rgb(190, 190, 190),
            frame_focused: [rgb(10, 10, 10), rgb(96, 96, 96), rgb(58, 58, 58), rgb(58, 58, 58), rgb(30, 30, 30), rgb(10, 10, 10)],
            frame_unfocused: [rgb(16, 16, 16), rgb(80, 80, 80), rgb(50, 50, 50), rgb(50, 50, 50), rgb(32, 32, 32), rgb(16, 16, 16)],
            hues: HUES_DARK,
            terminal: Terminal {
                id: "beos-dark".into(),
                name: "BeOS Dark".into(),
                mode: ThemeMode::Dark,
                foreground: hex(0xd9dce1),
                background: hex(0x1d2025),
                cursor: hex(0x7cb4ff),
                selection: hex(0x2f405a),
                ansi: ANSI_DARK,
            },
        }
    }

    pub fn is_dark(&self) -> bool {
        self.mode == ThemeMode::Dark
    }

    pub fn hue(&self, hue: Hue) -> Rgba {
        self.hues.get(hue)
    }

    /// `hue` as text on documents: as given where it reads (4.5:1),
    /// else darkened (light mode) or lightened (dark) until it does.
    pub fn hue_text(&self, hue: Hue) -> Rgba {
        readable(self.hue(hue), self.document_background, 4.5)
    }

    /// The colour of a token class on document backgrounds: the look's
    /// hues (each clears 4.5:1 against `document_background`), comments a
    /// step toward the background.
    pub fn syntax(&self, s: Syntax) -> Rgba {
        let hue = match s {
            Syntax::Plain => return self.document_text,
            Syntax::Comment => return readable(mix(self.document_text, self.document_background, 0.45), self.document_background, 4.5),
            Syntax::String => Hue::Green,
            Syntax::Keyword => Hue::Purple,
            Syntax::Number | Syntax::Attribute => Hue::Orange,
            Syntax::Type => Hue::Teal,
            Syntax::Function | Syntax::Tag | Syntax::Link | Syntax::Heading => Hue::Blue,
            Syntax::Constant => Hue::Red,
        };
        self.hue_text(hue)
    }

    /// Text set on `fill` (a hue, a chip): the look's dark ink or a
    /// white, whichever reads better.
    pub fn ink_on(&self, fill: Rgba) -> Rgba {
        let dark_ink = if self.is_dark() { self.document_background } else { self.document_text };
        let light_ink = if self.is_dark() { self.document_text } else { self.document_background };
        let (dark_ink, light_ink) = (mix(dark_ink, BLACK, 0.5), mix(light_ink, WHITE, 0.5));
        if contrast(dark_ink, fill) >= contrast(light_ink, fill) { dark_ink } else { light_ink }
    }
}

/// `#rrggbb`, or `#rrggbbaa` if not opaque.
pub fn to_hex(c: Rgba) -> String {
    if c[3] == 255 { format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]) }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` optional).
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_start_matches('#');
    let n = |i: usize, w: usize| u8::from_str_radix(s.get(i..i + w)?, 16).ok();
    match s.len() {
        3 => Some([n(0, 1)? * 17, n(1, 1)? * 17, n(2, 1)? * 17, 255]),
        6 => Some([n(0, 2)?, n(2, 2)?, n(4, 2)?, 255]),
        8 => Some([n(0, 2)?, n(2, 2)?, n(4, 2)?, n(6, 2)?]),
        _ => None,
    }
}

/// Serde for a colour as `#rrggbb`.
pub mod hex_color {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(c: &super::Rgba, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(*c))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<super::Rgba, D::Error> {
        let t = String::deserialize(d)?;
        super::parse_hex(&t).ok_or_else(|| D::Error::custom(format!("not a colour: {t:?}")))
    }
}

/// Serde for a fixed row of colours, each `#rrggbb`.
pub mod hex_colors {
    use serde::{Deserialize, Deserializer, Serializer, de::Error, ser::SerializeSeq};

    pub fn serialize<S: Serializer, const N: usize>(cs: &[super::Rgba; N], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(N))?;
        for c in cs {
            seq.serialize_element(&super::to_hex(*c))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[super::Rgba; N], D::Error> {
        let ts = Vec::<String>::deserialize(d)?;
        let cs = ts.iter().map(|t| super::parse_hex(t).ok_or_else(|| D::Error::custom(format!("not a colour: {t:?}")))).collect::<Result<Vec<_>, _>>()?;
        cs.try_into().map_err(|v: Vec<_>| D::Error::custom(format!("{} colours, not {N}", v.len())))
    }
}

/// WCAG relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Rgba) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `fg`, or the least darkening (on light `bg`) or lightening (on dark)
/// of it that reaches `ratio` against `bg`.
pub fn readable(fg: Rgba, bg: Rgba, ratio: f32) -> Rgba {
    if contrast(fg, bg) >= ratio {
        return fg;
    }
    let toward = if luminance(bg) > 0.18 { BLACK } else { WHITE };
    (1..=20).map(|i| mix(fg, toward, i as f32 / 20.0)).find(|c| contrast(*c, bg) >= ratio).unwrap_or(toward)
}

/// Token classes for syntax colouring (Quick Look, the Viewer,
/// StyledEdit): [`Palette::syntax`] colours them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Plain,
    Comment,
    String,
    Keyword,
    Number,
    Type,
    Function,
    Constant,
    Tag,
    Attribute,
    Heading,
    Link,
}

/// Linear blend, BeOS `tint_color`-style helpers.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2]), l(a[3], b[3])]
}

/// `tint > 1` darkens, `< 1` lightens, as in Haiku's `tint_color()`
/// (B_DARKEN_1_TINT = 1.147, B_LIGHTEN_1_TINT = 0.590, ...).
pub fn tint(c: Rgba, tint: f32) -> Rgba {
    if tint >= 1.0 { mix(c, [0, 0, 0, c[3]], (tint - 1.0).min(1.0)) } else { mix(c, [255, 255, 255, c[3]], 1.0 - tint) }
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
    /// Noto Sans, shipped with Olo (data/fonts): a humanist sans as plain
    /// as BeOS's Swiss 721, open, clear at small sizes and covering most
    /// of the world's scripts. The rest are for a tree without it.
    pub const UI: &[&str] = &["Noto Sans", "Nimbus Sans", "Liberation Sans", "DejaVu Sans"];
    pub const UI_BOLD_WEIGHT: u16 = 700;
    /// Noto Sans Mono, shipped with Olo too: Noto Sans's monospaced
    /// sibling (BeOS used a Courier), with box drawing for Terminal.
    pub const MONO: &[&str] = &["Noto Sans Mono", "JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono"];
    /// Olo's faces for symbols Noto Sans doesn't have (arrows, ⌘ ⌥ ⇧ ⌃,
    /// ▸ and ✓), in the order they are tried.
    pub const SYMBOLS: &[&str] = &["Noto Sans Symbols", "Noto Sans Symbols 2", "Noto Sans Math"];
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
