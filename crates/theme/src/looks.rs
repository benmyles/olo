//! Looks: the themes. A theme is the whole look, chosen in Preferences ▸
//! Themes (`Appearance::look`): its colours, light and dark
//! (`effective_theme`), its icons (one of the [`ICON_SETS`]) and its
//! wallpaper. [`palette`] makes the [`Palette`] for one.
//!
//! One list, [`THEMES`] (Ben, 2026-11-17: "the split between themes and
//! colors doesn't make a lot of sense. let's just have Classic, Platinum
//! and BeOS as their own themes (as well as all the other themes)"):
//!
//! - **Classic**: BeOS's look made glossy and bright; its colours are
//!   made by [`build`] from a few [`Seeds`] in BeOS's proportions.
//! - **Platinum**: Mac OS 9's, made the same way.
//! - **BeOS**: R5's grey and yellow as shipped ([`Palette::light`]), and
//!   Olo's dark version of it.
//! - **The schemes from elsewhere** ([`crate::schemes`]: Catppuccin,
//!   GitHub, Dracula, …), each a theme, with the classic icons.
//!
//! Anything about a theme can be changed ([`Custom`]: its colours, its
//! icons, its wallpaper; [`customized`]). Each look carries its
//! terminal's colours (`Palette::terminal`), and [`terminals`] lists what
//! Terminal offers alongside them.

use std::{collections::BTreeMap, sync::OnceLock};

use serde::{Deserialize, Serialize};

use crate::{
    ANSI_DARK, ANSI_LIGHT, BLACK, HUES_DARK, HUES_LIGHT, Hues, Palette, Rgba, Terminal, ThemeMode, WHITE, hex, mix, readable,
    schemes::{SCHEMES, Scheme},
};

/// A theme: its id (what the setting holds), its name, a line about it,
/// its colours, its icons and its wallpaper.
#[derive(Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    /// Its icons: one of the [`ICON_SETS`].
    pub icons: &'static str,
    /// Its wallpaper: the name of a picture in the wallpaper pack
    /// (`data/wallpapers`), the one of the pack that suits it best (one
    /// with a dark version beside it where there is one).
    pub wallpaper: &'static str,
    pub colors: Colors,
}

/// Where a theme's colours come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colors {
    /// Its own: made by [`build`] from its seeds.
    Own(&'static Pair),
    /// BeOS R5's, as shipped, and Olo's dark version of them.
    Beos,
    /// A scheme from elsewhere, tuned for Olo.
    Scheme(&'static Scheme),
}

/// A theme's seeds, light and dark.
#[derive(Debug, PartialEq, Eq)]
pub struct Pair {
    pub light: Seeds,
    pub dark: Seeds,
}

/// A set of icons: the drawings themselves (Classic's), or a set's own
/// colours and drawings over them (`data/themes/<id>/icons`; see
/// docs/ICONS.md, "A theme's colours").
#[derive(Debug, PartialEq, Eq)]
pub struct IconSet {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
}

/// Every icon set, in the order Customize shows them.
pub const ICON_SETS: &[IconSet] = &[
    IconSet { id: CLASSIC, name: "Classic", about: "After BeOS: glossy, bright and friendly." },
    IconSet { id: "platinum", name: "Platinum", about: "After Mac OS 9: black outlines, lilac greys." },
    IconSet { id: "beos", name: "BeOS", about: "As BeOS R5 drew its icons: flat faces in its 8-bit colours, black outlines." },
];

/// Icon set `id`, or the classic one if there is none by that name.
pub fn icon_set(id: &str) -> &'static IconSet {
    ICON_SETS.iter().find(|s| s.id == id).unwrap_or(&ICON_SETS[0])
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
    /// Deskbar's dots (`Palette::notice`, `unread`), if not the focus
    /// colour and the failure red.
    pub notice: Option<Rgba>,
    pub unread: Option<Rgba>,
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
    notice: None,
    unread: None,
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
    notice: None,
    unread: None,
    border: None,
    gloss: 40,
};

/// A scheme from elsewhere as a theme: its makers' name and colours, the
/// classic icons, and the pack's picture that suits it.
const fn scheme(s: &'static Scheme, wallpaper: &'static str) -> Theme {
    Theme { id: s.id, name: s.name, about: s.about, icons: CLASSIC, wallpaper, colors: Colors::Scheme(s) }
}

/// Every theme, in the order Preferences shows them: Olo's three, then
/// the schemes in theirs. Each with the picture of the wallpaper pack
/// that suits it best (Ben, 2026-11-17: "pick from the wallpapers that
/// olo comes with the best matching default wallpaper for each theme"):
/// chosen by eye from every picture in the pack, for the theme's own
/// colours, light and dark.
pub const THEMES: &[Theme] = &[
    Theme {
        id: CLASSIC,
        name: "Classic",
        about: "After BeOS: glossy, bright and friendly. Sky grey by day and ocean blue by night.",
        icons: CLASSIC,
        // (Olo's own dunes: Ben, 2026-11-16, "make the default
        // wallpaper for the olo classic theme the dunes one". It was
        // Mac OS 9's Blueberry Oxygen, the nearest to Be's blue.)
        wallpaper: "olo-dunes",
        colors: Colors::Own(&Pair {
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
                unread: Some(hex(0xe5392e)),
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
                unread: Some(hex(0xff5a4f)),
                gloss: 60,
                ..DARK
            },
        }),
    },
    Theme {
        id: "platinum",
        name: "Platinum",
        about: "After Mac OS 9: platinum grey, a lavender tab and lilac-blue, friendly outlines.",
        icons: "platinum",
        wallpaper: "quantum-foam",
        colors: Colors::Own(&Pair {
            light: Seeds {
                panel: hex(0xdddddd),
                ink: hex(0x000000),
                document: hex(0xffffff),
                control: hex(0xeeeeee),
                highlight: hex(0x4a4ac4),
                navigation: hex(0x3c3cb8),
                tab: hex(0xb4b4f0),
                on_tab: hex(0x000000),
                tooltip: hex(0xffffcc),
                status: hex(0x6363d6),
                unread: Some(hex(0xdd0806)),
                gloss: 50,
                ..LIGHT
            },
            dark: Seeds {
                panel: hex(0x2c2c34),
                ink: hex(0xe6e6f0),
                document: hex(0x1c1c22),
                control: hex(0x3a3a46),
                highlight: hex(0x5656c8),
                navigation: hex(0xa8a8ff),
                tab: hex(0x9a9ae6),
                on_tab: hex(0x111118),
                tooltip: hex(0x3c3a2c),
                status: hex(0x6c6cdc),
                unread: Some(hex(0xff6259)),
                gloss: 30,
                ..DARK
            },
        }),
    },
    Theme {
        id: "beos",
        name: "BeOS",
        about: "BeOS R5 as it shipped: grey panels, the yellow tab and icons in its 8-bit colours; and a dark version.",
        icons: "beos",
        // (Be's desktop was one blue, 51 102 152: Olo's own Be Blue.)
        wallpaper: "be-blue",
        colors: Colors::Beos,
    },
    // catppuccin-mocha: Catppuccin's mauve, soft.
    scheme(&SCHEMES[0], "macosx-flow-1"),
    // catppuccin-macchiato: lavender and blue.
    scheme(&SCHEMES[1], "macosx-dandelion-seeds"),
    // catppuccin-frappe: the muted flavour's plum.
    scheme(&SCHEMES[2], "macosx-lines-plum"),
    // github: slate.
    scheme(&SCHEMES[3], "macosx-tiger-aqua-graphite"),
    // one-dark: slate blue.
    scheme(&SCHEMES[4], "macosx-snowy-hills"),
    // dracula: its purple, pink and orange.
    scheme(&SCHEMES[5], "macosx-abstract-5"),
    // ayu: its orange.
    scheme(&SCHEMES[6], "macosx-flowing-rock"),
    // monokai: its pink, vivid.
    scheme(&SCHEMES[7], "superstring"),
    // winter-is-coming: snow and a cold lake.
    scheme(&SCHEMES[8], "moraine-lake"),
    // night-owl: the night sky.
    scheme(&SCHEMES[9], "midsummer-night"),
    // tokyo-night: a city's lights at night.
    scheme(&SCHEMES[10], "hong-kong-at-night"),
    // palenight: a pale purple dusk.
    scheme(&SCHEMES[11], "oasis-in-baja"),
    // synthwave-84: a grid under a violet sky.
    scheme(&SCHEMES[12], "boing"),
    // solarized: its teal and cyan.
    scheme(&SCHEMES[13], "macosx-agave"),
    // gruvbox: warm earth.
    scheme(&SCHEMES[14], "burmese-claypots"),
    // nord: pale arctic water.
    scheme(&SCHEMES[15], "macosx-water"),
    // rose-pine: a rose on pine green.
    scheme(&SCHEMES[16], "macosx-lotus"),
    // everforest: a forest.
    scheme(&SCHEMES[17], "os2-garden"),
    // kanagawa: raked sand, indigo.
    scheme(&SCHEMES[18], "macosx-zen-garden"),
    // tomorrow: plain and calm.
    scheme(&SCHEMES[19], "macosx-lines-blue"),
    // zenburn: low-contrast stones.
    scheme(&SCHEMES[20], "macosx-stones"),
    // cobalt2: cobalt, a yellow ribbon.
    scheme(&SCHEMES[21], "olo-ribbons"),
    // oceanic-next: the sea's surface.
    scheme(&SCHEMES[22], "macosx-tranquil-surface"),
];

/// The theme a new user gets (and an unknown setting falls back to: the
/// themes there were before these, too).
pub const DEFAULT: &str = CLASSIC;

/// The theme, and the icon set, whose icons are the drawings in
/// `<data>/icons` themselves.
pub const CLASSIC: &str = "classic";

/// Theme `id`, if there is one.
pub fn get(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id == id)
}

/// Theme `id`, or the default if there is none by that name.
pub fn resolve(id: &str) -> &'static Theme {
    get(id).or_else(|| get(DEFAULT)).expect("the default theme")
}

/// What settings from before the themes were one list mean now: a theme
/// (`look`: Classic or Platinum then) in one of its colour sets
/// (`colors`: `theme` for its own, `beos`, or a scheme's id). The theme
/// to show, and its icons where they aren't that theme's own (Platinum's
/// icons with Dracula's colours: Dracula, customized to Platinum's
/// icons).
pub fn migrate(look: &str, colors: &str) -> (&'static str, Option<&'static str>) {
    let was = resolve(look);
    match get(colors) {
        // (`classic` and `platinum` were never colour sets.)
        Some(now) if !matches!(now.colors, Colors::Own(_)) => (now.id, (was.icons != now.icons && was.id != DEFAULT).then_some(was.icons)),
        _ => (was.id, None),
    }
}

/// What the person changed of a theme (Preferences ▸ Themes ▸
/// Customize…): colours by their role's id ([`ROLES`]) as `#rrggbb`,
/// light and dark apart; its icons (an [`IconSet`]'s id); its wallpaper
/// (as the wallpaper setting holds one). What isn't changed is the
/// theme's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Custom {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub light: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dark: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallpaper: Option<String>,
}

impl Custom {
    /// Nothing is changed.
    pub fn is_empty(&self) -> bool {
        *self == Custom::default()
    }

    /// The colours changed in `mode`.
    pub fn colors(&self, mode: ThemeMode) -> &BTreeMap<String, String> {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }

    pub fn colors_mut(&mut self, mode: ThemeMode) -> &mut BTreeMap<String, String> {
        match mode {
            ThemeMode::Light => &mut self.light,
            ThemeMode::Dark => &mut self.dark,
        }
    }

    /// What tells these colours from the theme's own and from any other
    /// change of them (`Palette::custom`; part of a GTK theme's name):
    /// empty when no colour is changed.
    pub fn key(&self) -> String {
        if self.light.is_empty() && self.dark.is_empty() {
            return String::new();
        }
        // (FNV-1a: the same in every program and every build.)
        let mut h: u32 = 0x811c_9dc5;
        let mut eat = |text: &str| {
            for b in text.bytes().chain([0]) {
                h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
            }
        };
        for (mode, map) in [("light", &self.light), ("dark", &self.dark)] {
            eat(mode);
            for (role, colour) in map {
                eat(role);
                eat(colour);
            }
        }
        format!("{h:08x}")
    }
}

/// A colour of a theme that can be changed: a seed of its palette (what
/// follows from it, bevels and frames and shades, follows the change).
#[derive(Debug, PartialEq, Eq)]
pub struct Role {
    /// What [`Custom`] holds it under.
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
}

/// Every colour that can be changed, in the order Customize shows them:
/// the interface's, then the hues things are told apart by.
pub const ROLES: &[Role] = &[
    Role { id: "panel", name: "Panels", about: "Windows, toolbars and menus." },
    Role { id: "ink", name: "Text", about: "Text, everywhere." },
    Role { id: "document", name: "Documents", about: "Documents and lists, and Terminal's ground." },
    Role { id: "control", name: "Controls", about: "The faces of buttons and fields." },
    Role { id: "border", name: "Outlines", about: "Round buttons, fields and lists." },
    Role { id: "highlight", name: "Highlight", about: "What is chosen: a menu's item, the default button, a selection." },
    Role { id: "on_highlight", name: "Highlighted Text", about: "Text on the highlight." },
    Role { id: "navigation", name: "Focus", about: "The keyboard's focus, links and the text cursor." },
    Role { id: "tab", name: "Window Tab", about: "The focused window's tab." },
    Role { id: "on_tab", name: "Tab Title", about: "The title on the focused window's tab." },
    Role { id: "tooltip", name: "Tooltips", about: "A tooltip's ground." },
    Role { id: "status", name: "Progress", about: "Progress bars." },
    Role { id: "success", name: "Success", about: "What went well." },
    Role { id: "failure", name: "Failure", about: "What went wrong." },
    Role { id: "warning", name: "Warning", about: "Cautions." },
    Role { id: "notice", name: "Notification Dot", about: "Deskbar's dot for an application's notifications." },
    Role { id: "unread", name: "Waiting Dot", about: "Deskbar's dot for what is unread or waiting in an application." },
    Role { id: "red", name: "Red", about: "" },
    Role { id: "orange", name: "Orange", about: "" },
    Role { id: "yellow", name: "Yellow", about: "" },
    Role { id: "green", name: "Green", about: "" },
    Role { id: "teal", name: "Teal", about: "" },
    Role { id: "blue", name: "Blue", about: "" },
    Role { id: "purple", name: "Purple", about: "" },
    Role { id: "pink", name: "Pink", about: "" },
    Role { id: "brown", name: "Brown", about: "" },
    Role { id: "gray", name: "Gray", about: "" },
];

/// `#rrggbb` (or `rrggbb`) as a colour.
pub fn parse_hex(text: &str) -> Option<Rgba> {
    let digits = text.trim().trim_start_matches('#');
    (digits.len() == 6).then(|| u32::from_str_radix(digits, 16).ok().map(hex)).flatten()
}

impl Seeds {
    /// The colour of role `id` ([`ROLES`]) in `mode`: what it is when
    /// the seeds leave it to follow another (the dots, the outlines).
    pub fn get(&self, id: &str, mode: ThemeMode) -> Option<Rgba> {
        Some(match id {
            "panel" => self.panel,
            "ink" => self.ink,
            "document" => self.document,
            "control" => self.control,
            "border" => self.border.unwrap_or(match mode {
                ThemeMode::Light => mix(self.panel, self.ink, 0.56),
                ThemeMode::Dark => mix(self.panel, BLACK, 0.55),
            }),
            "highlight" => self.highlight,
            "on_highlight" => self.on_highlight,
            "navigation" => self.navigation,
            "tab" => self.tab,
            "on_tab" => self.on_tab,
            "tooltip" => self.tooltip,
            "status" => self.status,
            "success" => self.success,
            "failure" => self.failure,
            "warning" => self.warning,
            "notice" => self.notice.unwrap_or(self.navigation),
            "unread" => self.unread.unwrap_or(self.failure),
            "red" => self.hues.red,
            "orange" => self.hues.orange,
            "yellow" => self.hues.yellow,
            "green" => self.hues.green,
            "teal" => self.hues.teal,
            "blue" => self.hues.blue,
            "purple" => self.hues.purple,
            "pink" => self.hues.pink,
            "brown" => self.hues.brown,
            "gray" => self.hues.gray,
            _ => return None,
        })
    }

    /// Role `id` set to `c`; false if there is no such role.
    pub fn set(&mut self, id: &str, c: Rgba) -> bool {
        match id {
            "panel" => self.panel = c,
            "ink" => self.ink = c,
            "document" => self.document = c,
            "control" => self.control = c,
            "border" => self.border = Some(c),
            "highlight" => self.highlight = c,
            "on_highlight" => self.on_highlight = c,
            "navigation" => self.navigation = c,
            "tab" => self.tab = c,
            "on_tab" => self.on_tab = c,
            "tooltip" => self.tooltip = c,
            "status" => self.status = c,
            "success" => self.success = c,
            "failure" => self.failure = c,
            "warning" => self.warning = c,
            "notice" => self.notice = Some(c),
            "unread" => self.unread = Some(c),
            "red" => self.hues.red = c,
            "orange" => self.hues.orange = c,
            "yellow" => self.hues.yellow = c,
            "green" => self.hues.green = c,
            "teal" => self.hues.teal = c,
            "blue" => self.hues.blue = c,
            "purple" => self.hues.purple = c,
            "pink" => self.hues.pink = c,
            "brown" => self.hues.brown = c,
            "gray" => self.hues.gray = c,
            _ => return false,
        }
        true
    }

    /// The seeds palette `p` would have been made from, near enough: for
    /// a palette that wasn't made from seeds (BeOS's), so its colours
    /// can be changed by role like any other's.
    fn of(p: &Palette) -> Seeds {
        Seeds {
            panel: p.panel_background,
            ink: p.panel_text,
            document: p.document_background,
            control: p.control_background,
            highlight: p.menu_selected_background,
            on_highlight: p.menu_selected_item_text,
            navigation: p.keyboard_navigation,
            tab: p.tab_focused,
            on_tab: p.tab_text_focused,
            tooltip: p.tooltip_background,
            status: p.status_bar,
            success: p.success,
            failure: p.failure,
            warning: p.warning,
            hues: p.hues,
            notice: Some(p.notice),
            unread: Some(p.unread),
            border: Some(p.control_border),
            gloss: p.gloss[3],
        }
    }
}

impl Theme {
    /// Its seeds in `mode`: what its colours are changed through.
    pub fn seeds(&self, mode: ThemeMode) -> Seeds {
        match (self.colors, mode) {
            (Colors::Own(pair), ThemeMode::Light) => pair.light,
            (Colors::Own(pair), ThemeMode::Dark) => pair.dark,
            (Colors::Beos, ThemeMode::Light) => Seeds::of(&Palette::light()),
            (Colors::Beos, ThemeMode::Dark) => Seeds::of(&Palette::dark()),
            (Colors::Scheme(s), mode) => s.seeds(mode),
        }
    }

    /// Its terminal's colours in `mode`.
    pub fn terminal(&self, mode: ThemeMode) -> Terminal {
        match (self.colors, mode) {
            (Colors::Own(_), mode) => seed_terminal(&self.seeds(mode), mode, self.id, self.name),
            (Colors::Beos, ThemeMode::Light) => Palette::light().terminal,
            (Colors::Beos, ThemeMode::Dark) => Palette::dark().terminal,
            (Colors::Scheme(s), mode) => s.terminal(mode),
        }
    }

    /// Its colours in `mode`, as it comes.
    fn made(&self, mode: ThemeMode) -> Palette {
        let mut p = match (self.colors, mode) {
            (Colors::Beos, ThemeMode::Light) => Palette::light(),
            (Colors::Beos, ThemeMode::Dark) => Palette::dark(),
            (_, mode) => build(&self.seeds(mode), mode, self.terminal(mode)),
        };
        p.theme = self.id.into();
        p.custom = String::new();
        p
    }
}

/// The other mode.
fn other(mode: ThemeMode) -> ThemeMode {
    match mode {
        ThemeMode::Light => ThemeMode::Dark,
        ThemeMode::Dark => ThemeMode::Light,
    }
}

/// The palette for theme `theme` (an id; unknown ones are the default) in
/// `mode`, as the theme comes. Made once and kept.
pub fn palette(theme: &str, mode: ThemeMode) -> Palette {
    static MADE: OnceLock<Vec<[Palette; 2]>> = OnceLock::new();
    let made = MADE.get_or_init(|| {
        THEMES
            .iter()
            .map(|t| {
                let (mut light, mut dark) = (t.made(ThemeMode::Light), t.made(ThemeMode::Dark));
                (light.other_terminal, dark.other_terminal) = (dark.terminal.clone(), light.terminal.clone());
                [light, dark]
            })
            .collect()
    });
    let t = resolve(theme);
    let at = THEMES.iter().position(|x| x.id == t.id).unwrap_or(0);
    made[at][if mode == ThemeMode::Dark { 1 } else { 0 }].clone()
}

/// The palette for theme `theme` in `mode` with what the person changed
/// of its colours (`custom`): the theme's, with every colour the changed
/// seeds change, changed.
pub fn customized(theme: &str, custom: &Custom, mode: ThemeMode) -> Palette {
    let t = resolve(theme);
    let mut p = one_customized(t, custom, mode);
    p.other_terminal = one_customized(t, custom, other(mode)).terminal;
    p.custom = custom.key();
    p
}

/// [`customized`], its other mode's terminal left as the theme's.
fn one_customized(t: &Theme, custom: &Custom, mode: ThemeMode) -> Palette {
    let p = palette(t.id, mode);
    let base = t.seeds(mode);
    let mut mine = base;
    for (role, colour) in custom.colors(mode) {
        if let Some(c) = parse_hex(colour) {
            mine.set(role, c);
        }
    }
    if mine == base {
        return p;
    }
    // What the theme's seeds make, and what the changed ones do: where
    // those differ, the change shows; everything else is the theme's own
    // (BeOS's and a scheme's aren't just what their seeds make).
    let was = build(&base, mode, seed_terminal(&base, mode, "", ""));
    let now = build(&mine, mode, seed_terminal(&mine, mode, "", ""));
    let mut p = merge(p, &was, &now);
    // The terminal reads on its ground, whatever that is now.
    let bg = p.terminal.background;
    let grounds = if mode == ThemeMode::Light { [7, 15] } else { [0, 8] };
    for (i, c) in p.terminal.ansi.iter_mut().enumerate() {
        if !grounds.contains(&i) {
            *c = readable(*c, bg, 3.0);
        }
    }
    p.terminal.foreground = readable(p.terminal.foreground, bg, 7.0);
    p
}

/// `actual` with each colour that differs between `was` and `now` taken
/// from `now`.
fn merge(actual: Palette, was: &Palette, now: &Palette) -> Palette {
    use serde_json::Value;
    fn go(actual: &mut Value, was: &Value, now: &Value) {
        match (actual, was, now) {
            (Value::Object(a), Value::Object(w), Value::Object(n)) => {
                for (k, v) in a.iter_mut() {
                    if let (Some(w), Some(n)) = (w.get(k), n.get(k)) {
                        go(v, w, n);
                    }
                }
            }
            (Value::Array(a), Value::Array(w), Value::Array(n)) if a.len() == w.len() && w.len() == n.len() => {
                for (i, v) in a.iter_mut().enumerate() {
                    go(v, &w[i], &n[i]);
                }
            }
            (a, w, n) => {
                if w != n {
                    *a = n.clone();
                }
            }
        }
    }
    let json = |p: &Palette| serde_json::to_value(p).unwrap_or(Value::Null);
    let (mut merged, was, now) = (json(&actual), json(was), json(now));
    go(&mut merged, &was, &now);
    serde_json::from_value(merged).unwrap_or(actual)
}

/// The colours a look's tab groups can have ([`Palette::group_colors`]),
/// in the same eight places in every look: red, orange, yellow, green,
/// teal, blue, purple, grey.
///
/// - **Platinum**: Mac OS 9's Finder labels (Hot's red, Essential's
///   orange, Project I's green, Cool's cyan, Personal's blue, In
///   Progress's magenta, Project II's brown), with QuickDraw's yellow
///   softened; at night each lifted to read on the dark panel.
/// - **Every other theme**: BeOS's (its tab's yellow, the red and blue of
///   its marks), even and bright, as Be's were.
///
/// By day the fills are deep enough for white names (all but yellow and
/// orange, and Mac OS 9's bright green and cyan, kept as they were); at
/// night they're lifted and take a near-black name, as Chrome's tab
/// groups do. Each with that ink ([`Palette::label_ink`]).
pub fn group_colors(p: &crate::Palette) -> [crate::GroupColor; crate::GROUP_COLORS] {
    const NAMES: [&str; crate::GROUP_COLORS] = ["Red", "Orange", "Yellow", "Green", "Teal", "Blue", "Purple", "Gray"];
    let dark = p.mode == ThemeMode::Dark;
    let platinum = p.theme == "platinum";
    let (names, fills): ([&str; 8], [Rgba; 8]) = if platinum {
        let names = ["Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Magenta", "Brown"];
        if dark {
            (names, [hex(0xf0453c), hex(0xff8433), hex(0xf2d43c), hex(0x3cc434), hex(0x34b8ee), hex(0x7c7cf4), hex(0xf2509e), hex(0xb0784a)])
        } else {
            (names, [hex(0xdd0806), hex(0xff6402), hex(0xf5d60a), hex(0x1fb714), hex(0x02abea), hex(0x0000d4), hex(0xf20884), hex(0x8a5a2a)])
        }
    } else if dark {
        (NAMES, [hex(0xe5574c), hex(0xf29236), hex(0xf0c21a), hex(0x4db65c), hex(0x34b3bf), hex(0x5288ea), hex(0xa070de), hex(0x9097a0)])
    } else {
        (NAMES, [hex(0xd9372b), hex(0xee7d10), hex(0xffcb00), hex(0x28863a), hex(0x11818d), hex(0x2e6bd6), hex(0x8350c8), hex(0x6f757d)])
    };
    std::array::from_fn(|i| crate::GroupColor { name: names[i], fill: fills[i], ink: p.label_ink(fills[i]) })
}

/// The terminal seeds `s` make (theme `id`, named `name`): their ink on
/// their document colour, Olo's ANSI colours kept readable on it, the
/// cursor in their focus colour.
fn seed_terminal(s: &Seeds, mode: ThemeMode, id: &str, name: &str) -> Terminal {
    let (ansi, ground, word) = match mode {
        ThemeMode::Light => (ANSI_LIGHT, [7, 15], "Light"),
        ThemeMode::Dark => (ANSI_DARK, [0, 8], "Dark"),
    };
    let bg = s.document;
    let mut ansi = ansi;
    for (i, c) in ansi.iter_mut().enumerate() {
        if !ground.contains(&i) {
            *c = readable(*c, bg, 3.0);
        }
    }
    Terminal {
        id: format!("{id}-{}", word.to_lowercase()),
        name: format!("{name} {word}"),
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
/// [`palette`] fills in which look it is, and its other mode's terminal.
pub fn build(s: &Seeds, mode: ThemeMode, terminal: Terminal) -> Palette {
    let (p, ink) = (s.panel, s.ink);
    let readable_on_doc = |c: Rgba| readable(c, s.document, 4.5);
    let gloss = [255, 255, 255, s.gloss];
    match mode {
        ThemeMode::Light => Palette {
            mode,
            theme: String::new(),
            custom: String::new(),
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
            notice: readable(s.notice.unwrap_or(s.navigation), p, 3.0),
            unread: readable(s.unread.unwrap_or(s.failure), p, 3.0),
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
            frame_unfocused: [mix(p, ink, 0.37), mix(p, WHITE, 0.8), mix(p, WHITE, 0.2), mix(p, WHITE, 0.2), mix(p, ink, 0.22), mix(p, ink, 0.37)],
            hues: s.hues,
            other_terminal: terminal.clone(),
            terminal,
        },
        ThemeMode::Dark => Palette {
            mode,
            theme: String::new(),
            custom: String::new(),
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
            notice: readable(s.notice.unwrap_or(s.navigation), p, 3.0),
            unread: readable(s.unread.unwrap_or(s.failure), p, 3.0),
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
            frame_unfocused: [mix(p, BLACK, 0.63), mix(p, ink, 0.2), mix(p, ink, 0.04), mix(p, ink, 0.04), mix(p, BLACK, 0.26), mix(p, BLACK, 0.63)],
            hues: s.hues,
            other_terminal: terminal.clone(),
            terminal,
        },
    }
}

/// The terminal colours Terminal offers in `mode` while theme `theme` is
/// on: the theme's own, then every other theme's (one that several
/// share, Catppuccin's Latte, once).
pub fn terminals(theme: &str, mode: ThemeMode) -> Vec<Terminal> {
    let t = resolve(theme);
    let mut all = vec![palette(t.id, mode).terminal];
    for other in THEMES {
        let terminal = palette(other.id, mode).terminal;
        if !all.iter().any(|t| t.id == terminal.id) {
            all.push(terminal);
        }
    }
    all
}

/// The terminal colours `id` names (a theme's own in a mode:
/// `classic-light`, `beos-dark`, `dracula-dark`; `catppuccin-latte`).
pub fn terminal(id: &str) -> Option<Terminal> {
    THEMES.iter().flat_map(|t| [ThemeMode::Light, ThemeMode::Dark].map(|m| palette(t.id, m).terminal)).find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hue, contrast};

    fn every() -> impl Iterator<Item = (String, Palette)> {
        THEMES.iter().flat_map(|t| [ThemeMode::Light, ThemeMode::Dark].map(|m| (format!("{} {m:?}", t.id), palette(t.id, m))))
    }

    #[test]
    fn ids_are_unique_and_known() {
        for (i, t) in THEMES.iter().enumerate() {
            assert!(THEMES[..i].iter().all(|u| u.id != t.id && u.name != t.name), "{} twice", t.id);
            assert!(ICON_SETS.iter().any(|s| s.id == t.icons), "{}: no icon set {}", t.id, t.icons);
        }
        assert!(get(DEFAULT).is_some() && get(CLASSIC).is_some());
        assert_eq!(resolve("no-such-theme").id, DEFAULT);
        // One list: Olo's three first, then every scheme, each once.
        assert_eq!(THEMES.iter().take(3).map(|t| t.id).collect::<Vec<_>>(), ["classic", "platinum", "beos"]);
        for s in SCHEMES {
            assert!(matches!(get(s.id).map(|t| t.colors), Some(Colors::Scheme(x)) if x.id == s.id), "{} isn't a theme", s.id);
        }
        assert_eq!(THEMES.len(), 3 + SCHEMES.len());
        assert_eq!(icon_set("nope").id, CLASSIC);
        // Roles are told apart, and every one is a seed's.
        let seeds = resolve(DEFAULT).seeds(ThemeMode::Light);
        for (i, r) in ROLES.iter().enumerate() {
            assert!(ROLES[..i].iter().all(|x| x.id != r.id && x.name != r.name), "{} twice", r.id);
            assert!(seeds.get(r.id, ThemeMode::Light).is_some(), "{}", r.id);
            let mut changed = seeds;
            assert!(changed.set(r.id, hex(0x123456)));
            assert_eq!(changed.get(r.id, ThemeMode::Light), Some(hex(0x123456)), "{}", r.id);
        }
        assert_eq!(seeds.get("nope", ThemeMode::Light), None);
    }

    /// Settings from before the themes were one list: a theme in one of
    /// its colour sets.
    #[test]
    fn old_settings_are_a_theme() {
        // Its own colours: the theme. BeOS's, a scheme's: that theme.
        assert_eq!(migrate("classic", "theme"), ("classic", None));
        assert_eq!(migrate("platinum", ""), ("platinum", None));
        assert_eq!(migrate("classic", "beos"), ("beos", None));
        assert_eq!(migrate("classic", "dracula"), ("dracula", None));
        // Platinum's icons in another's colours are kept.
        assert_eq!(migrate("platinum", "tokyo-night"), ("tokyo-night", Some("platinum")));
        assert_eq!(migrate("platinum", "beos"), ("beos", Some("platinum")));
        // What went, and what never was a colour set.
        assert_eq!(migrate("bebox", "catppuccin"), (DEFAULT, None));
        assert_eq!(migrate("platinum", "classic"), ("platinum", None));
        // Now: nothing to do.
        assert_eq!(migrate("dracula", ""), ("dracula", None));
    }

    /// The colours alone, not which look they are.
    fn colors_of(p: &Palette) -> Palette {
        Palette { theme: String::new(), custom: String::new(), ..p.clone() }
    }

    #[test]
    fn beos_is_r5() {
        let p = palette("beos", ThemeMode::Light);
        assert_eq!((p.theme.as_str(), p.custom.as_str()), ("beos", ""));
        assert_eq!(colors_of(&p), colors_of(&Palette::light()));
        assert_eq!(colors_of(&palette("beos", ThemeMode::Dark)), colors_of(&Palette::dark()));
        assert_eq!(Palette::light().panel_background, crate::rgb(216, 216, 216));
        assert_eq!(Palette::light().tab_focused, crate::rgb(255, 203, 0));
    }

    #[test]
    fn every_look_is_its_own() {
        // Each theme's colours differ from every other's, in both modes,
        // and each mode is what it says.
        let all: Vec<(String, Palette)> = every().collect();
        for (i, (a, p)) in all.iter().enumerate() {
            assert_eq!(p.mode == ThemeMode::Dark, crate::luminance(p.panel_background) < 0.18, "{a}");
            // (Its other mode's terminal is the other mode's.)
            assert_ne!(p.other_terminal.mode, p.mode, "{a}");
            assert_eq!(p.other_terminal, palette(&p.theme, other(p.mode)).terminal, "{a}");
            for (b, q) in &all[..i] {
                assert_ne!(p, q, "{a} is {b}");
                // (Catppuccin's three share Latte by day.)
                let latte = |look: &str| look.starts_with("catppuccin-") && look.ends_with("Light");
                if !(latte(a) && latte(b)) {
                    assert_ne!(colors_of(p), colors_of(q), "{a} is {b}");
                }
            }
        }
        // The themes that went are the default now.
        assert_eq!(palette("bebox", ThemeMode::Dark), palette(DEFAULT, ThemeMode::Dark));
        assert_eq!(DEFAULT, CLASSIC);
    }

    /// What the person changes of a theme shows, and what follows from it
    /// follows; nothing else moves, in a theme whose colours aren't just
    /// what its seeds make too.
    #[test]
    fn a_theme_customized() {
        // Nothing changed: the theme, to the colour.
        for t in THEMES {
            for m in [ThemeMode::Light, ThemeMode::Dark] {
                assert_eq!(customized(t.id, &Custom::default(), m), palette(t.id, m), "{} {m:?}", t.id);
            }
        }
        assert!(Custom::default().is_empty() && Custom::default().key().is_empty());
        for theme in ["classic", "beos", "dracula"] {
            let mut custom = Custom::default();
            custom.light.insert("tab".into(), "#ff0080".into());
            custom.light.insert("panel".into(), "#f0e8d8".into());
            let (was, now) = (palette(theme, ThemeMode::Light), customized(theme, &custom, ThemeMode::Light));
            assert_eq!(now.tab_focused, hex(0xff0080), "{theme}");
            assert_eq!(now.panel_background, hex(0xf0e8d8), "{theme}");
            // What's made from them follows: the tab's light edge, the
            // frame, the menus.
            assert_ne!(now.tab_focused_light, was.tab_focused_light, "{theme}");
            assert_ne!(now.frame_focused, was.frame_focused, "{theme}");
            assert_eq!(now.menu_background, hex(0xf0e8d8), "{theme}");
            // What isn't, stays: documents, the highlight, the hues, the
            // terminal.
            assert_eq!((now.document_background, now.menu_selected_background, now.control_highlight), (was.document_background, was.menu_selected_background, was.control_highlight), "{theme}");
            assert_eq!((now.hues, &now.terminal), (was.hues, &was.terminal), "{theme}");
            // Text still reads on what changed (`build` sees to it).
            assert!(contrast(now.tab_text_focused, now.tab_focused) >= 4.5, "{theme}");
            // The other mode is the theme's own, and so is its terminal here.
            assert_eq!(colors_of(&customized(theme, &custom, ThemeMode::Dark)), colors_of(&palette(theme, ThemeMode::Dark)), "{theme}");
            assert_eq!(now.other_terminal, was.other_terminal, "{theme}");
            // It says it is changed, the same way each time, and another
            // change says otherwise.
            assert_eq!(now.custom, custom.key());
            assert_eq!(now.custom.len(), 8);
            let mut other = custom.clone();
            other.dark.insert("tab".into(), "#ff0080".into());
            assert_ne!(other.key(), custom.key());
        }
        // The documents' colour is Terminal's ground, and its colours
        // still read on it.
        let mut custom = Custom::default();
        custom.dark.insert("document".into(), "#202a44".into());
        custom.dark.insert("red".into(), "#ff2020".into());
        custom.dark.insert("nonsense".into(), "#ffffff".into());
        custom.dark.insert("ink".into(), "not a colour".into());
        let p = customized("tokyo-night", &custom, ThemeMode::Dark);
        assert_eq!((p.document_background, p.terminal.background, p.hues.red), (hex(0x202a44), hex(0x202a44), hex(0xff2020)));
        assert_eq!(p.panel_text, palette("tokyo-night", ThemeMode::Dark).panel_text);
        assert!(contrast(p.terminal.foreground, p.terminal.background) >= 7.0);
        for (i, c) in p.terminal.ansi.iter().enumerate().filter(|(i, _)| ![0, 8].contains(i)) {
            assert!(contrast(*c, p.terminal.background) >= 3.0, "colour {i}");
        }
        // In light mode, its dark terminal is the changed one.
        assert_eq!(customized("tokyo-night", &custom, ThemeMode::Light).other_terminal, p.terminal);
        // Icons and a wallpaper of the person's are theirs, not colours.
        let custom = Custom { icons: Some("platinum".into()), wallpaper: Some("pack:tub".into()), ..Custom::default() };
        assert!(!custom.is_empty() && custom.key().is_empty());
        assert_eq!(customized("classic", &custom, ThemeMode::Light), palette("classic", ThemeMode::Light));
        // As settings keep it: only what's changed.
        assert_eq!(serde_json::to_string(&custom).unwrap(), r#"{"icons":"platinum","wallpaper":"pack:tub"}"#);
        assert_eq!(serde_json::from_str::<Custom>("{}").unwrap(), Custom::default());
        assert_eq!(parse_hex("#0a0B0c"), Some(hex(0x0a0b0c)));
        assert_eq!(parse_hex("fff"), None);
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
            if p.theme != "beos" {
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
            // Deskbar's dots: each shows on a panel, and they're told apart.
            for (what, dot) in [("notice", p.notice), ("unread", p.unread)] {
                assert!(contrast(dot, p.panel_background) >= 3.0, "{look}: the {what} dot {:.2}", contrast(dot, p.panel_background));
            }
            let (n, u) = (p.notice, p.unread);
            assert!(u[0] as i32 - u[2] as i32 > 60 && n[2] as i32 + n[1] as i32 > n[0] as i32 * 2 - 40, "{look}: a red dot {u:?} and one that isn't {n:?}");
        }
    }

    #[test]
    fn terminals_read() {
        let mut all: Vec<Terminal> = every().map(|(_, p)| p.terminal).collect();
        all.extend([ThemeMode::Light, ThemeMode::Dark].map(|m| terminals(DEFAULT, m)).concat());
        for t in &all {
            assert!(contrast(t.foreground, t.background) >= 7.0, "{} text: {:.2}", t.name, contrast(t.foreground, t.background));
            assert!(contrast(t.foreground, t.selection) >= 4.5, "{} selection: {:.2}", t.name, contrast(t.foreground, t.selection));
            assert_ne!(t.cursor, t.background, "{}", t.name);
            // Every colour but the one that matches the ground reads.
            let skip = if t.mode == ThemeMode::Light { [7, 15] } else { [0, 8] };
            for (i, c) in t.ansi.iter().enumerate().filter(|(i, _)| !skip.contains(i)) {
                assert!(contrast(*c, t.background) >= 3.0, "{} colour {i}: {:.2}", t.name, contrast(*c, t.background));
            }
            assert_eq!(terminal(&t.id).as_ref(), Some(t), "{}", t.id);
        }
        assert_eq!(terminal("nope"), None);
        assert_eq!(terminal("nope-light"), None);
        assert_eq!(terminal("mocha"), None);
    }

    #[test]
    fn terminal_choices() {
        let names = |m| terminals("platinum", m).into_iter().map(|t| t.id).collect::<Vec<_>>();
        // The theme's own, then every other theme's in the themes' order
        // (Catppuccin's Latte once, though three flavours have it by day).
        let (light, dark) = (names(ThemeMode::Light), names(ThemeMode::Dark));
        assert_eq!(light[..5], ["platinum-light", "classic-light", "beos-light", "catppuccin-latte", "github-light"]);
        assert_eq!(dark[..6], ["platinum-dark", "classic-dark", "beos-dark", "catppuccin-mocha-dark", "catppuccin-macchiato-dark", "catppuccin-frappe-dark"]);
        assert_eq!((light.len(), dark.len()), (THEMES.len() - 2, THEMES.len()));
        // Each is found again by its id, and named as its makers name it.
        for t in terminals("classic", ThemeMode::Light).into_iter().chain(terminals("classic", ThemeMode::Dark)) {
            assert_eq!(terminal(&t.id), Some(t.clone()), "{}", t.id);
        }
        let named = |id: &str| terminal(id).map(|t| t.name);
        assert_eq!(named("classic-light").as_deref(), Some("Classic Light"));
        assert_eq!(named("beos-dark").as_deref(), Some("BeOS Dark"));
        assert_eq!(named("catppuccin-latte").as_deref(), Some("Catppuccin Latte"));
        assert_eq!(named("catppuccin-frappe-dark").as_deref(), Some("Catppuccin Frappé"));
        assert_eq!(named("dracula-light").as_deref(), Some("Dracula Alucard"));
        assert_eq!(named("github-dark").as_deref(), Some("GitHub Dark"));
        assert_eq!(named("one-dark-light").as_deref(), Some("One Light"));
        assert_eq!(named("night-owl-light").as_deref(), Some("Light Owl"));
        assert_eq!(named("tokyo-night-dark").as_deref(), Some("Tokyo Night"));
        assert_eq!(named("winter-is-coming-dark").as_deref(), Some("Winter is Coming Dark Blue"));
        assert_eq!(named("solarized-light").as_deref(), Some("Solarized Light"));
        assert_eq!(named("nord-dark").as_deref(), Some("Nord"));
        assert_eq!(named("rose-pine-light").as_deref(), Some("Rosé Pine Dawn"));
        assert_eq!(named("rose-pine-dark").as_deref(), Some("Rosé Pine"));
        assert_eq!(named("kanagawa-dark").as_deref(), Some("Kanagawa Wave"));
        assert_eq!(named("tomorrow-light").as_deref(), Some("Tomorrow"));
        assert_eq!(named("tomorrow-dark").as_deref(), Some("Tomorrow Night"));
        assert_eq!(named("zenburn-dark").as_deref(), Some("Zenburn"));
        // The look's own terminal is first among its mode's choices.
        for (look, p) in every() {
            assert_eq!(terminals(&p.theme, p.mode)[0], p.terminal, "{look}");
        }
    }

    #[test]
    fn palettes_round_trip_as_json() {
        let p = palette("platinum", ThemeMode::Dark);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["panel_background"], crate::to_hex(p.panel_background));
        assert_eq!(json["desktop_text_shadow"], "#000000c8");
        assert_eq!(json["hues"]["red"], crate::to_hex(p.hues.red));
        assert_eq!(json["terminal"]["id"], "platinum-dark");
        assert_eq!(json["other_terminal"]["id"], "platinum-light");
        assert_eq!((json["theme"].as_str(), json["custom"].as_str()), (Some("platinum"), Some("")));
        assert_eq!(serde_json::from_value::<Palette>(json.clone()).unwrap(), p);
        // One written before it had them reads too.
        let mut old = json;
        old.as_object_mut().unwrap().remove("other_terminal");
        old.as_object_mut().unwrap().remove("custom");
        old["colors"] = "theme".into();
        let read = serde_json::from_value::<Palette>(old).unwrap();
        assert_eq!((read.panel_background, read.custom.as_str(), read.other_terminal.id.as_str()), (p.panel_background, "", "beos-dark"));
    }
}
