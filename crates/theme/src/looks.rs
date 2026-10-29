//! Looks: a theme (its icons, wordmark and wallpaper, chosen in
//! Preferences ▸ Themes, `Appearance::icon_theme`) in one of its colour
//! sets (`Appearance::colors`), light or dark (`effective_theme`).
//! [`palette`] makes the [`Palette`] for one.
//!
//! Every theme has three colour sets, each light and dark:
//!
//! - **Its own** ([`ColorSet::Theme`]): colours drawn from the theme's
//!   pictures (BeBox's graphite and amber lights, Toybox's maple and
//!   painted yellow, Enamel's ivory, navy and gold), made by [`build`]
//!   from a few [`Seeds`] in BeOS's proportions.
//! - **BeOS** ([`ColorSet::Beos`]): R5's grey and yellow as shipped, and
//!   Olo's dark version of it; the same in every theme.
//! - **Catppuccin** ([`ColorSet::Catppuccin`]): Latte by day, Macchiato
//!   by night, the tab in the accent that suits the theme
//!   ([`Theme::catppuccin`]).
//!
//! Each look carries its terminal's colours (`Palette::terminal`), and
//! [`terminals`] lists what Terminal offers alongside them.

use crate::{
    ANSI_DARK, ANSI_LIGHT, BLACK, HUES_DARK, HUES_LIGHT, Hues, Palette, Rgba, Terminal, ThemeMode, WHITE,
    catppuccin::{self, Accent, Flavour},
    contrast, hex, mix, readable,
};

/// A theme: its id (its directory under `themes/`, and what the setting
/// holds), its name, a line about it, and its own colours.
#[derive(Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    /// A line about its own colours, for Preferences.
    pub colors_about: &'static str,
    pub light: Seeds,
    pub dark: Seeds,
    /// The Catppuccin accent its tab takes.
    pub catppuccin: Accent,
}

/// What a theme's own palette in one mode is made from; [`build`] works
/// out the rest (bevels, frames, tabs, menus, lists, the terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seeds {
    /// Panels (windows, toolbars, menus).
    pub panel: Rgba,
    /// Text, everywhere.
    pub ink: Rgba,
    /// Documents and lists (and the terminal's ground).
    pub document: Rgba,
    /// Buttons and other controls' faces.
    pub control: Rgba,
    /// Chosen things: a menu's item, a default button, selections.
    pub highlight: Rgba,
    pub on_highlight: Rgba,
    /// Keyboard focus, links, the text cursor.
    pub navigation: Rgba,
    /// The focused window's tab: the look's identity.
    pub tab: Rgba,
    pub on_tab: Rgba,
    pub tooltip: Rgba,
    /// Progress bars.
    pub status: Rgba,
    pub success: Rgba,
    pub failure: Rgba,
    pub warning: Rgba,
    pub hues: Hues,
    /// Controls' outlines, if not a shade of the panel (Halftone's ink).
    pub border: Option<Rgba>,
    /// The gloss along the top of coloured fills: 0 for flat looks.
    pub gloss: u8,
}

/// Defaults for a light theme's seeds (BeOS's own, a little modernised).
const LIGHT: Seeds = Seeds {
    panel: hex(0xd8d8d8),
    ink: hex(0x000000),
    document: hex(0xffffff),
    control: hex(0xf5f5f5),
    highlight: hex(0x2f6fd6),
    on_highlight: WHITE,
    navigation: hex(0x2a62c9),
    tab: hex(0xffcb00),
    on_tab: hex(0x000000),
    tooltip: hex(0xfff6cc),
    status: hex(0x3d7be0),
    success: hex(0x23883a),
    failure: hex(0xcc2b2b),
    warning: hex(0x966400),
    hues: HUES_LIGHT,
    border: None,
    gloss: 70,
};

const DARK: Seeds = Seeds {
    panel: hex(0x2b2b2b),
    ink: hex(0xe0e0e0),
    document: hex(0x1c1c1c),
    control: hex(0x3a3a3a),
    highlight: hex(0x3570d0),
    on_highlight: WHITE,
    navigation: hex(0x6aa0ff),
    tab: hex(0xe8b400),
    on_tab: hex(0x141414),
    tooltip: hex(0x46463a),
    status: hex(0x4a88ec),
    success: hex(0x5cc46c),
    failure: hex(0xff6b63),
    warning: hex(0xf0c040),
    hues: HUES_DARK,
    border: None,
    gloss: 40,
};

/// Process inks, for Halftone.
const PROCESS_LIGHT: Hues = Hues {
    red: hex(0xe2231a),
    orange: hex(0xf07d00),
    yellow: hex(0xf5c400),
    green: hex(0x009a44),
    teal: hex(0x0098a6),
    blue: hex(0x0066cc),
    purple: hex(0x7d3c98),
    pink: hex(0xe6007e),
    brown: hex(0x8b5a2b),
    gray: hex(0x6d6e71),
};

const PROCESS_DARK: Hues = Hues {
    red: hex(0xff4a3d),
    orange: hex(0xff9a2e),
    yellow: hex(0xffd21f),
    green: hex(0x2fc46a),
    teal: hex(0x21b8c4),
    blue: hex(0x3d8bff),
    purple: hex(0xb06ad6),
    pink: hex(0xff4fa8),
    brown: hex(0xc4874e),
    gray: hex(0x9a9ba0),
};

/// Every theme, in the order Preferences shows them.
pub const THEMES: &[Theme] = &[
    Theme {
        id: "bebox",
        name: "BeBox",
        about: "Graphite and blinking lights.",
        colors_about: "Graphite greys, Be blue and an amber tab.",
        light: Seeds {
            panel: hex(0xd5d8dd),
            ink: hex(0x1c1f24),
            document: hex(0xf8f9fb),
            control: hex(0xeceef2),
            tab: hex(0xf5c400),
            on_tab: hex(0x1c1f24),
            tooltip: hex(0xfdf4cc),
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x2a2d32),
            ink: hex(0xe1e4e9),
            document: hex(0x1b1d21),
            control: hex(0x393d43),
            tooltip: hex(0x3c3a2e),
            ..DARK
        },
        catppuccin: Accent::Peach,
    },
    Theme {
        id: "soft-be",
        name: "Soft Be",
        about: "Soft matte clay on the Be desk.",
        colors_about: "Putty, cornflower and butter yellow.",
        light: Seeds {
            panel: hex(0xe3e0d7),
            ink: hex(0x2b2a33),
            document: hex(0xfbfaf5),
            control: hex(0xf2f0e9),
            highlight: hex(0x4570c4),
            navigation: hex(0x3d6cc4),
            tab: hex(0xf2cd5c),
            on_tab: hex(0x2b2a33),
            tooltip: hex(0xfdf3d2),
            status: hex(0x6b94de),
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x2b2f3e),
            ink: hex(0xe5e5ee),
            document: hex(0x212431),
            control: hex(0x393e51),
            highlight: hex(0x4a6fc4),
            navigation: hex(0x86a8f0),
            tab: hex(0xdcb84e),
            on_tab: hex(0x1d1d24),
            tooltip: hex(0x3c3a33),
            status: hex(0x6f95e8),
            ..DARK
        },
        catppuccin: Accent::Green,
    },
    Theme {
        id: "toybox",
        name: "Toybox",
        about: "Painted wooden toys.",
        colors_about: "Maple, cream and painted yellow and blue.",
        light: Seeds {
            panel: hex(0xe6d9c3),
            ink: hex(0x2d241c),
            document: hex(0xfdf9f1),
            control: hex(0xf6efe3),
            highlight: hex(0x2f66c4),
            navigation: hex(0x2a5fbf),
            tab: hex(0xf2bd1c),
            on_tab: hex(0x2d241c),
            tooltip: hex(0xfff1c2),
            status: hex(0x3577d6),
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x342b24),
            ink: hex(0xefe6da),
            document: hex(0x251f1a),
            control: hex(0x43382f),
            highlight: hex(0x3f6fc8),
            navigation: hex(0x78a6f2),
            tab: hex(0xe3ab18),
            on_tab: hex(0x1f1a14),
            tooltip: hex(0x46392a),
            status: hex(0x4f86e0),
            ..DARK
        },
        catppuccin: Accent::Red,
    },
    Theme {
        id: "papercraft",
        name: "Papercraft",
        about: "Folded card, one tone a face.",
        colors_about: "Sage card stock, teal ink and a kraft tab; flat, no gloss.",
        light: Seeds {
            panel: hex(0xdbe2d2),
            ink: hex(0x232d27),
            document: hex(0xfafbf5),
            control: hex(0xeef2e7),
            highlight: hex(0x2a7a71),
            navigation: hex(0x23716a),
            tab: hex(0xe0b45e),
            on_tab: hex(0x232d27),
            tooltip: hex(0xf8eed0),
            status: hex(0x34978b),
            gloss: 0,
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x26302a),
            ink: hex(0xe2e9e1),
            document: hex(0x1a211d),
            control: hex(0x343f38),
            highlight: hex(0x2c8579),
            navigation: hex(0x7accbf),
            tab: hex(0xcaa252),
            on_tab: hex(0x1b1d1b),
            tooltip: hex(0x37382a),
            status: hex(0x3fa596),
            gloss: 0,
            ..DARK
        },
        catppuccin: Accent::Teal,
    },
    Theme {
        id: "enamel",
        name: "Enamel",
        about: "Enamel pins in polished gold.",
        colors_about: "Gilt ivory, navy enamel and gold lettering.",
        light: Seeds {
            panel: hex(0xe8dfc3),
            ink: hex(0x1b2238),
            document: hex(0xfcf9ef),
            control: hex(0xf6f0df),
            highlight: hex(0x27458f),
            on_highlight: hex(0xfbf6e6),
            navigation: hex(0x2a4fa8),
            tab: hex(0x22397a),
            on_tab: hex(0xf2d27c),
            tooltip: hex(0xfbefcb),
            status: hex(0x2f56b0),
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x1a2148),
            ink: hex(0xece5d4),
            document: hex(0x121735),
            control: hex(0x283160),
            highlight: hex(0x3a5cc0),
            on_highlight: hex(0xfbf6e6),
            navigation: hex(0x8aa6f0),
            tab: hex(0xd4a12a),
            on_tab: hex(0x161616),
            tooltip: hex(0x3a3524),
            status: hex(0x4a6fd0),
            ..DARK
        },
        catppuccin: Accent::Yellow,
    },
    Theme {
        id: "halftone",
        name: "Halftone",
        about: "Flat ink and halftone dots.",
        colors_about: "Newsprint, black ink and process colours; flat.",
        light: Seeds {
            panel: hex(0xf2efe7),
            ink: hex(0x121212),
            document: hex(0xfffdf6),
            control: hex(0xfaf7ee),
            highlight: hex(0x1a66d6),
            navigation: hex(0x1a5fd0),
            tab: hex(0xffd21f),
            on_tab: hex(0x121212),
            tooltip: hex(0xfff4b0),
            status: hex(0x1a66d6),
            hues: PROCESS_LIGHT,
            border: Some(hex(0x1c1c1c)),
            gloss: 0,
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x1d1d1d),
            ink: hex(0xf3f0e6),
            document: hex(0x121212),
            control: hex(0x2c2c2c),
            highlight: hex(0x2d6fd8),
            navigation: hex(0x5b9bff),
            tab: hex(0xf5c800),
            on_tab: hex(0x111111),
            tooltip: hex(0x3a3720),
            status: hex(0x2d74e0),
            hues: PROCESS_DARK,
            gloss: 0,
            ..DARK
        },
        catppuccin: Accent::Pink,
    },
    Theme {
        id: CLASSIC,
        name: "Classic",
        about: "Olo's first set: glossy, bright.",
        colors_about: "Glossy sky grey by day and ocean blue by night.",
        light: Seeds {
            panel: hex(0xdbe3ee),
            ink: hex(0x131a26),
            document: hex(0xffffff),
            control: hex(0xf3f6fb),
            highlight: hex(0x1d6ae0),
            navigation: hex(0x1a62d6),
            tab: hex(0xffcb12),
            on_tab: hex(0x131a26),
            tooltip: hex(0xfff6c8),
            status: hex(0x2b86f5),
            gloss: 110,
            ..LIGHT
        },
        dark: Seeds {
            panel: hex(0x182b36),
            ink: hex(0xe7edf7),
            document: hex(0x0f1e27),
            control: hex(0x24394a),
            highlight: hex(0x2c6fe0),
            navigation: hex(0x6aa5ff),
            tab: hex(0xf5c000),
            on_tab: hex(0x121212),
            tooltip: hex(0x3a3825),
            status: hex(0x3d86f5),
            gloss: 60,
            ..DARK
        },
        catppuccin: Accent::Sky,
    },
];

/// The theme a new user gets (and an unknown setting falls back to).
pub const DEFAULT: &str = "bebox";

/// The theme whose icons are the set in `<data>/icons` itself.
pub const CLASSIC: &str = "classic";

/// Theme `id`, if there is one.
pub fn get(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id == id)
}

/// Theme `id`, or the default if there is none by that name.
pub fn resolve(id: &str) -> &'static Theme {
    get(id).or_else(|| get(DEFAULT)).expect("the default theme")
}

/// A theme's colour sets (`Appearance::colors` holds the id).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ColorSet {
    /// The theme's own.
    #[default]
    Theme,
    Beos,
    Catppuccin,
}

impl ColorSet {
    pub const ALL: [ColorSet; 3] = [ColorSet::Theme, ColorSet::Beos, ColorSet::Catppuccin];

    /// What the setting holds.
    pub fn id(self) -> &'static str {
        match self {
            ColorSet::Theme => "theme",
            ColorSet::Beos => "beos",
            ColorSet::Catppuccin => "catppuccin",
        }
    }

    /// The set `id` names; unknown ids are the theme's own.
    pub fn resolve(id: &str) -> ColorSet {
        ColorSet::ALL.into_iter().find(|c| c.id() == id).unwrap_or_default()
    }

    /// Its name in `theme`: the theme's for its own.
    pub fn name(self, theme: &Theme) -> &'static str {
        match self {
            ColorSet::Theme => theme.name,
            ColorSet::Beos => "BeOS",
            ColorSet::Catppuccin => "Catppuccin",
        }
    }

    /// A line about it in `theme`.
    pub fn about(self, theme: &Theme) -> String {
        match self {
            ColorSet::Theme => theme.colors_about.to_owned(),
            ColorSet::Beos => "BeOS R5's grey panels and yellow tab, and a dark version.".to_owned(),
            ColorSet::Catppuccin => format!("Latte by day and Macchiato by night, with a {} tab.", theme.catppuccin.name()),
        }
    }
}

/// The palette for theme `theme` (an id; unknown ones are the default)
/// in colour set `colors` (an id; unknown ones are the theme's own), in
/// `mode`.
pub fn palette(theme: &str, colors: &str, mode: ThemeMode) -> Palette {
    let t = resolve(theme);
    let set = ColorSet::resolve(colors);
    let mut p = match (set, mode) {
        (ColorSet::Theme, ThemeMode::Light) => build(&t.light, ThemeMode::Light, own_terminal(t, ThemeMode::Light)),
        (ColorSet::Theme, ThemeMode::Dark) => build(&t.dark, ThemeMode::Dark, own_terminal(t, ThemeMode::Dark)),
        (ColorSet::Beos, ThemeMode::Light) => Palette::light(),
        (ColorSet::Beos, ThemeMode::Dark) => Palette::dark(),
        (ColorSet::Catppuccin, mode) => {
            let f = catppuccin_flavour(mode);
            build(&catppuccin_seeds(f, t.catppuccin), mode, f.terminal())
        }
    };
    (p.theme, p.colors) = (t.id.into(), set.id().into());
    p
}

/// The flavour the Catppuccin colours take in `mode`.
fn catppuccin_flavour(mode: ThemeMode) -> &'static Flavour {
    match mode {
        ThemeMode::Light => &catppuccin::LATTE,
        ThemeMode::Dark => &catppuccin::MACCHIATO,
    }
}

/// A Catppuccin flavour as seeds: Text on the flavour's ramp, Blue for
/// what is chosen, the theme's accent on the tab, its accents as hues.
fn catppuccin_seeds(f: &Flavour, accent: Accent) -> Seeds {
    let light = f.mode == ThemeMode::Light;
    let tab = f.accent(accent);
    let on_tab = if contrast(f.text, tab) >= contrast(f.crust, tab) { f.text } else { f.crust };
    Seeds {
        panel: if light { f.crust } else { f.base },
        ink: f.text,
        document: if light { f.base } else { f.mantle },
        control: if light { f.base } else { f.surface0 },
        highlight: f.blue,
        on_highlight: if light { f.base } else { f.crust },
        navigation: f.blue,
        tab,
        on_tab: readable(on_tab, tab, 4.5),
        tooltip: if light { f.mantle } else { f.surface0 },
        status: f.blue,
        success: f.green,
        failure: f.red,
        warning: f.yellow,
        hues: Hues {
            red: f.red,
            orange: f.peach,
            yellow: f.yellow,
            green: f.green,
            teal: f.teal,
            blue: f.blue,
            purple: f.mauve,
            pink: f.pink,
            brown: mix(f.peach, f.surface2, 0.45),
            gray: f.overlay1,
        },
        border: Some(if light { f.overlay1 } else { f.crust }),
        gloss: 0,
    }
}

/// A theme's own terminal: its ink on its document colour, Olo's ANSI
/// colours kept readable on it, the cursor in its focus colour.
fn own_terminal(t: &Theme, mode: ThemeMode) -> Terminal {
    let (s, ansi, ground, word) = match mode {
        ThemeMode::Light => (&t.light, ANSI_LIGHT, [7, 15], "Light"),
        ThemeMode::Dark => (&t.dark, ANSI_DARK, [0, 8], "Dark"),
    };
    let bg = s.document;
    let mut ansi = ansi;
    for (i, c) in ansi.iter_mut().enumerate() {
        if !ground.contains(&i) {
            *c = readable(*c, bg, 3.0);
        }
    }
    Terminal {
        id: format!("{}-{}", t.id, word.to_lowercase()),
        name: format!("{} {word}", t.name),
        mode,
        foreground: s.ink,
        background: bg,
        cursor: s.navigation,
        selection: mix(bg, s.highlight, if mode == ThemeMode::Dark { 0.35 } else { 0.25 }),
        ansi,
    }
}

/// The whole palette from `s`, in BeOS's proportions: R5's bevels,
/// frames and tab shading (`Palette::light`), and Olo's dark version of
/// them (`Palette::dark`), worked out from the seeds rather than greys.
/// [`palette`] fills in which look it is.
pub fn build(s: &Seeds, mode: ThemeMode, terminal: Terminal) -> Palette {
    let (p, ink) = (s.panel, s.ink);
    let readable_on_doc = |c: Rgba| readable(c, s.document, 4.5);
    let gloss = [255, 255, 255, s.gloss];
    match mode {
        ThemeMode::Light => Palette {
            mode,
            theme: String::new(),
            colors: String::new(),
            panel_background: p,
            panel_text: ink,
            document_background: s.document,
            document_text: ink,
            control_background: s.control,
            control_text: ink,
            control_border: s.border.unwrap_or(mix(p, ink, 0.56)),
            control_highlight: s.highlight,
            control_mark: ink,
            navigation_base: s.navigation,
            navigation_pulse: ink,
            shine: mix(p, WHITE, 0.85),
            shadow: mix(ink, BLACK, 0.5),
            menu_background: p,
            menu_selected_background: s.highlight,
            menu_item_text: ink,
            menu_selected_item_text: readable(s.on_highlight, s.highlight, 4.5),
            menu_selected_border: mix(s.highlight, ink, 0.35),
            tooltip_background: s.tooltip,
            tooltip_text: ink,
            list_background: s.document,
            list_selected_background: mix(s.document, s.highlight, 0.22),
            list_item_text: ink,
            list_selected_item_text: ink,
            success: readable_on_doc(s.success),
            failure: readable_on_doc(s.failure),
            warning: readable_on_doc(s.warning),
            keyboard_navigation: s.navigation,
            desktop_text: WHITE,
            desktop_text_shadow: [0, 0, 0, 160],
            status_bar: s.status,
            scrim: [0, 0, 0, 100],
            drop_shadow: [0, 0, 0, 40],
            gloss,
            tab_focused: s.tab,
            tab_focused_light: mix(s.tab, WHITE, 0.5),
            tab_focused_shadow: mix(s.tab, BLACK, 0.31),
            tab_unfocused: mix(p, WHITE, 0.41),
            tab_unfocused_light: mix(p, WHITE, 0.85),
            tab_unfocused_shadow: mix(p, ink, 0.22),
            tab_text_focused: readable(s.on_tab, s.tab, 4.5),
            tab_text_unfocused: readable(mix(ink, p, 0.37), mix(p, WHITE, 0.41), 4.5),
            frame_focused: [mix(p, ink, 0.5), mix(p, WHITE, 0.85), p, p, mix(p, ink, 0.3), mix(p, ink, 0.5)],
            frame_unfocused: [
                mix(p, ink, 0.37),
                mix(p, WHITE, 0.8),
                mix(p, WHITE, 0.2),
                mix(p, WHITE, 0.2),
                mix(p, ink, 0.22),
                mix(p, ink, 0.37),
            ],
            hues: s.hues,
            terminal,
        },
        ThemeMode::Dark => Palette {
            mode,
            theme: String::new(),
            colors: String::new(),
            panel_background: p,
            panel_text: ink,
            document_background: s.document,
            document_text: ink,
            control_background: s.control,
            control_text: ink,
            control_border: s.border.unwrap_or(mix(p, BLACK, 0.55)),
            control_highlight: s.highlight,
            control_mark: mix(ink, WHITE, 0.5),
            navigation_base: s.navigation,
            navigation_pulse: mix(ink, WHITE, 0.5),
            shine: mix(p, ink, 0.26),
            shadow: mix(p, BLACK, 0.9),
            menu_background: mix(p, ink, 0.03),
            menu_selected_background: s.highlight,
            menu_item_text: ink,
            menu_selected_item_text: readable(s.on_highlight, s.highlight, 4.5),
            menu_selected_border: mix(s.highlight, BLACK, 0.4),
            tooltip_background: s.tooltip,
            tooltip_text: mix(ink, WHITE, 0.4),
            list_background: s.document,
            list_selected_background: mix(s.document, s.highlight, 0.35),
            list_item_text: ink,
            list_selected_item_text: mix(ink, WHITE, 0.6),
            success: readable_on_doc(s.success),
            failure: readable_on_doc(s.failure),
            warning: readable_on_doc(s.warning),
            keyboard_navigation: s.navigation,
            desktop_text: mix(ink, WHITE, 0.5),
            desktop_text_shadow: [0, 0, 0, 200],
            status_bar: s.status,
            scrim: [0, 0, 0, 140],
            drop_shadow: [0, 0, 0, 90],
            gloss,
            tab_focused: s.tab,
            tab_focused_light: mix(s.tab, WHITE, 0.3),
            tab_focused_shadow: mix(s.tab, BLACK, 0.4),
            tab_unfocused: mix(p, ink, 0.15),
            tab_unfocused_light: mix(p, ink, 0.29),
            tab_unfocused_shadow: mix(p, BLACK, 0.16),
            tab_text_focused: readable(s.on_tab, s.tab, 4.5),
            tab_text_unfocused: readable(mix(ink, p, 0.18), mix(p, ink, 0.15), 4.5),
            frame_focused: [mix(p, BLACK, 0.77), mix(p, ink, 0.29), mix(p, ink, 0.08), mix(p, ink, 0.08), mix(p, BLACK, 0.3), mix(p, BLACK, 0.77)],
            frame_unfocused: [
                mix(p, BLACK, 0.63),
                mix(p, ink, 0.2),
                mix(p, ink, 0.04),
                mix(p, ink, 0.04),
                mix(p, BLACK, 0.26),
                mix(p, BLACK, 0.63),
            ],
            hues: s.hues,
            terminal,
        },
    }
}

/// The terminal colours Terminal offers in `mode` while theme `theme` is
/// on: the theme's own, BeOS's, and Catppuccin's (Latte and Frappé for
/// light mode, Frappé being the lightest of its darks; Macchiato and
/// Mocha for dark).
pub fn terminals(theme: &str, mode: ThemeMode) -> Vec<Terminal> {
    let t = resolve(theme);
    let mut all = vec![own_terminal(t, mode), palette(t.id, ColorSet::Beos.id(), mode).terminal];
    let flavours: [&Flavour; 2] = match mode {
        ThemeMode::Light => [&catppuccin::LATTE, &catppuccin::FRAPPE],
        ThemeMode::Dark => [&catppuccin::MACCHIATO, &catppuccin::MOCHA],
    };
    all.extend(flavours.iter().map(|f| f.terminal()));
    all
}

/// The terminal colours `id` names (any theme's own, `<theme>-light` or
/// `-dark`; `beos-light`, `beos-dark`; a Catppuccin flavour's id).
pub fn terminal(id: &str) -> Option<Terminal> {
    if let Some(f) = catppuccin::FLAVOURS.iter().find(|f| f.id == id) {
        return Some(f.terminal());
    }
    let (theme, mode) = id.rsplit_once('-')?;
    let mode = match mode {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => return None,
    };
    if theme == "beos" {
        return Some(palette(DEFAULT, ColorSet::Beos.id(), mode).terminal);
    }
    get(theme).map(|t| own_terminal(t, mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hue;

    fn every() -> impl Iterator<Item = (String, Palette)> {
        THEMES.iter().flat_map(|t| {
            ColorSet::ALL.into_iter().flat_map(move |c| {
                [ThemeMode::Light, ThemeMode::Dark].map(|m| (format!("{} {} {m:?}", t.id, c.id()), palette(t.id, c.id(), m)))
            })
        })
    }

    #[test]
    fn ids_are_unique_and_known() {
        for (i, t) in THEMES.iter().enumerate() {
            assert!(THEMES[..i].iter().all(|u| u.id != t.id), "{} twice", t.id);
        }
        assert!(get(DEFAULT).is_some() && get(CLASSIC).is_some());
        assert_eq!(resolve("no-such-theme").id, DEFAULT);
        assert_eq!(ColorSet::resolve("nope"), ColorSet::Theme);
        for c in ColorSet::ALL {
            assert_eq!(ColorSet::resolve(c.id()), c);
        }
    }

    /// The colours alone, not which look they are.
    fn colors_of(p: &Palette) -> Palette {
        Palette { theme: String::new(), colors: String::new(), ..p.clone() }
    }

    #[test]
    fn beos_is_r5_in_every_theme() {
        for t in THEMES {
            let p = palette(t.id, "beos", ThemeMode::Light);
            assert_eq!((p.theme.as_str(), p.colors.as_str()), (t.id, "beos"));
            assert_eq!(colors_of(&p), colors_of(&Palette::light()));
            assert_eq!(colors_of(&palette(t.id, "beos", ThemeMode::Dark)), colors_of(&Palette::dark()));
        }
        assert_eq!(palette("nope", "nope", ThemeMode::Light).colors, "theme");
        assert_eq!(Palette::light().panel_background, crate::rgb(216, 216, 216));
        assert_eq!(Palette::light().tab_focused, crate::rgb(255, 203, 0));
    }

    #[test]
    fn every_look_is_its_own() {
        // Each theme's own colours differ from every other look's, in
        // both modes, and each mode is what it says.
        let all: Vec<(String, Palette)> = every().collect();
        for (i, (a, p)) in all.iter().enumerate() {
            assert_eq!(p.mode == ThemeMode::Dark, crate::luminance(p.panel_background) < 0.18, "{a}");
            for (b, q) in &all[..i] {
                assert_ne!(p, q, "{a} is {b}");
                if !(a.contains(" beos ") && b.contains(" beos ")) && !(a.contains(" catppuccin ") && b.contains(" catppuccin ")) {
                    assert_ne!(colors_of(p), colors_of(q), "{a} is {b}");
                }
            }
        }
        // Catppuccin's are the same but for the tab.
        let (x, y) = (palette("bebox", "catppuccin", ThemeMode::Dark), palette("toybox", "catppuccin", ThemeMode::Dark));
        assert_ne!(x.tab_focused, y.tab_focused);
        assert_eq!((x.panel_background, x.terminal.id.as_str()), (y.panel_background, "macchiato"));
    }

    /// WCAG AA for text (4.5:1) wherever text is set, in every look.
    /// BeOS R5's menu selection (white on mid grey) is as it shipped.
    #[test]
    fn text_reads_everywhere() {
        for (look, p) in every() {
            let mut pairs = vec![
                ("panel", p.panel_text, p.panel_background),
                ("document", p.document_text, p.document_background),
                ("control", p.control_text, p.control_background),
                ("menu", p.menu_item_text, p.menu_background),
                ("tooltip", p.tooltip_text, p.tooltip_background),
                ("list", p.list_item_text, p.list_background),
                ("list selection", p.list_selected_item_text, p.list_selected_background),
                ("focused tab", p.tab_text_focused, p.tab_focused),
                ("unfocused tab", p.tab_text_unfocused, p.tab_unfocused),
            ];
            if !look.contains(" beos ") {
                pairs.push(("menu selection", p.menu_selected_item_text, p.menu_selected_background));
            }
            for h in Hue::ALL {
                pairs.push((h.name(), p.hue_text(h), p.document_background));
            }
            for (what, fg, bg) in pairs {
                assert!(contrast(fg, bg) >= 4.5, "{look}: {what} {:.2}", contrast(fg, bg));
            }
            for (what, c) in [("success", p.success), ("failure", p.failure), ("warning", p.warning)] {
                assert!(contrast(c, p.document_background) >= 3.0, "{look}: {what} {:.2}", contrast(c, p.document_background));
            }
            assert!(contrast(p.keyboard_navigation, p.panel_background) >= 2.5, "{look}: focus {:.2}", contrast(p.keyboard_navigation, p.panel_background));
        }
    }

    #[test]
    fn terminals_read() {
        let mut all: Vec<Terminal> = every().map(|(_, p)| p.terminal).collect();
        all.extend(THEMES.iter().flat_map(|t| [ThemeMode::Light, ThemeMode::Dark].map(|m| terminals(t.id, m)).concat()));
        for t in &all {
            assert!(contrast(t.foreground, t.background) >= 7.0, "{} text: {:.2}", t.name, contrast(t.foreground, t.background));
            assert!(contrast(t.foreground, t.selection) >= 4.5, "{} selection: {:.2}", t.name, contrast(t.foreground, t.selection));
            assert_ne!(t.cursor, t.background, "{}", t.name);
            // Olo's own: every colour but the one that matches the ground
            // reads (Catppuccin's are as its palette gives them).
            if !t.name.starts_with("Catppuccin") {
                let skip = if t.mode == ThemeMode::Light { [7, 15] } else { [0, 8] };
                for (i, c) in t.ansi.iter().enumerate().filter(|(i, _)| !skip.contains(i)) {
                    assert!(contrast(*c, t.background) >= 3.0, "{} colour {i}: {:.2}", t.name, contrast(*c, t.background));
                }
            }
            assert_eq!(terminal(&t.id).as_ref(), Some(t), "{}", t.id);
        }
        assert_eq!(terminal("nope"), None);
        assert_eq!(terminal("nope-light"), None);
    }

    #[test]
    fn terminal_choices() {
        let names = |m| terminals("toybox", m).into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(names(ThemeMode::Light), ["toybox-light", "beos-light", "latte", "frappe"]);
        assert_eq!(names(ThemeMode::Dark), ["toybox-dark", "beos-dark", "macchiato", "mocha"]);
        // The look's own terminal is among its mode's choices.
        for (look, p) in every() {
            let theme = look.split(' ').next().unwrap();
            assert!(terminals(theme, p.mode).contains(&p.terminal), "{look}");
        }
    }

    #[test]
    fn palettes_round_trip_as_json() {
        let p = palette("enamel", "theme", ThemeMode::Dark);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["panel_background"], "#1a2148");
        assert_eq!(json["desktop_text_shadow"], "#000000c8");
        assert_eq!(json["hues"]["red"], crate::to_hex(p.hues.red));
        assert_eq!(json["terminal"]["id"], "enamel-dark");
        assert_eq!(serde_json::from_value::<Palette>(json).unwrap(), p);
    }
}
