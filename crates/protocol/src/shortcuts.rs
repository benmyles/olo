//! Keyboard shortcuts app_server handles itself (window tiling, workspaces,
//! Find, screenshots, …): what each does, its default keys, and the
//! changes kept in `Settings::shortcuts` (Preferences ▸ Keyboard ▸
//! Shortcuts). Keys are written "Ctrl+Cmd+Shift+Left": Cmd is ⌘ (the
//! Super key when it is the command key), Alt is ⌥.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A key with modifiers. `key` is an evdev code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub key: u32,
}

/// Key names (as written in settings) and how Preferences shows them.
const KEYS: &[(u32, &str, &str)] = &[
    (1, "Escape", "Esc"),
    (2, "1", "1"),
    (3, "2", "2"),
    (4, "3", "3"),
    (5, "4", "4"),
    (6, "5", "5"),
    (7, "6", "6"),
    (8, "7", "7"),
    (9, "8", "8"),
    (10, "9", "9"),
    (11, "0", "0"),
    (12, "Minus", "-"),
    (13, "Equal", "="),
    (14, "Backspace", "⌫"),
    (15, "Tab", "⇥"),
    (16, "Q", "Q"),
    (17, "W", "W"),
    (18, "E", "E"),
    (19, "R", "R"),
    (20, "T", "T"),
    (21, "Y", "Y"),
    (22, "U", "U"),
    (23, "I", "I"),
    (24, "O", "O"),
    (25, "P", "P"),
    (26, "BracketLeft", "["),
    (27, "BracketRight", "]"),
    (28, "Return", "↵"),
    (30, "A", "A"),
    (31, "S", "S"),
    (32, "D", "D"),
    (33, "F", "F"),
    (34, "G", "G"),
    (35, "H", "H"),
    (36, "J", "J"),
    (37, "K", "K"),
    (38, "L", "L"),
    (39, "Semicolon", ";"),
    (40, "Apostrophe", "'"),
    (41, "Grave", "`"),
    (43, "Backslash", "\\"),
    (44, "Z", "Z"),
    (45, "X", "X"),
    (46, "C", "C"),
    (47, "V", "V"),
    (48, "B", "B"),
    (49, "N", "N"),
    (50, "M", "M"),
    (51, "Comma", ","),
    (52, "Period", "."),
    (53, "Slash", "/"),
    (57, "Space", "Space"),
    (59, "F1", "F1"),
    (60, "F2", "F2"),
    (61, "F3", "F3"),
    (62, "F4", "F4"),
    (63, "F5", "F5"),
    (64, "F6", "F6"),
    (65, "F7", "F7"),
    (66, "F8", "F8"),
    (67, "F9", "F9"),
    (68, "F10", "F10"),
    (87, "F11", "F11"),
    (88, "F12", "F12"),
    (96, "KeypadEnter", "⌤"),
    (99, "Print", "Print Screen"),
    (102, "Home", "↖"),
    (103, "Up", "↑"),
    (104, "PageUp", "⇞"),
    (105, "Left", "←"),
    (106, "Right", "→"),
    (107, "End", "↘"),
    (108, "Down", "↓"),
    (109, "PageDown", "⇟"),
    (111, "Delete", "⌦"),
];

/// evdev codes of the modifier keys (never a chord's key).
pub const MODIFIER_KEYS: &[u32] = &[29, 97, 42, 54, 56, 100, 125, 126, 58];

impl Chord {
    pub fn new(key: u32) -> Chord {
        Chord { ctrl: false, alt: false, shift: false, cmd: false, key }
    }

    /// Whether `key` can be part of a shortcut.
    pub fn known_key(key: u32) -> bool {
        KEYS.iter().any(|(k, ..)| *k == key)
    }

    /// As on a Mac menu: "⌃⌥⇧⌘←".
    pub fn symbols(&self) -> String {
        let mut s = String::new();
        for (on, sym) in [(self.ctrl, "⌃"), (self.alt, "⌥"), (self.shift, "⇧"), (self.cmd, "⌘")] {
            if on {
                s.push_str(sym);
            }
        }
        let key = KEYS.iter().find(|(k, ..)| *k == self.key).map_or("?", |(_, _, sym)| sym);
        if s.is_empty() || key.chars().count() == 1 {
            s.push_str(key);
        } else {
            // "⌥Print Screen" reads better spaced.
            s.push(' ');
            s.push_str(key);
        }
        s
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+"), (self.cmd, "Cmd+")] {
            if on {
                f.write_str(name)?;
            }
        }
        match KEYS.iter().find(|(k, ..)| *k == self.key) {
            Some((_, name, _)) => f.write_str(name),
            None => write!(f, "Key{}", self.key),
        }
    }
}

impl FromStr for Chord {
    type Err = String;

    fn from_str(s: &str) -> Result<Chord, String> {
        let mut c = Chord::new(0);
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // "Cmd++" would be odd; keys are named, so the last part is the key.
        let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| format!("no key in {s:?}"))?;
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "option" | "opt" => c.alt = true,
                "shift" => c.shift = true,
                "cmd" | "command" | "super" | "meta" => c.cmd = true,
                _ => return Err(format!("unknown modifier {m:?} in {s:?}")),
            }
        }
        c.key = match KEYS.iter().find(|(_, name, _)| name.eq_ignore_ascii_case(key)) {
            Some((code, ..)) => *code,
            None => key.strip_prefix("Key").and_then(|n| n.parse().ok()).ok_or_else(|| format!("unknown key {key:?}"))?,
        };
        Ok(c)
    }
}

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shortcut {
    /// The focused window into part of its display (see [`Tile`]).
    Tile(Tile),
    /// Zoom the focused window, or unzoom it.
    Zoom,
    /// Zoom the focused window top to bottom only (its width and place
    /// across as they are), or unzoom it that way.
    ZoomVertical,
    /// Zoom the focused window side to side only, or unzoom it that way.
    ZoomHorizontal,
    /// The focused window back as it was before it was tiled or zoomed.
    Restore,
    /// The focused window to the middle of its display.
    Center,
    /// Float the focused window over auto-tiled ones, or tile it again.
    Float,
    /// Hide the focused window (minimize it to Deskbar).
    Hide,
    /// Roll the focused window up to just its tab, or back down.
    RollUp,
    /// The next / previous tab in the focused window, in any app with
    /// tabs: app_server hands it Ctrl+Tab / Ctrl+Shift+Tab, the keys
    /// every toolkit (and Chrome, and Olo's kit) takes for that.
    NextTab,
    PreviousTab,
    /// The focused window to the next / previous display.
    NextDisplay,
    PreviousDisplay,
    /// Every window on the active display, in a grid.
    TileAll,
    /// Auto-tiling on or off, for the active display's workspace showing.
    AutoTile,
    /// Focus mode on or off (`WindowingMode::Focus`), as `AutoTile`.
    FocusMode,
    /// Every window on the active display, arranged by a model for what
    /// each shows (Smart Tiling, Preferences ▸ AI).
    SmartTile,
    /// Every window on the active display, stacked diagonally.
    CascadeAll,
    /// Every window on the active display stacked at the top left with
    /// their tabs side by side.
    FanOut,
    /// Every window on the active display zoomed (again: unzoomed).
    ZoomAll,
    /// Cascade All and Fan Out for every window but the focused one,
    /// which stays where it is, in front.
    CascadeOthers,
    FanOutOthers,
    /// Put the windows back as they were before the last arrangement
    /// (again: as the arrangement had them).
    UndoArrangement,
    /// Hide every window on this workspace, or bring them back.
    ShowDesktop,
    /// The next / previous of the active display's workspaces (each
    /// display has its own; the others stay as they are).
    NextWorkspace,
    PreviousWorkspace,
    /// The active display's workspace 1–9 (counting from 0 here).
    Workspace(u8),
    /// The focused window to the next / previous workspace (and along
    /// with it).
    WindowToNextWorkspace,
    WindowToPreviousWorkspace,
    /// The focused window to workspace 1–9 (counting from 0), following
    /// it there.
    WindowToWorkspace(u8),
    /// The overview: every display's workspaces and windows at once
    /// (app_server's overview.rs).
    Overview,
    /// The command palette (apps/palette).
    Palette,
    Find,
    Lock,
    Caffeinate,
    ScreenshotScreen,
    ScreenshotRegion,
    ScreenshotWindow,
    ScreenshotApp,
}

/// A part of a display a window can be tiled into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tile {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
    /// A cell an arranging command (Tile All, Tile App, Smart Tiling)
    /// chose: where it is is the window's part (`TilePart`), so its edges
    /// drag its neighbours like any tile's. Alone, the whole area.
    Cell,
}

impl Tile {
    /// The part, as fractions of the area: (x, y, width, height).
    pub fn fraction(self) -> (f32, f32, f32, f32) {
        const T: f32 = 1.0 / 3.0;
        match self {
            Tile::Left => (0.0, 0.0, 0.5, 1.0),
            Tile::Right => (0.5, 0.0, 0.5, 1.0),
            Tile::Top => (0.0, 0.0, 1.0, 0.5),
            Tile::Bottom => (0.0, 0.5, 1.0, 0.5),
            Tile::TopLeft => (0.0, 0.0, 0.5, 0.5),
            Tile::TopRight => (0.5, 0.0, 0.5, 0.5),
            Tile::BottomLeft => (0.0, 0.5, 0.5, 0.5),
            Tile::BottomRight => (0.5, 0.5, 0.5, 0.5),
            Tile::LeftThird => (0.0, 0.0, T, 1.0),
            Tile::CenterThird => (T, 0.0, T, 1.0),
            Tile::RightThird => (2.0 * T, 0.0, T, 1.0),
            Tile::LeftTwoThirds => (0.0, 0.0, 2.0 * T, 1.0),
            Tile::RightTwoThirds => (T, 0.0, 2.0 * T, 1.0),
            Tile::Cell => (0.0, 0.0, 1.0, 1.0),
        }
    }
}

/// One shortcut as Preferences lists it.
pub struct ShortcutInfo {
    pub action: Shortcut,
    /// Its name in `Settings::shortcuts`.
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    /// Default keys.
    pub keys: &'static [&'static str],
}

const fn s(action: Shortcut, id: &'static str, group: &'static str, title: &'static str, keys: &'static [&'static str]) -> ShortcutInfo {
    ShortcutInfo { action, id, group, title, keys }
}

pub const WINDOWS: &str = "Windows";
pub const ARRANGE: &str = "Arranging";
pub const WORKSPACES: &str = "Workspaces";
pub const DESKTOP: &str = "Desktop";
pub const SCREENSHOTS: &str = "Screenshots";

/// Every shortcut, in the order Preferences shows them.
pub const SHORTCUTS: &[ShortcutInfo] = &[
    s(Shortcut::Tile(Tile::Left), "tile-left", WINDOWS, "Left half", &["Shift+Cmd+Left"]),
    s(Shortcut::Tile(Tile::Right), "tile-right", WINDOWS, "Right half", &["Shift+Cmd+Right"]),
    s(Shortcut::Tile(Tile::Top), "tile-top", WINDOWS, "Top half", &["Shift+Cmd+Up"]),
    s(Shortcut::Tile(Tile::Bottom), "tile-bottom", WINDOWS, "Bottom half", &["Shift+Cmd+Down"]),
    s(Shortcut::Tile(Tile::TopLeft), "tile-top-left", WINDOWS, "Top left quarter", &["Ctrl+Cmd+U"]),
    s(Shortcut::Tile(Tile::TopRight), "tile-top-right", WINDOWS, "Top right quarter", &["Ctrl+Cmd+I"]),
    s(Shortcut::Tile(Tile::BottomLeft), "tile-bottom-left", WINDOWS, "Bottom left quarter", &["Ctrl+Cmd+J"]),
    s(Shortcut::Tile(Tile::BottomRight), "tile-bottom-right", WINDOWS, "Bottom right quarter", &["Ctrl+Cmd+K"]),
    s(Shortcut::Tile(Tile::LeftThird), "tile-left-third", WINDOWS, "Left third", &["Ctrl+Cmd+D"]),
    s(Shortcut::Tile(Tile::CenterThird), "tile-center-third", WINDOWS, "Center third", &["Ctrl+Cmd+F"]),
    s(Shortcut::Tile(Tile::RightThird), "tile-right-third", WINDOWS, "Right third", &["Ctrl+Cmd+G"]),
    s(Shortcut::Tile(Tile::LeftTwoThirds), "tile-left-two-thirds", WINDOWS, "Left two thirds", &["Ctrl+Cmd+E"]),
    s(Shortcut::Tile(Tile::RightTwoThirds), "tile-right-two-thirds", WINDOWS, "Right two thirds", &["Ctrl+Cmd+T"]),
    s(Shortcut::Zoom, "zoom", WINDOWS, "Zoom or unzoom", &["Shift+Cmd+Return"]),
    s(Shortcut::ZoomVertical, "zoom-vertical", WINDOWS, "Zoom vertically", &["Alt+Shift+Cmd+Up", "Alt+Shift+Cmd+Down"]),
    s(Shortcut::ZoomHorizontal, "zoom-horizontal", WINDOWS, "Zoom horizontally", &["Alt+Shift+Cmd+Left", "Alt+Shift+Cmd+Right"]),
    s(Shortcut::Restore, "restore", WINDOWS, "Back to its own size", &["Shift+Cmd+Backspace"]),
    s(Shortcut::Center, "center", WINDOWS, "Center", &["Ctrl+Cmd+C"]),
    s(Shortcut::Float, "float-window", WINDOWS, "Float over the tiles, or tile", &["Ctrl+Alt+Cmd+F"]),
    s(Shortcut::Hide, "hide-window", WINDOWS, "Hide window", &["Cmd+M"]),
    s(Shortcut::RollUp, "roll-up", WINDOWS, "Roll up or down", &["Ctrl+Cmd+M"]),
    s(Shortcut::NextTab, "next-tab", WINDOWS, "Next tab in the window", &["Ctrl+Tab"]),
    s(Shortcut::PreviousTab, "previous-tab", WINDOWS, "Previous tab in the window", &["Ctrl+Shift+Tab"]),
    s(Shortcut::NextDisplay, "next-display", WINDOWS, "To the next display", &["Ctrl+Alt+Cmd+Right"]),
    s(Shortcut::PreviousDisplay, "previous-display", WINDOWS, "To the previous display", &["Ctrl+Alt+Cmd+Left"]),
    s(Shortcut::TileAll, "tile-all", ARRANGE, "Tile all windows on the display", &["Ctrl+Cmd+Return"]),
    s(Shortcut::AutoTile, "auto-tiling", ARRANGE, "Auto-tiling on or off", &["Ctrl+Alt+Cmd+T"]),
    s(Shortcut::FocusMode, "focus-mode", ARRANGE, "Focus mode on or off", &["Ctrl+Alt+Cmd+O"]),
    s(Shortcut::SmartTile, "smart-tile", ARRANGE, "Smart tile all windows on the display", &["Ctrl+Shift+Cmd+Return"]),
    s(Shortcut::ZoomAll, "zoom-all", ARRANGE, "Zoom or unzoom all windows on the display", &["Ctrl+Alt+Cmd+Return"]),
    s(Shortcut::CascadeAll, "cascade-all", ARRANGE, "Cascade all windows on the display", &["Ctrl+Cmd+Backslash"]),
    s(Shortcut::FanOut, "fan-out", ARRANGE, "Fan out all windows, tabs side by side", &["Ctrl+Cmd+Slash"]),
    s(Shortcut::CascadeOthers, "cascade-others", ARRANGE, "Cascade the other windows, leaving this one", &["Ctrl+Shift+Cmd+Backslash"]),
    s(Shortcut::FanOutOthers, "fan-out-others", ARRANGE, "Fan out the other windows, leaving this one", &["Ctrl+Shift+Cmd+Slash"]),
    s(Shortcut::UndoArrangement, "undo-arrangement", ARRANGE, "Undo window arrangement", &["Ctrl+Cmd+Z"]),
    s(Shortcut::ShowDesktop, "show-desktop", ARRANGE, "Show the desktop", &["Ctrl+Cmd+H"]),
    s(Shortcut::NextWorkspace, "next-workspace", WORKSPACES, "Next workspace", &["Ctrl+Cmd+Right"]),
    s(Shortcut::PreviousWorkspace, "previous-workspace", WORKSPACES, "Previous workspace", &["Ctrl+Cmd+Left"]),
    s(Shortcut::WindowToNextWorkspace, "window-to-next-workspace", WORKSPACES, "Window to the next workspace", &["Ctrl+Shift+Cmd+Right"]),
    s(Shortcut::WindowToPreviousWorkspace, "window-to-previous-workspace", WORKSPACES, "Window to the previous workspace", &["Ctrl+Shift+Cmd+Left"]),
    s(Shortcut::Workspace(0), "workspace-1", WORKSPACES, "Workspace 1", &["Ctrl+Cmd+1"]),
    s(Shortcut::Workspace(1), "workspace-2", WORKSPACES, "Workspace 2", &["Ctrl+Cmd+2"]),
    s(Shortcut::Workspace(2), "workspace-3", WORKSPACES, "Workspace 3", &["Ctrl+Cmd+3"]),
    s(Shortcut::Workspace(3), "workspace-4", WORKSPACES, "Workspace 4", &["Ctrl+Cmd+4"]),
    s(Shortcut::Workspace(4), "workspace-5", WORKSPACES, "Workspace 5", &["Ctrl+Cmd+5"]),
    s(Shortcut::Workspace(5), "workspace-6", WORKSPACES, "Workspace 6", &["Ctrl+Cmd+6"]),
    s(Shortcut::Workspace(6), "workspace-7", WORKSPACES, "Workspace 7", &["Ctrl+Cmd+7"]),
    s(Shortcut::Workspace(7), "workspace-8", WORKSPACES, "Workspace 8", &["Ctrl+Cmd+8"]),
    s(Shortcut::Workspace(8), "workspace-9", WORKSPACES, "Workspace 9", &["Ctrl+Cmd+9"]),
    s(Shortcut::WindowToWorkspace(0), "window-to-workspace-1", WORKSPACES, "Window to workspace 1", &["Ctrl+Shift+Cmd+1"]),
    s(Shortcut::WindowToWorkspace(1), "window-to-workspace-2", WORKSPACES, "Window to workspace 2", &["Ctrl+Shift+Cmd+2"]),
    s(Shortcut::WindowToWorkspace(2), "window-to-workspace-3", WORKSPACES, "Window to workspace 3", &["Ctrl+Shift+Cmd+3"]),
    s(Shortcut::WindowToWorkspace(3), "window-to-workspace-4", WORKSPACES, "Window to workspace 4", &["Ctrl+Shift+Cmd+4"]),
    s(Shortcut::WindowToWorkspace(4), "window-to-workspace-5", WORKSPACES, "Window to workspace 5", &["Ctrl+Shift+Cmd+5"]),
    s(Shortcut::WindowToWorkspace(5), "window-to-workspace-6", WORKSPACES, "Window to workspace 6", &["Ctrl+Shift+Cmd+6"]),
    s(Shortcut::WindowToWorkspace(6), "window-to-workspace-7", WORKSPACES, "Window to workspace 7", &["Ctrl+Shift+Cmd+7"]),
    s(Shortcut::WindowToWorkspace(7), "window-to-workspace-8", WORKSPACES, "Window to workspace 8", &["Ctrl+Shift+Cmd+8"]),
    s(Shortcut::WindowToWorkspace(8), "window-to-workspace-9", WORKSPACES, "Window to workspace 9", &["Ctrl+Shift+Cmd+9"]),
    s(Shortcut::Palette, "palette", DESKTOP, "Command palette", &["Cmd+Space"]),
    s(Shortcut::Overview, "overview", DESKTOP, "Overview of every window", &["Ctrl+Space"]),
    s(Shortcut::Find, "find", DESKTOP, "Find", &["Alt+Cmd+Space"]),
    s(Shortcut::Lock, "lock", DESKTOP, "Lock the screen", &["Ctrl+Cmd+Q"]),
    s(Shortcut::Caffeinate, "caffeinate", DESKTOP, "Stay awake, or not", &["Ctrl+Cmd+A"]),
    s(Shortcut::ScreenshotScreen, "screenshot-screen", SCREENSHOTS, "Screenshot of the screen", &["Shift+Cmd+3", "Print"]),
    s(Shortcut::ScreenshotRegion, "screenshot-region", SCREENSHOTS, "Screenshot of an area", &["Shift+Cmd+4"]),
    s(Shortcut::ScreenshotWindow, "screenshot-window", SCREENSHOTS, "Screenshot of the window", &["Alt+Print"]),
    s(Shortcut::ScreenshotApp, "screenshot-app", SCREENSHOTS, "Screenshot app", &["Shift+Cmd+5"]),
];

/// The shortcut with this id.
pub fn by_id(id: &str) -> Option<&'static ShortcutInfo> {
    SHORTCUTS.iter().find(|i| i.id == id)
}

/// The groups, in order.
pub const GROUPS: &[&str] = &[WINDOWS, ARRANGE, WORKSPACES, DESKTOP, SCREENSHOTS];

/// Keys app_server keeps whatever the shortcuts say (shown, not editable).
pub const FIXED: &[(&str, &str)] = &[
    ("Switch applications (Twitcher)", "⌘⇥"),
    ("Switch windows of the application", "⌘`"),
];

/// Keys changed from the defaults, by shortcut id; an empty list turns a
/// shortcut off. Ids not listed keep their default keys.
pub type ShortcutSettings = BTreeMap<String, Vec<String>>;

/// The keys of a shortcut, given the changes.
pub fn keys_for(info: &ShortcutInfo, changed: &ShortcutSettings) -> Vec<Chord> {
    match changed.get(info.id) {
        Some(keys) => keys.iter().filter_map(|k| k.parse().ok()).collect(),
        None => info.keys.iter().filter_map(|k| k.parse().ok()).collect(),
    }
}

/// Every chord and what it does (the first shortcut wins a clash).
pub fn bindings(changed: &ShortcutSettings) -> Vec<(Chord, Shortcut)> {
    let mut out: Vec<(Chord, Shortcut)> = Vec::new();
    for info in SHORTCUTS {
        for c in keys_for(info, changed) {
            if !out.iter().any(|(o, _)| *o == c) {
                out.push((c, info.action));
            }
        }
    }
    out
}

/// Give shortcut `id` just `chord` (None: no keys), taking it from any
/// other shortcut that had it. Returns the shortcuts it was taken from.
pub fn assign(changed: &mut ShortcutSettings, id: &str, chord: Option<Chord>) -> Vec<&'static str> {
    let mut taken = Vec::new();
    if let Some(c) = chord {
        for info in SHORTCUTS.iter().filter(|i| i.id != id) {
            let keys = keys_for(info, changed);
            if keys.contains(&c) {
                let rest: Vec<String> = keys.iter().filter(|k| **k != c).map(|k| k.to_string()).collect();
                changed.insert(info.id.to_owned(), rest);
                taken.push(info.title);
            }
        }
    }
    changed.insert(id.to_owned(), chord.into_iter().map(|c| c.to_string()).collect());
    // Back to the default: nothing to keep.
    if let Some(info) = SHORTCUTS.iter().find(|i| i.id == id)
        && keys_for(info, changed) == info.keys.iter().filter_map(|k| k.parse().ok()).collect::<Vec<Chord>>()
    {
        changed.remove(id);
    }
    taken
}

/// How a display of a workspace arranges its windows (Deskbar's tray,
/// `Request::SetWindowingMode`; Preferences ▸ Windows for where none is
/// chosen).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WindowingMode {
    /// Windows stay where they are put.
    #[default]
    Standard,
    /// Every window in a tile: a new one takes its share, a closed one's
    /// room goes to its neighbours.
    AutoTiling,
    /// Only the active window shows, zoomed (with its app's other windows
    /// grouped behind it, and dialogs and floating windows over it).
    Focus,
}

impl WindowingMode {
    pub const ALL: [WindowingMode; 3] = [WindowingMode::Standard, WindowingMode::AutoTiling, WindowingMode::Focus];

    /// Its name in menus.
    pub fn title(self) -> &'static str {
        match self {
            WindowingMode::Standard => "Standard",
            WindowingMode::AutoTiling => "Auto-Tiling",
            WindowingMode::Focus => "Focus",
        }
    }

    /// What it does, in a few words.
    pub fn detail(self) -> &'static str {
        match self {
            WindowingMode::Standard => "Windows stay put",
            WindowingMode::AutoTiling => "New windows take a tile",
            WindowingMode::Focus => "One window at a time",
        }
    }

    /// Its name in commands and the IPC: `standard`, `auto_tiling`,
    /// `focus`.
    pub fn id(self) -> &'static str {
        match self {
            WindowingMode::Standard => "standard",
            WindowingMode::AutoTiling => "auto_tiling",
            WindowingMode::Focus => "focus",
        }
    }

    pub fn from_id(id: &str) -> Option<WindowingMode> {
        WindowingMode::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// Space around arranged windows (Preferences ▸ Windows), grouping, and
/// windowing modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TilingSettings {
    /// Between tiled windows (logical pixels).
    pub gap: u32,
    /// Between tiled windows and the edges of the display.
    pub margin: u32,
    /// Apps whose windows aren't grouped (by app id): every other app's
    /// windows share one frame with a tab each (app_server's `groups`).
    /// In Focus mode an app's windows are grouped whatever this says.
    pub ungrouped_apps: Vec<String>,
    /// The windowing mode of every workspace (and display) that hasn't
    /// one of its own (Preferences ▸ Windows).
    pub default_mode: WindowingMode,
    /// The workspaces (and displays of them) whose mode isn't the
    /// default (see [`TilingSettings::mode`]). app_server keeps it as
    /// modes are chosen (`Request::SetWindowingMode`): a copy sent back
    /// with the rest of the settings is ignored.
    pub modes: Vec<ModeChoice>,
    /// Where auto-tiling was on, as settings kept it before windowing
    /// modes: read into `modes` (`TilingSettings::upgrade`), never
    /// written.
    #[serde(skip_serializing)]
    pub auto: Vec<AutoTiling>,
}

impl Default for TilingSettings {
    fn default() -> Self {
        TilingSettings {
            gap: 8,
            margin: 8,
            ungrouped_apps: Vec::new(),
            default_mode: WindowingMode::Standard,
            modes: Vec::new(),
            auto: Vec::new(),
        }
    }
}

/// The windowing mode of a display's (`display`, its `display_key`)
/// workspace (counting from 0); without a display (as older versions
/// kept them), every display's workspace of that number that has none
/// of its own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModeChoice {
    pub workspace: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    pub mode: WindowingMode,
}

/// Auto-tiling on or off, as settings kept it before windowing modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoTiling {
    pub workspace: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    pub on: bool,
}

impl TilingSettings {
    /// Settings from before windowing modes: auto-tiling's ons and offs
    /// as modes.
    pub fn upgrade(&mut self) {
        for a in std::mem::take(&mut self.auto) {
            let mode = if a.on { WindowingMode::AutoTiling } else { WindowingMode::Standard };
            if !self.modes.iter().any(|m| m.workspace == a.workspace && m.display == a.display) {
                self.modes.push(ModeChoice { workspace: a.workspace, display: a.display, mode });
            }
        }
    }

    /// The windowing mode of the display `display` (its key) on its
    /// workspace `workspace`: its own, else one chosen for that
    /// workspace on every display (as older versions kept them), else
    /// the default.
    pub fn mode(&self, workspace: u32, display: &str) -> WindowingMode {
        let find = |d: Option<&str>| self.modes.iter().find(|a| a.workspace == workspace && a.display.as_deref() == d).map(|a| a.mode);
        find(Some(display)).or_else(|| find(None)).unwrap_or(self.default_mode)
    }

    /// Whether auto-tiling is the mode there (as `mode` reads it).
    pub fn auto_tiling(&self, workspace: u32, display: &str) -> bool {
        self.mode(workspace, display) == WindowingMode::AutoTiling
    }

    /// Choose the mode of `display` on its workspace `workspace`. Only
    /// what differs from what it would have anyway is kept, so a
    /// workspace never chosen follows the default.
    pub fn set_mode(&mut self, workspace: u32, display: &str, mode: WindowingMode) {
        self.modes.retain(|a| !(a.workspace == workspace && a.display.as_deref() == Some(display)));
        if self.mode(workspace, display) != mode {
            self.modes.push(ModeChoice { workspace, display: Some(display.to_owned()), mode });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_round_trip() {
        for info in SHORTCUTS {
            for k in info.keys {
                let c: Chord = k.parse().unwrap_or_else(|e| panic!("{}: {e}", info.id));
                assert_eq!(c.to_string(), *k, "{}", info.id);
            }
        }
        let c: Chord = "ctrl+option+command+left".parse().unwrap();
        assert!(c.ctrl && c.alt && c.cmd && !c.shift && c.key == 105);
        assert_eq!(c.symbols(), "⌃⌥⌘←");
        assert_eq!("Alt+Print".parse::<Chord>().unwrap().symbols(), "⌥ Print Screen");
        assert!("Cmd+Nope".parse::<Chord>().is_err() && "Hyper+A".parse::<Chord>().is_err());
    }

    #[test]
    fn defaults_never_clash() {
        let all: Vec<Chord> = SHORTCUTS.iter().flat_map(|i| keys_for(i, &Default::default())).collect();
        for (i, c) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(c), "{c} is used twice");
        }
        let ids: std::collections::HashSet<&str> = SHORTCUTS.iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), SHORTCUTS.len());
        assert!(SHORTCUTS.iter().all(|i| GROUPS.contains(&i.group)));
    }

    #[test]
    fn modes_by_workspace_and_display() {
        use WindowingMode::*;
        let mut t = TilingSettings::default();
        assert_eq!(t.mode(0, "eDP-1"), Standard);
        // Each display's workspaces have their own.
        t.set_mode(1, "eDP-1", AutoTiling);
        assert!(t.auto_tiling(1, "eDP-1") && !t.auto_tiling(1, "DP-2") && !t.auto_tiling(0, "eDP-1"));
        t.set_mode(1, "DP-2", Focus);
        assert_eq!((t.mode(1, "DP-2"), t.mode(1, "eDP-1")), (Focus, AutoTiling));
        // A workspace's choice for every display, as kept before: a
        // display without its own follows it.
        t.modes.push(ModeChoice { workspace: 3, display: None, mode: AutoTiling });
        assert!(t.auto_tiling(3, "HDMI-A-1"));
        t.set_mode(3, "HDMI-A-1", Standard);
        assert_eq!((t.mode(3, "HDMI-A-1"), t.mode(3, "eDP-1")), (Standard, AutoTiling));
        t.modes.retain(|m| m.workspace != 3);
        // Back to what it would be anyway: nothing kept.
        t.set_mode(1, "eDP-1", Standard);
        t.set_mode(1, "DP-2", Standard);
        assert!(t.modes.is_empty(), "{:?}", t.modes);
        t.set_mode(0, "eDP-1", Standard);
        assert!(t.modes.is_empty());
        // Those never chosen follow the default.
        t.set_mode(2, "eDP-1", Standard);
        t.default_mode = Focus;
        assert_eq!((t.mode(0, "eDP-1"), t.mode(2, "eDP-1")), (Focus, Focus));
        t.set_mode(2, "eDP-1", Standard);
        assert_eq!((t.mode(0, "eDP-1"), t.mode(2, "eDP-1")), (Focus, Standard));
    }

    #[test]
    fn auto_tiling_settings_become_modes() {
        let old = r#"{"gap":8,"margin":8,"auto":[{"workspace":1,"on":true},{"workspace":2,"display":"DP-2","on":false}]}"#;
        let mut t: TilingSettings = serde_json::from_str(old).unwrap();
        t.upgrade();
        assert_eq!(t.mode(1, "eDP-1"), WindowingMode::AutoTiling);
        assert_eq!(t.mode(2, "DP-2"), WindowingMode::Standard);
        let saved = serde_json::to_string(&t).unwrap();
        assert!(!saved.contains("\"auto\"") && saved.contains("auto_tiling"), "{saved}");
    }

    #[test]
    fn assigning_takes_keys_from_others() {
        let mut changed = ShortcutSettings::new();
        let c: Chord = "Shift+Cmd+Left".parse().unwrap();
        assert_eq!(assign(&mut changed, "center", Some(c)), ["Left half"]);
        assert_eq!(changed["tile-left"], Vec::<String>::new());
        assert_eq!(changed["center"], ["Shift+Cmd+Left"]);
        let b = bindings(&changed);
        assert!(b.contains(&(c, Shortcut::Center)));
        // Back to its default: forgotten.
        assign(&mut changed, "center", "Ctrl+Cmd+C".parse().ok());
        assert!(!changed.contains_key("center"));
        // Off.
        assign(&mut changed, "find", None);
        assert!(!bindings(&changed).iter().any(|(_, s)| *s == Shortcut::Find));
    }
}
