//! Keyboard shortcuts app_server handles itself (window tiling, workspaces,
//! Find, screenshots, …): what each does, its keys on each platform a
//! keyboard profile follows ([`Platform`]: Mac, Windows, BeOS), and the
//! profile ([`KeyboardProfile`], Preferences ▸ Keyboard) with the changes
//! made to them. Keys are written "Ctrl+Cmd+Shift+Left" and are the
//! physical ones: Cmd is the Super key (⌘ on a Mac, the Windows key,
//! BeOS's Option), Alt is ⌥ (BeOS's command key).

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

/// A key's name as a PC keyboard writes it (Windows, BeOS): "Enter",
/// "Backspace", "Del", arrows as arrows.
pub fn key_word(key: u32) -> String {
    match key {
        1 => "Esc".into(),
        14 => "Backspace".into(),
        15 => "Tab".into(),
        28 | 96 => "Enter".into(),
        99 => "PrtSc".into(),
        102 => "Home".into(),
        104 => "PgUp".into(),
        107 => "End".into(),
        109 => "PgDn".into(),
        111 => "Del".into(),
        k => KEYS.iter().find(|(c, ..)| *c == k).map_or_else(|| format!("Key{k}"), |(_, _, sym)| (*sym).to_owned()),
    }
}

/// evdev codes of the modifier keys (never a chord's key).
pub const MODIFIER_KEYS: &[u32] = &[29, 97, 42, 54, 56, 100, 125, 126, 58];

impl Chord {
    pub fn new(key: u32) -> Chord {
        Chord { ctrl: false, alt: false, shift: false, cmd: false, key }
    }

    /// The keys pressed for it, as evdev codes (its modifiers are physical
    /// keys: ⌘ the Super key, Option Alt), modifiers first: for
    /// `Request::PressKeys`, which takes them through the profile as the
    /// person's own.
    pub fn physical(&self) -> Vec<u32> {
        let mods = [(self.ctrl, 29), (self.alt, 56), (self.cmd, 125), (self.shift, 42)];
        mods.iter().filter(|(on, _)| *on).map(|&(_, code)| code).chain(std::iter::once(self.key)).collect()
    }

    /// Whether `key` can be part of a shortcut.
    pub fn known_key(key: u32) -> bool {
        KEYS.iter().any(|(k, ..)| *k == key)
    }

    /// As `platform` writes it: on a Mac "⌃⌥⇧⌘←"; on Windows
    /// "Win+Ctrl+Alt+Shift+←" (the Super key is the Windows key); on BeOS
    /// "Ctrl+Alt+Opt+Shift+←" (Haiku's names; the Super key is Option).
    pub fn label(&self, platform: Platform) -> String {
        let mods: &[(bool, &str)] = match platform {
            Platform::Mac => return self.symbols(),
            Platform::Windows => &[(self.cmd, "Win"), (self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift")],
            Platform::Beos => &[(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.cmd, "Opt"), (self.shift, "Shift")],
        };
        let mut s = String::new();
        for (on, name) in mods {
            if *on {
                s.push_str(name);
                s.push('+');
            }
        }
        s.push_str(&key_word(self.key));
        s
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
                "cmd" | "command" | "super" | "meta" | "win" | "windows" => c.cmd = true,
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

/// A direction on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];

    /// Its name in shortcuts' ids and commands: "left".
    pub fn id(self) -> &'static str {
        match self {
            Dir::Left => "left",
            Dir::Right => "right",
            Dir::Up => "up",
            Dir::Down => "down",
        }
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
    /// Main and Stack: the focused window is the main one, in the main
    /// window's place; the one that was takes its place in the stack.
    MakeMain,
    /// Main and Stack: the focused window is the main one and stays so
    /// as windows open (they take the stack's places), until another is
    /// made main by hand; on the pinned one, it's let go again.
    PinMain,
    /// Main and Stack: the main window before this one is the main one
    /// again, and this one goes back to the stack (a new window took
    /// the main place: put it back).
    SwapMain,
    /// The window beside the focused one in a direction takes the
    /// keyboard (and comes to the front): on its display, else the next
    /// display that way (as i3, sway, bspwm and Hyprland do).
    Focus(Dir),
    /// Auto-tiling: the focused tile trades places with the one beside it
    /// that way.
    Swap(Dir),
    /// Auto-tiling: the edge beside the focused tile that way moves that
    /// way (its own edge on that side, else the one on the other side), a
    /// step at a time, least sizes kept.
    Resize(Dir),
    /// Auto-tiling's next / previous layout on the active display's
    /// workspace (`TilingLayout::ALL`, round), as tiling window managers
    /// cycle theirs.
    NextLayout,
    PreviousLayout,
    /// The active display's auto-tiled arrangement turned a quarter,
    /// clockwise (`true`) or back; mirrored left and right (`true`) or top
    /// and bottom; its runs of tiles made as big as each other (Main and
    /// Stack: its main window's place turned or mirrored, its share put
    /// back), as bspwm's rotate, flip and balance.
    RotateTiles(bool),
    FlipTiles(bool),
    EvenOutTiles,
    /// The split the focused tile shares with its neighbour turned the
    /// other way (i3's and Hyprland's toggled split).
    ToggleSplit,
    /// More or less space between arranged windows (`TilingSettings::gap`,
    /// a step of 4 points), as Hyprland's and i3's gap keys.
    MoreGap,
    LessGap,
    /// One more or one fewer main window in Main and Stack on the active
    /// display's workspace (dwm's and xmonad's nmaster keys).
    MoreMains,
    FewerMains,
    /// Main and Stack's main part a step bigger or smaller, whichever
    /// tile has the focus (dwm's and xmonad's Expand / Shrink).
    BiggerMain,
    SmallerMain,
    /// The focused window on every workspace of its display, or back on
    /// one (BeOS's `B_ALL_WORKSPACES`, i3's sticky).
    AllWorkspaces,
    /// In the auto-tiled windows' order (Main and Stack's main windows
    /// then its stack; each layout's own): the keyboard to the next
    /// (`true`) or previous window, wrapping round (dwm's focusstack,
    /// xmonad's focusDown / focusUp); the focused window traded with it
    /// (xmonad's swapDown / swapUp); every window one place on, or back
    /// (Amethyst's rotate), the arrangement's shape kept.
    FocusInOrder(bool),
    /// The next new window goes on that side of the focused tile
    /// (bspwm's preselection, i3's `split h` / `split v` before it
    /// opens), shown on the tile till then; again, not.
    Preselect(Dir),
    SwapInOrder(bool),
    RotateWindows(bool),
    /// Scrolling: the focused column's width the next of a third, a half,
    /// two thirds and all of the display (niri's preset widths).
    ColumnWidth,
    /// The focused window into the scratchpad (hidden, floating), or out
    /// of it; and the scratchpad's next window shown over the active
    /// workspace, centred, or hidden again (i3's and sway's).
    ToScratchpad,
    Scratchpad,
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
    RedoArrangement,
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
    /// The emoji picker (apps/emoji-picker): the emoji chosen goes into
    /// the field that had the keyboard.
    EmojiPicker,
    Find,
    Lock,
    Caffeinate,
    /// Fold Deskbar away to the screen's edge, or bring it back (as its
    /// own button does: `deskbar.folded`, every display's; Ben,
    /// 2026-11-18: "ctrl+cmd+d by default (but customizable)").
    ToggleDeskbar,
    ScreenshotScreen,
    ScreenshotRegion,
    ScreenshotWindow,
    ScreenshotApp,
    /// A recording of the screen started, or the one going finished.
    ScreenRecording,
    /// Twitcher, held: every workspace's applications (Mac ⌘Tab,
    /// Windows Alt+Tab, BeOS Ctrl+Tab), back with Shift, chosen when the
    /// keys held are let go.
    SwitchApps,
    /// Twitcher for the workspaces showing (Mac ⌥Tab).
    SwitchAppsHere,
    /// Twitcher for the focused application's windows (⌘\`).
    SwitchWindows,
    /// Every shortcut of the profile, and the front app's, on a sheet
    /// (Mac ⇧⌘/, as Help is ⌘?).
    ShortcutHelp,
    /// Hide every window of the focused application (Mac ⌘H).
    HideApp,
    /// Activity Monitor, to see what runs and end what hangs (Mac ⌥⌘⎋,
    /// Windows' Task Manager Ctrl+Shift+Esc, BeOS's Team Monitor
    /// Ctrl+Alt+Del).
    ActivityMonitor,
    /// The focused window behind every other (BeOS Ctrl+Alt+B).
    SendToBack,
    /// Zoomed or tiled: back to its own size; else hidden (Windows ⊞↓).
    RestoreOrHide,
    /// Close the focused window, whatever the app (Windows' Alt+F4).
    CloseWindow,
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
    /// The middle two thirds across, a sixth either side.
    CenterTwoThirds,
    TopTwoThirds,
    BottomTwoThirds,
    /// A cell an arranging command (Tile All, Tile App, `Arrange`)
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
            Tile::CenterTwoThirds => (T / 2.0, 0.0, 2.0 * T, 1.0),
            Tile::TopTwoThirds => (0.0, 0.0, 1.0, 2.0 * T),
            Tile::BottomTwoThirds => (0.0, T, 1.0, 2.0 * T),
            Tile::Cell => (0.0, 0.0, 1.0, 1.0),
        }
    }

    /// Every part a person can choose (the window menu's Tile, Main and
    /// Stack's main window), as the menus list them: halves, quarters,
    /// thirds, two thirds.
    pub const CHOOSABLE: [Tile; 16] = [
        Tile::Left,
        Tile::Right,
        Tile::Top,
        Tile::Bottom,
        Tile::TopLeft,
        Tile::TopRight,
        Tile::BottomLeft,
        Tile::BottomRight,
        Tile::LeftThird,
        Tile::CenterThird,
        Tile::RightThird,
        Tile::LeftTwoThirds,
        Tile::CenterTwoThirds,
        Tile::RightTwoThirds,
        Tile::TopTwoThirds,
        Tile::BottomTwoThirds,
    ];

    /// The part a person can choose that `part` is (fractions of the
    /// area: a window's tile, `WindowInfo::part`), if it is one: within
    /// a fiftieth each way (gaps, and edges dragged a little).
    pub fn of_part(part: [f32; 4]) -> Option<Tile> {
        Tile::CHOOSABLE.into_iter().find(|t| {
            let f = t.fraction();
            [f.0, f.1, f.2, f.3].iter().zip(part).all(|(a, b)| (a - b).abs() <= 0.02)
        })
    }

    /// Its name in menus: "Left Half", "Center Two Thirds".
    pub fn title(self) -> &'static str {
        match self {
            Tile::Left => "Left Half",
            Tile::Right => "Right Half",
            Tile::Top => "Top Half",
            Tile::Bottom => "Bottom Half",
            Tile::TopLeft => "Top Left Quarter",
            Tile::TopRight => "Top Right Quarter",
            Tile::BottomLeft => "Bottom Left Quarter",
            Tile::BottomRight => "Bottom Right Quarter",
            Tile::LeftThird => "Left Third",
            Tile::CenterThird => "Center Third",
            Tile::RightThird => "Right Third",
            Tile::LeftTwoThirds => "Left Two Thirds",
            Tile::CenterTwoThirds => "Center Two Thirds",
            Tile::RightTwoThirds => "Right Two Thirds",
            Tile::TopTwoThirds => "Top Two Thirds",
            Tile::BottomTwoThirds => "Bottom Two Thirds",
            Tile::Cell => "Cell",
        }
    }

    /// Its shortcut's id ("tile-left"); none for a cell.
    pub fn shortcut_id(self) -> Option<&'static str> {
        SHORTCUTS.iter().find(|s| s.action == Shortcut::Tile(self)).map(|s| s.id)
    }

    /// Main and Stack's parts (x, y, width, height, as fractions of the
    /// tiling area) with its main window here and `stack` more, as
    /// app_server lays them out until an edge is dragged: the main
    /// window's, then the stack's in order. Beside an edge, the rest is a
    /// column (or a row) of them each as big; in the middle, a column
    /// either side, the right one taking the first half (and the odd
    /// one); in a corner, the other half takes all but the last, which
    /// has the rest of the main window's column.
    pub fn main_and_stack_parts(self, stack: usize) -> Vec<(f32, f32, f32, f32)> {
        self.main_and_stack_parts_with(1, stack)
    }

    /// [`Tile::main_and_stack_parts`] with `mains` main windows (1 or
    /// more) sharing the main place: one above the other, or side by side
    /// in a place across the top or bottom ([`Tile::mains_side_by_side`]),
    /// each as big; theirs first, in order.
    pub fn main_and_stack_parts_with(self, mains: usize, stack: usize) -> Vec<(f32, f32, f32, f32)> {
        let (x, y, w, h) = self.fraction();
        let column = |(cx, cy, cw, ch): (f32, f32, f32, f32), n: usize| (0..n).map(move |i| (cx, cy + ch * i as f32 / n as f32, cw, ch / n as f32));
        let row = |(cx, cy, cw, ch): (f32, f32, f32, f32), n: usize| (0..n).map(move |i| (cx + cw * i as f32 / n as f32, cy, cw / n as f32, ch));
        let mut out: Vec<(f32, f32, f32, f32)> = if self.mains_side_by_side() { row((x, y, w, h), mains.max(1)).collect() } else { column((x, y, w, h), mains.max(1)).collect() };
        match self {
            Tile::Left | Tile::LeftThird | Tile::LeftTwoThirds | Tile::Cell => out.extend(column((w, 0.0, 1.0 - w, 1.0), stack)),
            Tile::Right | Tile::RightThird | Tile::RightTwoThirds => out.extend(column((0.0, 0.0, x, 1.0), stack)),
            Tile::Top | Tile::TopTwoThirds => out.extend(row((0.0, h, 1.0, 1.0 - h), stack)),
            Tile::Bottom | Tile::BottomTwoThirds => out.extend(row((0.0, 0.0, 1.0, y), stack)),
            Tile::CenterThird | Tile::CenterTwoThirds => {
                let right = stack.div_ceil(2);
                out.extend(column((x + w, 0.0, 1.0 - x - w, 1.0), right));
                out.extend(column((0.0, 0.0, x, 1.0), stack - right));
            }
            Tile::TopLeft | Tile::TopRight | Tile::BottomLeft | Tile::BottomRight => {
                let other = if x > 0.0 { (0.0, 0.0, x, 1.0) } else { (w, 0.0, 1.0 - w, 1.0) };
                let below = if y > 0.0 { (x, 0.0, w, y) } else { (x, h, w, 1.0 - h) };
                let n = if stack >= 2 { stack - 1 } else { stack };
                out.extend(column(other, n));
                if stack >= 2 {
                    out.push(below);
                }
            }
        }
        out
    }

    /// Whether Main and Stack's main windows here sit side by side (a
    /// place across the top or the bottom), not one above the other.
    pub fn mains_side_by_side(self) -> bool {
        matches!(self, Tile::Top | Tile::TopTwoThirds | Tile::Bottom | Tile::BottomTwoThirds)
    }

    /// Which group of the menus it is in (halves, quarters, thirds, two
    /// thirds), for separators between them.
    pub fn family(self) -> u8 {
        match self {
            Tile::Left | Tile::Right | Tile::Top | Tile::Bottom => 0,
            Tile::TopLeft | Tile::TopRight | Tile::BottomLeft | Tile::BottomRight => 1,
            Tile::LeftThird | Tile::CenterThird | Tile::RightThird => 2,
            Tile::LeftTwoThirds | Tile::CenterTwoThirds | Tile::RightTwoThirds | Tile::TopTwoThirds | Tile::BottomTwoThirds => 3,
            Tile::Cell => 4,
        }
    }
}

/// The platform a keyboard profile follows, so its keys are as there:
/// Mac, Windows or BeOS (as Haiku keeps it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    #[default]
    Mac,
    Windows,
    Beos,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::Mac, Platform::Windows, Platform::Beos];

    pub fn name(self) -> &'static str {
        match self {
            Platform::Mac => "Mac",
            Platform::Windows => "Windows",
            Platform::Beos => "BeOS",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The key apps take as the command key (Copy is it and C): on a Mac ⌘
/// (the Super key), on Windows Ctrl, on BeOS Alt (the key beside the
/// space bar, as a Mac's ⌘ is). Whichever it is, apps see Ctrl; with Alt
/// as it, the Super key is Option (as on BeOS).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKey {
    #[default]
    Command,
    Control,
    Alt,
}

impl CommandKey {
    pub const ALL: [CommandKey; 3] = [CommandKey::Command, CommandKey::Control, CommandKey::Alt];

    /// As Preferences offers it.
    pub fn title(self) -> &'static str {
        match self {
            CommandKey::Command => "⌘ Command (the Super key)",
            CommandKey::Control => "Control",
            CommandKey::Alt => "Alt",
        }
    }
}

/// A keyboard profile: how keys behave and what every shortcut is, as on
/// one platform (Preferences ▸ Keyboard). Mac, Windows and BeOS come as
/// they are on those; a profile changed by hand is "Mac (Customized)"
/// (or keeps the name it was saved under).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardProfile {
    pub name: String,
    /// Whose keys it starts from (its shortcuts' defaults).
    pub platform: Platform,
    pub command_key: CommandKey,
    /// Caps Lock is another Ctrl.
    pub caps_lock_control: bool,
    /// Tapping the Super key alone (⌘ on a Mac, the Windows key) opens
    /// the command palette, as the Windows key opens Start.
    pub palette_on_tap: bool,
    /// Shortcuts changed from the platform's, by id (app_server's, in
    /// [`SHORTCUTS`], and apps', in [`crate::app_keys::APP_KEYS`]); an empty
    /// list turns one off.
    pub shortcuts: ShortcutSettings,
}

impl Default for KeyboardProfile {
    fn default() -> Self {
        KeyboardProfile::builtin(Platform::Mac)
    }
}

impl KeyboardProfile {
    /// A platform's own, as it comes.
    pub fn builtin(platform: Platform) -> KeyboardProfile {
        let (command_key, caps_lock_control, palette_on_tap) = match platform {
            Platform::Mac => (CommandKey::Command, true, false),
            Platform::Windows => (CommandKey::Control, false, true),
            Platform::Beos => (CommandKey::Alt, false, false),
        };
        KeyboardProfile { name: platform.name().into(), platform, command_key, caps_lock_control, palette_on_tap, shortcuts: ShortcutSettings::new() }
    }

    /// Its platform's, just as it comes (whatever its name).
    pub fn is_builtin(&self) -> bool {
        KeyboardProfile { name: self.platform.name().into(), ..self.clone() } == KeyboardProfile::builtin(self.platform)
    }

    /// The name a platform's profile has once changed: "Mac (Customized)".
    pub fn customized_name(platform: Platform) -> String {
        format!("{} (Customized)", platform.name())
    }

    /// Changed by hand: a platform's own becomes "Mac (Customized)", and
    /// back to "Mac" when it is as it comes again; one saved under a name
    /// of its own keeps it.
    pub fn changed(&mut self) {
        let named_for = |p: Platform| self.name == p.name() || self.name == KeyboardProfile::customized_name(p);
        if Platform::ALL.into_iter().any(named_for) {
            self.name = if self.is_builtin() { self.platform.name().into() } else { KeyboardProfile::customized_name(self.platform) };
        }
    }

    /// The Super key is Option (the command key being Alt, as on BeOS).
    pub fn super_is_option(&self) -> bool {
        self.command_key == CommandKey::Alt
    }
}

/// One shortcut as Preferences lists it.
pub struct ShortcutInfo {
    pub action: Shortcut,
    /// Its name in a profile's `shortcuts`.
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    /// Its keys on each platform: Mac, Windows, BeOS. "Cmd" is the
    /// Super key (⌘ on a Mac, the Windows key, BeOS's Option); keys are
    /// the physical ones.
    pub keys: [&'static [&'static str]; 3],
}

impl ShortcutInfo {
    /// Its keys on `platform`, as it comes.
    pub fn default_keys(&self, platform: Platform) -> Vec<Chord> {
        self.keys[platform.index()].iter().filter_map(|k| k.parse().ok()).collect()
    }
}

const fn s(
    action: Shortcut,
    id: &'static str,
    group: &'static str,
    title: &'static str,
    mac: &'static [&'static str],
    windows: &'static [&'static str],
    beos: &'static [&'static str],
) -> ShortcutInfo {
    ShortcutInfo { action, id, group, title, keys: [mac, windows, beos] }
}

pub const WINDOWS: &str = "Windows";
pub const ARRANGE: &str = "Arranging";
pub const WORKSPACES: &str = "Workspaces";
pub const SWITCHING: &str = "Switching";
pub const DESKTOP: &str = "Desktop";
pub const SCREENSHOTS: &str = "Screenshots";

/// Every shortcut app_server handles, in the order Preferences shows them,
/// with its keys on each platform. Mac's tiling is Rectangle's (⌃⌥), so
/// ⇧⌘-arrows select text as on any Mac; its spaces are Mission
/// Control's (⌃←/→, ⌃1…). Windows' is Snap and Task View's (the Windows
/// key with the arrows, ⊞Tab), Olo's extras beside them on ⊞⌥. BeOS's is
/// Haiku's (Ctrl+Alt for windows, Alt+F1… workspaces, Ctrl+Tab
/// Twitcher, Opt+Tab a window's tabs), Olo's extras on Ctrl+Opt.
pub const SHORTCUTS: &[ShortcutInfo] = &[
    s(Shortcut::Tile(Tile::Left), "tile-left", WINDOWS, "Left half", &["Ctrl+Alt+Left"], &["Cmd+Left"], &["Ctrl+Cmd+Left"]),
    s(Shortcut::Tile(Tile::Right), "tile-right", WINDOWS, "Right half", &["Ctrl+Alt+Right"], &["Cmd+Right"], &["Ctrl+Cmd+Right"]),
    s(Shortcut::Tile(Tile::Top), "tile-top", WINDOWS, "Top half", &["Ctrl+Alt+Up"], &["Alt+Cmd+Up"], &["Ctrl+Cmd+Up"]),
    s(Shortcut::Tile(Tile::Bottom), "tile-bottom", WINDOWS, "Bottom half", &["Ctrl+Alt+Down"], &["Alt+Cmd+Down"], &["Ctrl+Cmd+Down"]),
    s(Shortcut::Tile(Tile::TopLeft), "tile-top-left", WINDOWS, "Top left quarter", &["Ctrl+Alt+U"], &["Alt+Cmd+U"], &["Ctrl+Cmd+U"]),
    s(Shortcut::Tile(Tile::TopRight), "tile-top-right", WINDOWS, "Top right quarter", &["Ctrl+Alt+I"], &["Alt+Cmd+I"], &["Ctrl+Cmd+I"]),
    s(Shortcut::Tile(Tile::BottomLeft), "tile-bottom-left", WINDOWS, "Bottom left quarter", &["Ctrl+Alt+J"], &["Alt+Cmd+J"], &["Ctrl+Cmd+J"]),
    s(Shortcut::Tile(Tile::BottomRight), "tile-bottom-right", WINDOWS, "Bottom right quarter", &["Ctrl+Alt+K"], &["Alt+Cmd+K"], &["Ctrl+Cmd+K"]),
    s(Shortcut::Tile(Tile::LeftThird), "tile-left-third", WINDOWS, "Left third", &["Ctrl+Alt+D"], &["Alt+Cmd+D"], &["Ctrl+Cmd+D"]),
    s(Shortcut::Tile(Tile::CenterThird), "tile-center-third", WINDOWS, "Center third", &["Ctrl+Alt+F"], &["Alt+Cmd+F"], &["Ctrl+Cmd+F"]),
    s(Shortcut::Tile(Tile::RightThird), "tile-right-third", WINDOWS, "Right third", &["Ctrl+Alt+G"], &["Alt+Cmd+G"], &["Ctrl+Cmd+G"]),
    s(Shortcut::Tile(Tile::LeftTwoThirds), "tile-left-two-thirds", WINDOWS, "Left two thirds", &["Ctrl+Alt+E"], &["Alt+Cmd+E"], &["Ctrl+Cmd+E"]),
    s(Shortcut::Tile(Tile::CenterTwoThirds), "tile-center-two-thirds", WINDOWS, "Center two thirds", &["Ctrl+Alt+R"], &["Alt+Cmd+W"], &["Ctrl+Cmd+R"]),
    s(Shortcut::Tile(Tile::RightTwoThirds), "tile-right-two-thirds", WINDOWS, "Right two thirds", &["Ctrl+Alt+T"], &["Alt+Cmd+R"], &["Ctrl+Cmd+T"]),
    s(Shortcut::Tile(Tile::TopTwoThirds), "tile-top-two-thirds", WINDOWS, "Top two thirds", &["Ctrl+Alt+Y"], &["Alt+Cmd+Y"], &["Ctrl+Cmd+Y"]),
    s(Shortcut::Tile(Tile::BottomTwoThirds), "tile-bottom-two-thirds", WINDOWS, "Bottom two thirds", &["Ctrl+Alt+B"], &["Alt+Cmd+B"], &["Ctrl+Cmd+B"]),
    s(Shortcut::Zoom, "zoom", WINDOWS, "Zoom or unzoom", &["Ctrl+Alt+Return"], &["Cmd+Up"], &["Ctrl+Alt+Z"]),
    s(Shortcut::ZoomVertical, "zoom-vertical", WINDOWS, "Zoom vertically", &["Ctrl+Alt+Shift+Up", "Ctrl+Alt+Shift+Down"], &["Shift+Cmd+Up"], &["Ctrl+Shift+Cmd+Up", "Ctrl+Shift+Cmd+Down"]),
    s(Shortcut::ZoomHorizontal, "zoom-horizontal", WINDOWS, "Zoom horizontally", &["Ctrl+Alt+Shift+Left", "Ctrl+Alt+Shift+Right"], &[], &["Ctrl+Shift+Cmd+Left", "Ctrl+Shift+Cmd+Right"]),
    s(Shortcut::Restore, "restore", WINDOWS, "Back to its own size", &["Ctrl+Alt+Backspace"], &[], &["Ctrl+Cmd+Backspace"]),
    s(Shortcut::RestoreOrHide, "restore-or-hide", WINDOWS, "Back to its own size, or hide", &[], &["Cmd+Down"], &[]),
    s(Shortcut::Center, "center", WINDOWS, "Center", &["Ctrl+Alt+C"], &["Alt+Cmd+C"], &["Ctrl+Cmd+C"]),
    s(Shortcut::Float, "float-window", WINDOWS, "Float over the tiles, or tile", &["Ctrl+Alt+Cmd+F"], &["Ctrl+Alt+Cmd+F"], &["Ctrl+Alt+Cmd+F"]),
    s(Shortcut::MakeMain, "make-main", WINDOWS, "Make it the main window", &["Ctrl+Alt+Cmd+M"], &["Ctrl+Alt+Cmd+M"], &["Ctrl+Alt+Cmd+M"]),
    s(Shortcut::PinMain, "pin-main", WINDOWS, "Pin it as the main window, or let it go", &["Ctrl+Alt+Shift+Cmd+M"], &["Ctrl+Alt+Shift+Cmd+M"], &["Ctrl+Alt+Shift+Cmd+M"]),
    s(Shortcut::SwapMain, "swap-main", WINDOWS, "Swap with the previous main window", &["Ctrl+Alt+Cmd+Backspace"], &["Ctrl+Alt+Cmd+Backspace"], &["Ctrl+Alt+Cmd+Backspace"]),
    // By direction, as tiling window managers do it (vim's H J K L: i3,
    // sway, bspwm, Hyprland, Amethyst), the same keys in every profile.
    s(Shortcut::Focus(Dir::Left), "focus-left", WINDOWS, "Focus the window to the left", &["Ctrl+Alt+Cmd+H"], &["Ctrl+Alt+Cmd+H"], &["Ctrl+Alt+Cmd+H"]),
    s(Shortcut::Focus(Dir::Down), "focus-down", WINDOWS, "Focus the window below", &["Ctrl+Alt+Cmd+J"], &["Ctrl+Alt+Cmd+J"], &["Ctrl+Alt+Cmd+J"]),
    s(Shortcut::Focus(Dir::Up), "focus-up", WINDOWS, "Focus the window above", &["Ctrl+Alt+Cmd+K"], &["Ctrl+Alt+Cmd+K"], &["Ctrl+Alt+Cmd+K"]),
    s(Shortcut::Focus(Dir::Right), "focus-right", WINDOWS, "Focus the window to the right", &["Ctrl+Alt+Cmd+L"], &["Ctrl+Alt+Cmd+L"], &["Ctrl+Alt+Cmd+L"]),
    s(Shortcut::Swap(Dir::Left), "swap-left", WINDOWS, "Swap with the tile to the left", &["Ctrl+Alt+Shift+Cmd+H"], &["Ctrl+Alt+Shift+Cmd+H"], &["Ctrl+Alt+Shift+Cmd+H"]),
    s(Shortcut::Swap(Dir::Down), "swap-down", WINDOWS, "Swap with the tile below", &["Ctrl+Alt+Shift+Cmd+J"], &["Ctrl+Alt+Shift+Cmd+J"], &["Ctrl+Alt+Shift+Cmd+J"]),
    s(Shortcut::Swap(Dir::Up), "swap-up", WINDOWS, "Swap with the tile above", &["Ctrl+Alt+Shift+Cmd+K"], &["Ctrl+Alt+Shift+Cmd+K"], &["Ctrl+Alt+Shift+Cmd+K"]),
    s(Shortcut::Swap(Dir::Right), "swap-right", WINDOWS, "Swap with the tile to the right", &["Ctrl+Alt+Shift+Cmd+L"], &["Ctrl+Alt+Shift+Cmd+L"], &["Ctrl+Alt+Shift+Cmd+L"]),
    s(Shortcut::Resize(Dir::Left), "resize-left", WINDOWS, "Move the tile's edge left", &["Ctrl+Alt+Shift+Cmd+Left"], &["Ctrl+Alt+Shift+Cmd+Left"], &["Ctrl+Alt+Shift+Cmd+Left"]),
    s(Shortcut::Resize(Dir::Right), "resize-right", WINDOWS, "Move the tile's edge right", &["Ctrl+Alt+Shift+Cmd+Right"], &["Ctrl+Alt+Shift+Cmd+Right"], &["Ctrl+Alt+Shift+Cmd+Right"]),
    s(Shortcut::Resize(Dir::Up), "resize-up", WINDOWS, "Move the tile's edge up", &["Ctrl+Alt+Shift+Cmd+Up"], &["Ctrl+Alt+Shift+Cmd+Up"], &["Ctrl+Alt+Shift+Cmd+Up"]),
    s(Shortcut::Resize(Dir::Down), "resize-down", WINDOWS, "Move the tile's edge down", &["Ctrl+Alt+Shift+Cmd+Down"], &["Ctrl+Alt+Shift+Cmd+Down"], &["Ctrl+Alt+Shift+Cmd+Down"]),
    s(Shortcut::NextLayout, "next-layout", ARRANGE, "Next auto-tiling layout", &["Ctrl+Alt+Cmd+Space"], &["Ctrl+Alt+Cmd+Space"], &["Ctrl+Alt+Cmd+Space"]),
    s(Shortcut::PreviousLayout, "previous-layout", ARRANGE, "Previous auto-tiling layout", &["Ctrl+Alt+Shift+Cmd+Space"], &["Ctrl+Alt+Shift+Cmd+Space"], &["Ctrl+Alt+Shift+Cmd+Space"]),
    s(Shortcut::RotateTiles(true), "rotate-tiles", ARRANGE, "Rotate the tiles", &["Ctrl+Alt+Cmd+R"], &["Ctrl+Alt+Cmd+R"], &["Ctrl+Alt+Cmd+R"]),
    s(Shortcut::RotateTiles(false), "rotate-tiles-back", ARRANGE, "Rotate the tiles back", &["Ctrl+Alt+Shift+Cmd+R"], &["Ctrl+Alt+Shift+Cmd+R"], &["Ctrl+Alt+Shift+Cmd+R"]),
    s(Shortcut::FlipTiles(true), "flip-tiles-across", ARRANGE, "Flip the tiles left and right", &["Ctrl+Alt+Cmd+X"], &["Ctrl+Alt+Cmd+X"], &["Ctrl+Alt+Cmd+X"]),
    s(Shortcut::FlipTiles(false), "flip-tiles-up-down", ARRANGE, "Flip the tiles top and bottom", &["Ctrl+Alt+Cmd+Y"], &["Ctrl+Alt+Cmd+Y"], &["Ctrl+Alt+Cmd+Y"]),
    s(Shortcut::EvenOutTiles, "even-out-tiles", ARRANGE, "Even out the tiles", &["Ctrl+Alt+Cmd+Equal"], &["Ctrl+Alt+Cmd+Equal"], &["Ctrl+Alt+Cmd+Equal"]),
    s(Shortcut::ToggleSplit, "toggle-split", WINDOWS, "Turn the tile's split", &["Ctrl+Alt+Cmd+S"], &["Ctrl+Alt+Cmd+S"], &["Ctrl+Alt+Cmd+S"]),
    s(Shortcut::MoreGap, "more-gap", ARRANGE, "More space between windows", &["Ctrl+Alt+Cmd+BracketRight"], &["Ctrl+Alt+Cmd+BracketRight"], &["Ctrl+Alt+Cmd+BracketRight"]),
    s(Shortcut::LessGap, "less-gap", ARRANGE, "Less space between windows", &["Ctrl+Alt+Cmd+BracketLeft"], &["Ctrl+Alt+Cmd+BracketLeft"], &["Ctrl+Alt+Cmd+BracketLeft"]),
    s(Shortcut::MoreMains, "more-main-windows", ARRANGE, "More main windows", &["Ctrl+Alt+Cmd+I"], &["Ctrl+Alt+Cmd+I"], &["Ctrl+Alt+Cmd+I"]),
    s(Shortcut::FewerMains, "fewer-main-windows", ARRANGE, "Fewer main windows", &["Ctrl+Alt+Cmd+D"], &["Ctrl+Alt+Cmd+D"], &["Ctrl+Alt+Cmd+D"]),
    s(Shortcut::BiggerMain, "bigger-main", ARRANGE, "Make the main windows bigger", &["Ctrl+Alt+Cmd+Period"], &["Ctrl+Alt+Cmd+Period"], &["Ctrl+Alt+Cmd+Period"]),
    s(Shortcut::SmallerMain, "smaller-main", ARRANGE, "Make the main windows smaller", &["Ctrl+Alt+Cmd+Comma"], &["Ctrl+Alt+Cmd+Comma"], &["Ctrl+Alt+Cmd+Comma"]),
    s(Shortcut::AllWorkspaces, "all-workspaces", WINDOWS, "Show on all workspaces", &["Ctrl+Alt+Cmd+A"], &["Ctrl+Alt+Cmd+A"], &["Ctrl+Alt+Cmd+A"]),
    s(Shortcut::Preselect(Dir::Left), "preselect-left", WINDOWS, "Next window to the tile's left", &["Ctrl+Shift+Cmd+H"], &["Ctrl+Shift+Cmd+H"], &["Ctrl+Shift+Cmd+H"]),
    s(Shortcut::Preselect(Dir::Down), "preselect-down", WINDOWS, "Next window below the tile", &["Ctrl+Shift+Cmd+J"], &["Ctrl+Shift+Cmd+J"], &["Ctrl+Shift+Cmd+J"]),
    s(Shortcut::Preselect(Dir::Up), "preselect-up", WINDOWS, "Next window above the tile", &["Ctrl+Shift+Cmd+K"], &["Ctrl+Shift+Cmd+K"], &["Ctrl+Shift+Cmd+K"]),
    s(Shortcut::Preselect(Dir::Right), "preselect-right", WINDOWS, "Next window to the tile's right", &["Ctrl+Shift+Cmd+L"], &["Ctrl+Shift+Cmd+L"], &["Ctrl+Shift+Cmd+L"]),
    s(Shortcut::FocusInOrder(true), "focus-next-tile", WINDOWS, "Focus the next tile", &["Ctrl+Alt+Cmd+N"], &["Ctrl+Alt+Cmd+N"], &["Ctrl+Alt+Cmd+N"]),
    s(Shortcut::FocusInOrder(false), "focus-previous-tile", WINDOWS, "Focus the previous tile", &["Ctrl+Alt+Cmd+P"], &["Ctrl+Alt+Cmd+P"], &["Ctrl+Alt+Cmd+P"]),
    s(Shortcut::SwapInOrder(true), "swap-next-tile", WINDOWS, "Swap with the next tile", &["Ctrl+Alt+Shift+Cmd+N"], &["Ctrl+Alt+Shift+Cmd+N"], &["Ctrl+Alt+Shift+Cmd+N"]),
    s(Shortcut::SwapInOrder(false), "swap-previous-tile", WINDOWS, "Swap with the previous tile", &["Ctrl+Alt+Shift+Cmd+P"], &["Ctrl+Alt+Shift+Cmd+P"], &["Ctrl+Alt+Shift+Cmd+P"]),
    s(Shortcut::ColumnWidth, "column-width", WINDOWS, "Next column width (Scrolling)", &["Ctrl+Alt+Cmd+W"], &["Ctrl+Alt+Cmd+W"], &["Ctrl+Alt+Cmd+W"]),
    s(Shortcut::RotateWindows(true), "rotate-windows", ARRANGE, "Move every tile's window on one", &["Ctrl+Alt+Shift+Cmd+Period"], &["Ctrl+Alt+Shift+Cmd+Period"], &["Ctrl+Alt+Shift+Cmd+Period"]),
    s(Shortcut::RotateWindows(false), "rotate-windows-back", ARRANGE, "Move every tile's window back one", &["Ctrl+Alt+Shift+Cmd+Comma"], &["Ctrl+Alt+Shift+Cmd+Comma"], &["Ctrl+Alt+Shift+Cmd+Comma"]),
    s(Shortcut::ToScratchpad, "to-scratchpad", WINDOWS, "Move to the scratchpad", &["Ctrl+Alt+Shift+Cmd+Minus"], &["Ctrl+Alt+Shift+Cmd+Minus"], &["Ctrl+Alt+Shift+Cmd+Minus"]),
    s(Shortcut::Scratchpad, "scratchpad", ARRANGE, "Show or hide the scratchpad", &["Ctrl+Alt+Cmd+Minus"], &["Ctrl+Alt+Cmd+Minus"], &["Ctrl+Alt+Cmd+Minus"]),
    s(Shortcut::Hide, "hide-window", WINDOWS, "Hide window", &["Cmd+M"], &[], &["Ctrl+Alt+M"]),
    s(Shortcut::HideApp, "hide-app", WINDOWS, "Hide the application", &["Cmd+H"], &[], &["Ctrl+Alt+H"]),
    s(Shortcut::RollUp, "roll-up", WINDOWS, "Roll up or down", &["Ctrl+Cmd+M"], &["Alt+Cmd+M"], &["Ctrl+Alt+R"]),
    s(Shortcut::SendToBack, "send-to-back", WINDOWS, "Send to back", &[], &[], &["Ctrl+Alt+B"]),
    s(Shortcut::CloseWindow, "close-window", WINDOWS, "Close the window", &[], &["Alt+F4"], &[]),
    s(Shortcut::NextTab, "next-tab", WINDOWS, "Next tab in the window", &["Ctrl+Tab"], &["Ctrl+Tab"], &["Cmd+Tab"]),
    s(Shortcut::PreviousTab, "previous-tab", WINDOWS, "Previous tab in the window", &["Ctrl+Shift+Tab"], &["Ctrl+Shift+Tab"], &["Shift+Cmd+Tab"]),
    s(Shortcut::NextDisplay, "next-display", WINDOWS, "To the next display", &["Ctrl+Alt+Cmd+Right"], &["Shift+Cmd+Right"], &["Ctrl+Alt+Cmd+Right"]),
    s(Shortcut::PreviousDisplay, "previous-display", WINDOWS, "To the previous display", &["Ctrl+Alt+Cmd+Left"], &["Shift+Cmd+Left"], &["Ctrl+Alt+Cmd+Left"]),
    s(Shortcut::TileAll, "tile-all", ARRANGE, "Tile all windows on the display", &["Ctrl+Cmd+Return"], &["Alt+Cmd+Return"], &["Ctrl+Cmd+Return"]),
    s(Shortcut::AutoTile, "auto-tiling", ARRANGE, "Auto-tiling on or off", &["Ctrl+Alt+Cmd+T"], &["Alt+Cmd+T"], &["Ctrl+Alt+Cmd+T"]),
    s(Shortcut::FocusMode, "focus-mode", ARRANGE, "Focus mode on or off", &["Ctrl+Alt+Cmd+O"], &["Alt+Cmd+O"], &["Ctrl+Alt+Cmd+O"]),
    s(Shortcut::ZoomAll, "zoom-all", ARRANGE, "Zoom or unzoom all windows on the display", &["Ctrl+Alt+Cmd+Return"], &["Ctrl+Alt+Cmd+Return"], &["Ctrl+Alt+Cmd+Return"]),
    s(Shortcut::CascadeAll, "cascade-all", ARRANGE, "Cascade all windows on the display", &["Ctrl+Cmd+Backslash"], &["Alt+Cmd+Backslash"], &["Ctrl+Cmd+Backslash"]),
    s(Shortcut::FanOut, "fan-out", ARRANGE, "Fan out all windows, tabs side by side", &["Ctrl+Cmd+Slash"], &["Alt+Cmd+Slash"], &["Ctrl+Cmd+Slash"]),
    s(Shortcut::CascadeOthers, "cascade-others", ARRANGE, "Cascade the other windows, leaving this one", &["Ctrl+Shift+Cmd+Backslash"], &["Alt+Shift+Cmd+Backslash"], &["Ctrl+Shift+Cmd+Backslash"]),
    s(Shortcut::FanOutOthers, "fan-out-others", ARRANGE, "Fan out the other windows, leaving this one", &["Ctrl+Shift+Cmd+Slash"], &["Alt+Shift+Cmd+Slash"], &["Ctrl+Shift+Cmd+Slash"]),
    s(Shortcut::UndoArrangement, "undo-arrangement", ARRANGE, "Undo window arrangement", &["Ctrl+Cmd+Z"], &["Alt+Cmd+Z"], &["Ctrl+Cmd+Z"]),
    s(Shortcut::RedoArrangement, "redo-arrangement", ARRANGE, "Redo window arrangement", &["Ctrl+Shift+Cmd+Z"], &["Alt+Shift+Cmd+Z"], &["Ctrl+Shift+Cmd+Z"]),
    s(Shortcut::ShowDesktop, "show-desktop", ARRANGE, "Show the desktop", &["Cmd+F3"], &["Cmd+D"], &["Ctrl+Alt+D"]),
    s(Shortcut::NextWorkspace, "next-workspace", WORKSPACES, "Next workspace", &["Ctrl+Right"], &["Ctrl+Cmd+Right"], &["Ctrl+Alt+Right"]),
    s(Shortcut::PreviousWorkspace, "previous-workspace", WORKSPACES, "Previous workspace", &["Ctrl+Left"], &["Ctrl+Cmd+Left"], &["Ctrl+Alt+Left"]),
    s(Shortcut::WindowToNextWorkspace, "window-to-next-workspace", WORKSPACES, "Window to the next workspace", &["Ctrl+Shift+Right"], &["Ctrl+Shift+Cmd+Right"], &["Ctrl+Alt+Shift+Right"]),
    s(Shortcut::WindowToPreviousWorkspace, "window-to-previous-workspace", WORKSPACES, "Window to the previous workspace", &["Ctrl+Shift+Left"], &["Ctrl+Shift+Cmd+Left"], &["Ctrl+Alt+Shift+Left"]),
    s(Shortcut::Workspace(0), "workspace-1", WORKSPACES, "Workspace 1", &["Ctrl+1"], &["Ctrl+Cmd+1"], &["Alt+F1"]),
    s(Shortcut::Workspace(1), "workspace-2", WORKSPACES, "Workspace 2", &["Ctrl+2"], &["Ctrl+Cmd+2"], &["Alt+F2"]),
    s(Shortcut::Workspace(2), "workspace-3", WORKSPACES, "Workspace 3", &["Ctrl+3"], &["Ctrl+Cmd+3"], &["Alt+F3"]),
    s(Shortcut::Workspace(3), "workspace-4", WORKSPACES, "Workspace 4", &["Ctrl+4"], &["Ctrl+Cmd+4"], &["Alt+F4"]),
    s(Shortcut::Workspace(4), "workspace-5", WORKSPACES, "Workspace 5", &["Ctrl+5"], &["Ctrl+Cmd+5"], &["Alt+F5"]),
    s(Shortcut::Workspace(5), "workspace-6", WORKSPACES, "Workspace 6", &["Ctrl+6"], &["Ctrl+Cmd+6"], &["Alt+F6"]),
    s(Shortcut::Workspace(6), "workspace-7", WORKSPACES, "Workspace 7", &["Ctrl+7"], &["Ctrl+Cmd+7"], &["Alt+F7"]),
    s(Shortcut::Workspace(7), "workspace-8", WORKSPACES, "Workspace 8", &["Ctrl+8"], &["Ctrl+Cmd+8"], &["Alt+F8"]),
    s(Shortcut::Workspace(8), "workspace-9", WORKSPACES, "Workspace 9", &["Ctrl+9"], &["Ctrl+Cmd+9"], &["Alt+F9"]),
    s(Shortcut::WindowToWorkspace(0), "window-to-workspace-1", WORKSPACES, "Window to workspace 1", &["Ctrl+Shift+1"], &["Ctrl+Shift+Cmd+1"], &["Alt+Shift+F1"]),
    s(Shortcut::WindowToWorkspace(1), "window-to-workspace-2", WORKSPACES, "Window to workspace 2", &["Ctrl+Shift+2"], &["Ctrl+Shift+Cmd+2"], &["Alt+Shift+F2"]),
    s(Shortcut::WindowToWorkspace(2), "window-to-workspace-3", WORKSPACES, "Window to workspace 3", &["Ctrl+Shift+3"], &["Ctrl+Shift+Cmd+3"], &["Alt+Shift+F3"]),
    s(Shortcut::WindowToWorkspace(3), "window-to-workspace-4", WORKSPACES, "Window to workspace 4", &["Ctrl+Shift+4"], &["Ctrl+Shift+Cmd+4"], &["Alt+Shift+F4"]),
    s(Shortcut::WindowToWorkspace(4), "window-to-workspace-5", WORKSPACES, "Window to workspace 5", &["Ctrl+Shift+5"], &["Ctrl+Shift+Cmd+5"], &["Alt+Shift+F5"]),
    s(Shortcut::WindowToWorkspace(5), "window-to-workspace-6", WORKSPACES, "Window to workspace 6", &["Ctrl+Shift+6"], &["Ctrl+Shift+Cmd+6"], &["Alt+Shift+F6"]),
    s(Shortcut::WindowToWorkspace(6), "window-to-workspace-7", WORKSPACES, "Window to workspace 7", &["Ctrl+Shift+7"], &["Ctrl+Shift+Cmd+7"], &["Alt+Shift+F7"]),
    s(Shortcut::WindowToWorkspace(7), "window-to-workspace-8", WORKSPACES, "Window to workspace 8", &["Ctrl+Shift+8"], &["Ctrl+Shift+Cmd+8"], &["Alt+Shift+F8"]),
    s(Shortcut::WindowToWorkspace(8), "window-to-workspace-9", WORKSPACES, "Window to workspace 9", &["Ctrl+Shift+9"], &["Ctrl+Shift+Cmd+9"], &["Alt+Shift+F9"]),
    s(Shortcut::SwitchApps, "switch-apps", SWITCHING, "Switch applications (Twitcher)", &["Cmd+Tab"], &["Alt+Tab"], &["Ctrl+Tab"]),
    s(Shortcut::SwitchAppsHere, "switch-apps-here", SWITCHING, "Switch applications on the workspaces showing", &["Alt+Tab"], &[], &[]),
    s(Shortcut::SwitchWindows, "switch-windows", SWITCHING, "Switch windows of the application", &["Cmd+Grave"], &["Alt+Grave"], &["Ctrl+Grave"]),
    s(Shortcut::Overview, "overview", SWITCHING, "Overview of every window", &["Ctrl+Up"], &["Cmd+Tab"], &["Cmd+Space"]),
    s(Shortcut::Palette, "palette", DESKTOP, "Command palette", &["Cmd+Space"], &["Cmd+S"], &["Alt+Space"]),
    s(Shortcut::Find, "find", DESKTOP, "Find", &["Alt+Cmd+Space"], &["Cmd+F"], &["Ctrl+Alt+F"]),
    s(Shortcut::EmojiPicker, "emoji-picker", DESKTOP, "Emoji & symbols", &["Ctrl+Cmd+Space"], &["Cmd+Period"], &["Ctrl+Cmd+Space"]),
    s(Shortcut::ShortcutHelp, "shortcut-help", DESKTOP, "Keyboard shortcuts", &["Shift+Cmd+Slash"], &["Shift+Cmd+Slash"], &["Alt+Shift+Slash"]),
    s(Shortcut::ActivityMonitor, "activity-monitor", DESKTOP, "Activity Monitor", &["Alt+Cmd+Escape"], &["Ctrl+Shift+Escape"], &["Ctrl+Alt+Delete"]),
    s(Shortcut::Lock, "lock", DESKTOP, "Lock the screen", &["Ctrl+Cmd+Q"], &["Cmd+L"], &["Ctrl+Alt+L"]),
    s(Shortcut::Caffeinate, "caffeinate", DESKTOP, "Stay awake, or not", &["Ctrl+Cmd+A"], &["Alt+Cmd+A"], &["Ctrl+Alt+A"]),
    // (BeOS's ⌃⌘D is its left third: ⌥⌘D there.)
    s(Shortcut::ToggleDeskbar, "toggle-deskbar", DESKTOP, "Hide or show Deskbar", &["Ctrl+Cmd+D"], &["Ctrl+Cmd+D"], &["Alt+Cmd+D"]),
    s(Shortcut::ScreenshotScreen, "screenshot-screen", SCREENSHOTS, "Screenshot of the screen", &["Shift+Cmd+3", "Print"], &["Cmd+Print"], &["Shift+Print"]),
    s(Shortcut::ScreenshotRegion, "screenshot-region", SCREENSHOTS, "Screenshot of an area", &["Shift+Cmd+4"], &["Shift+Cmd+S", "Print"], &["Ctrl+Print"]),
    s(Shortcut::ScreenshotWindow, "screenshot-window", SCREENSHOTS, "Screenshot of the window", &["Alt+Print"], &["Alt+Print"], &["Alt+Print"]),
    s(Shortcut::ScreenshotApp, "screenshot-app", SCREENSHOTS, "Screenshot app", &["Shift+Cmd+5"], &["Shift+Cmd+R"], &["Print"]),
    s(Shortcut::ScreenRecording, "screen-recording", SCREENSHOTS, "Record the screen, or stop", &["Shift+Cmd+6"], &["Ctrl+Alt+Print"], &["Ctrl+Alt+Print"]),
];

/// The shortcut with this id.
pub fn by_id(id: &str) -> Option<&'static ShortcutInfo> {
    SHORTCUTS.iter().find(|i| i.id == id)
}

/// The groups, in order.
pub const GROUPS: &[&str] = &[WINDOWS, ARRANGE, WORKSPACES, SWITCHING, DESKTOP, SCREENSHOTS];

/// Keys changed from a platform's, by shortcut id; an empty list turns a
/// shortcut off. Ids not listed keep their platform's keys.
pub type ShortcutSettings = BTreeMap<String, Vec<String>>;

/// The keys of a shortcut in `profile`.
pub fn keys_for(info: &ShortcutInfo, profile: &KeyboardProfile) -> Vec<Chord> {
    match profile.shortcuts.get(info.id) {
        Some(keys) => keys.iter().filter_map(|k| k.parse().ok()).collect(),
        None => info.default_keys(profile.platform),
    }
}

/// Every chord and what it does in `profile` (the first shortcut wins a
/// clash).
pub fn bindings(profile: &KeyboardProfile) -> Vec<(Chord, Shortcut)> {
    let mut out: Vec<(Chord, Shortcut)> = Vec::new();
    for info in SHORTCUTS {
        for c in keys_for(info, profile) {
            if !out.iter().any(|(o, _)| *o == c) {
                out.push((c, info.action));
            }
        }
    }
    out
}

/// Any shortcut, app_server's or an app's, as Preferences and the
/// shortcuts sheet list them.
#[derive(Clone, Copy)]
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    /// None: app_server's (taken first, whatever has the keyboard).
    /// `Some(None)`: every app's. `Some(Some(app))`: one app's.
    pub scope: Option<Option<&'static str>>,
    defaults: [&'static [&'static str]; 3],
}

impl Entry {
    pub fn default_keys(&self, platform: Platform) -> Vec<Chord> {
        self.defaults[platform.index()].iter().filter_map(|k| k.parse().ok()).collect()
    }

    /// Its keys in `profile`.
    pub fn keys(&self, profile: &KeyboardProfile) -> Vec<Chord> {
        match profile.shortcuts.get(self.id) {
            Some(keys) => keys.iter().filter_map(|k| k.parse().ok()).collect(),
            None => self.default_keys(profile.platform),
        }
    }

    /// Two can't have the same keys: app_server's take them from every
    /// app; an app's clash with the same app's (every app's with each
    /// other). An app's own may have a key every app's has: it is that
    /// app's choice (Passwords' ⌘C copies the username).
    fn clashes_with(&self, other: &Entry) -> bool {
        match (self.scope, other.scope) {
            (None, _) | (_, None) => true,
            (Some(a), Some(b)) => a == b,
        }
    }
}

/// Every shortcut: app_server's, then the apps'.
pub fn entries() -> impl Iterator<Item = Entry> {
    let system = SHORTCUTS.iter().map(|i| Entry { id: i.id, title: i.title, scope: None, defaults: i.keys });
    let apps = crate::app_keys::APP_KEYS.iter().map(|k| Entry { id: k.id, title: k.title, scope: Some(k.app), defaults: k.keys });
    system.chain(apps)
}

pub fn entry(id: &str) -> Option<Entry> {
    entries().find(|e| e.id == id)
}

/// Give shortcut `id` (app_server's or an app's) just `chord` (None: no
/// keys) in `profile`, taking it from any other it would clash with.
/// Returns the titles of those it was taken from.
pub fn assign(profile: &mut KeyboardProfile, id: &str, chord: Option<Chord>) -> Vec<&'static str> {
    let mut taken = Vec::new();
    let Some(this) = entry(id) else { return taken };
    if let Some(c) = chord {
        for other in entries().filter(|e| e.id != id && e.clashes_with(&this)) {
            let keys = other.keys(profile);
            if keys.contains(&c) {
                let rest: Vec<String> = keys.iter().filter(|k| **k != c).map(|k| k.to_string()).collect();
                profile.shortcuts.insert(other.id.to_owned(), rest);
                taken.push(other.title);
            }
        }
    }
    profile.shortcuts.insert(id.to_owned(), chord.into_iter().map(|c| c.to_string()).collect());
    // Back to the platform's: nothing to keep.
    for e in entries() {
        if profile.shortcuts.get(e.id).is_some_and(|_| e.keys(profile) == e.default_keys(profile.platform)) {
            profile.shortcuts.remove(e.id);
        }
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

/// How auto-tiling lays a display's windows out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TilingLayout {
    /// Each new window takes half of the tile that had the focus, cut
    /// along its longer side (as bspwm and Hyprland's dwindle do).
    Halves,
    /// One main window in its place (`TilingSettings::main_place_of`:
    /// the left two thirds unless chosen), the rest stacked in the rest,
    /// each as big as the others; a new window is the main one (as
    /// dwm's, and its centred main). The default (Ben, 2026-11-15: "the
    /// main window gets the left 2/3 and the stack is the right 1/3").
    #[default]
    MainAndStack,
    /// Side by side, each as wide (Amethyst's Column).
    Columns,
    /// One above the other, each as tall (Amethyst's Row).
    Rows,
    /// Rows of about square cells (xmonad's Grid, awesome's fair).
    Grid,
    /// Each takes half of the room left, going round (awesome's and
    /// bspwm's spiral).
    Spiral,
    /// Side by side in a strip as wide as they need, each its own width
    /// (half the display at first), scrolled to the active one (niri's,
    /// PaperWM's).
    Scrolling,
}

/// The layouts offered by name (Deskbar's Auto-Tiling Layout menu,
/// Preferences): a layout, and for Main and Stack where its main window
/// goes (Ben, 2026-11-15: "make one of the auto-tile layout options
/// having the main window be the left half and then the stack be the
/// right half"). Any other place is Main and Stack's Main Window menu.
pub const LAYOUT_PRESETS: [(&str, TilingLayout, Option<Tile>); 8] = [
    ("Main Two Thirds, Stack a Third", TilingLayout::MainAndStack, Some(Tile::LeftTwoThirds)),
    ("Main Half, Stack Half", TilingLayout::MainAndStack, Some(Tile::Left)),
    ("Halves", TilingLayout::Halves, None),
    ("Columns", TilingLayout::Columns, None),
    ("Rows", TilingLayout::Rows, None),
    ("Grid", TilingLayout::Grid, None),
    ("Spiral", TilingLayout::Spiral, None),
    ("Scrolling", TilingLayout::Scrolling, None),
];

impl TilingLayout {
    pub const ALL: [TilingLayout; 7] = [TilingLayout::MainAndStack, TilingLayout::Halves, TilingLayout::Columns, TilingLayout::Rows, TilingLayout::Grid, TilingLayout::Spiral, TilingLayout::Scrolling];

    /// Whether it's built in its own shape from the tiles in order (as
    /// the number of tiles changes), not grown a split at a time.
    pub fn built(self) -> bool {
        matches!(self, TilingLayout::Columns | TilingLayout::Rows | TilingLayout::Grid | TilingLayout::Spiral | TilingLayout::Scrolling)
    }

    /// Its name in menus.
    pub fn title(self) -> &'static str {
        match self {
            TilingLayout::Halves => "Halves",
            TilingLayout::MainAndStack => "Main and Stack",
            TilingLayout::Columns => "Columns",
            TilingLayout::Rows => "Rows",
            TilingLayout::Grid => "Grid",
            TilingLayout::Spiral => "Spiral",
            TilingLayout::Scrolling => "Scrolling",
        }
    }

    /// What it does, in a sentence.
    pub fn detail(self) -> &'static str {
        match self {
            TilingLayout::Halves => "Each new window takes half of the one you were using.",
            TilingLayout::MainAndStack => "The main window in its place; the others stack around it.",
            TilingLayout::Columns => "Every window a column, each as wide.",
            TilingLayout::Rows => "Every window a row, each as tall.",
            TilingLayout::Grid => "The windows in rows of about square cells.",
            TilingLayout::Spiral => "Each window takes half of the room left, round in a spiral.",
            TilingLayout::Scrolling => "Every window a column of its own width, in a strip that scrolls to the active one.",
        }
    }

    /// Its name in commands and the IPC: `halves`, `main_and_stack`.
    pub fn id(self) -> &'static str {
        match self {
            TilingLayout::Halves => "halves",
            TilingLayout::MainAndStack => "main_and_stack",
            TilingLayout::Columns => "columns",
            TilingLayout::Rows => "rows",
            TilingLayout::Grid => "grid",
            TilingLayout::Spiral => "spiral",
            TilingLayout::Scrolling => "scrolling",
        }
    }

    pub fn from_id(id: &str) -> Option<TilingLayout> {
        TilingLayout::ALL.into_iter().find(|l| l.id() == id)
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
    /// Apps whose windows always float over the tiles (and over the one
    /// Focus shows), by app id: a window's menu ▸ Float Window ▸ Always
    /// (`Request::SetAppFloating`). A window floated or tiled by hand
    /// goes by that instead.
    pub floating_apps: Vec<String>,
    /// Apps whose new windows open on a workspace of their own (a window
    /// menu's Always Open on Workspace N; i3's `assign`, Hyprland's
    /// workspace rules), on the display they open on. A window coming back
    /// where it was, or a dialog with its window, goes as before.
    pub app_workspaces: Vec<AppWorkspace>,
    /// Apps whose new windows always go to one place, on whichever
    /// display and workspace they open (the window menu's Pin ▸): Main
    /// and Stack's main window, or a part of the display. A window coming
    /// back where it was, or a dialog with its window, goes as before.
    pub app_pins: Vec<AppPin>,
    /// The windowing mode of every workspace (and display) that hasn't
    /// one of its own (Preferences ▸ Windows).
    pub default_mode: WindowingMode,
    /// How auto-tiling lays windows out on every workspace (and display)
    /// that hasn't a layout of its own (Preferences ▸ Windows).
    pub layout: TilingLayout,
    /// The workspaces (and displays of them) whose layout isn't the
    /// default (Deskbar's Window Arrangement menu,
    /// `Request::SetTilingLayout`; see [`TilingSettings::layout_of`];
    /// Ben, 2026-11-15: "remember it per-desktop-workspace"). app_server
    /// keeps it as they are chosen: a copy sent back with the rest is
    /// ignored, as `modes`.
    pub layouts: Vec<LayoutChoice>,
    /// What these settings were written as: 0 before 2026-11-15, whose
    /// defaults were Halves and the middle two thirds
    /// ([`TilingSettings::upgrade`] moves those to today's).
    #[serde(default)]
    pub version: u32,
    /// Where Main and Stack's main window goes on every workspace (and
    /// display) that hasn't a place of its own (Preferences ▸ Windows):
    /// any part a window can be tiled into ([`Tile::CHOOSABLE`]), the
    /// others stacked in the rest (Ben, 2026-11-14: "let's have the
    /// default be center two-thirds").
    pub main_place: Tile,
    /// The workspaces (and displays of them) whose main window goes
    /// elsewhere (Deskbar's tray, `Request::SetMainPlace`; see
    /// [`TilingSettings::main_place_of`]). app_server keeps it as they
    /// are chosen: a copy sent back with the rest is ignored, as
    /// `modes`.
    pub main_places: Vec<MainPlaceChoice>,
    /// How many main windows Main and Stack has on every workspace (and
    /// display) that hasn't a number of its own (Preferences ▸ Windows):
    /// they share the main place, one above the other (side by side in a
    /// place across the top or bottom), as dwm's and xmonad's nmaster.
    /// 1 to [`MAX_MAINS`].
    pub main_count: u32,
    /// No space around an auto-tiled window alone on its display: it
    /// fills the display beside Deskbar, as i3's and sway's smart gaps
    /// (Preferences ▸ Windows). The margin and gap come back with a
    /// second tile.
    pub smart_gaps: bool,
    /// Where a new window goes among the tiles in Main and Stack and the
    /// layouts built in order (Columns, Rows, Grid, Spiral), as dwm's
    /// attach, xmonad's insertPosition, Amethyst's "new windows to main"
    /// (Preferences ▸ Windows). Halves always halves the active tile.
    pub new_windows: NewWindows,
    /// The workspaces (and displays of them) with another number of main
    /// windows (Deskbar's tray ▸ Number of Main Windows, ⌃⌥⌘I / ⌃⌥⌘D,
    /// `Request::SetMainCount`; see [`TilingSettings::main_count_of`]).
    /// app_server keeps it as they are chosen: a copy sent back with the
    /// rest is ignored, as `modes`.
    pub main_counts: Vec<MainCountChoice>,
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

/// What [`TilingSettings`] are written as today (`version`).
pub const TILING_VERSION: u32 = 1;

impl Default for TilingSettings {
    fn default() -> Self {
        TilingSettings {
            gap: 8,
            margin: 8,
            ungrouped_apps: Vec::new(),
            floating_apps: Vec::new(),
            app_workspaces: Vec::new(),
            app_pins: Vec::new(),
            default_mode: WindowingMode::Standard,
            layout: TilingLayout::MainAndStack,
            layouts: Vec::new(),
            version: TILING_VERSION,
            main_place: Tile::LeftTwoThirds,
            main_places: Vec::new(),
            main_count: 1,
            smart_gaps: false,
            new_windows: NewWindows::First,
            main_counts: Vec::new(),
            modes: Vec::new(),
            auto: Vec::new(),
        }
    }
}

/// Where Main and Stack's main window goes on a display's (`display`,
/// its `display_key`) workspace (counting from 0).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MainPlaceChoice {
    pub workspace: u32,
    pub display: String,
    pub place: Tile,
}

/// Where an app's windows always go as they open
/// ([`TilingSettings::app_pins`]; the window menu's Pin ▸ Always Main,
/// Always Top Left Quarter).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Pin {
    /// Main and Stack's main window, pinned there (as `Request::PinMain`).
    Main,
    /// A part of the display: tiled there, the rest tiled round it where
    /// windows are auto-tiled.
    Part(Tile),
}

impl Pin {
    /// Its item in the window menu's Pin.
    pub fn title(self) -> String {
        match self {
            Pin::Main => "Always Main".to_owned(),
            Pin::Part(tile) => format!("Always {}", tile.title()),
        }
    }
}

/// An app's pin ([`TilingSettings::app_pins`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppPin {
    pub app_id: String,
    pub pin: Pin,
}

/// An app's own workspace ([`TilingSettings::app_workspaces`]), counting
/// from 0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppWorkspace {
    pub app_id: String,
    pub workspace: u32,
}

/// Where a new window goes among the tiles ([`TilingSettings::new_windows`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewWindows {
    /// First: Main and Stack's main window (the others move down one).
    #[default]
    First,
    /// Right after the tile that had the focus.
    AfterActive,
    /// At the end: the bottom of the stack.
    Last,
}

impl NewWindows {
    pub const ALL: [NewWindows; 3] = [NewWindows::First, NewWindows::AfterActive, NewWindows::Last];

    pub fn title(self) -> &'static str {
        match self {
            NewWindows::First => "First, as the main window",
            NewWindows::AfterActive => "After the active window",
            NewWindows::Last => "Last",
        }
    }
}

/// How many main windows Main and Stack has on a display's (`display`,
/// its `display_key`) workspace (counting from 0).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MainCountChoice {
    pub workspace: u32,
    pub display: String,
    pub count: u32,
}

/// The most main windows Main and Stack has (more is a grid's work).
pub const MAX_MAINS: u32 = 4;

/// Auto-tiling's layout on a display's (`display`, its `display_key`)
/// workspace (counting from 0).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutChoice {
    pub workspace: u32,
    pub display: String,
    pub layout: TilingLayout,
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
        // Settings from before today's defaults (Ben, 2026-11-15: "change
        // the default auto-tiling so that the main window gets the left
        // 2/3 and the stack is the right 1/3"): what was the default then
        // is today's default; what was chosen stays.
        if self.version < 1 {
            if self.layout == TilingLayout::Halves {
                self.layout = TilingLayout::MainAndStack;
            }
            if self.main_place == Tile::CenterTwoThirds {
                self.main_place = Tile::LeftTwoThirds;
            }
            // (A workspace's own place that's now the default isn't kept.)
            let main_place = self.main_place;
            self.main_places.retain(|m| m.place != main_place);
        }
        self.version = TILING_VERSION;
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

    /// Auto-tiling's layout on the display `display` (its key) on its
    /// workspace `workspace`: its own, else the default.
    pub fn layout_of(&self, workspace: u32, display: &str) -> TilingLayout {
        self.layouts.iter().find(|l| l.workspace == workspace && l.display == display).map_or(self.layout, |l| l.layout)
    }

    /// Choose auto-tiling's layout on `display`'s workspace `workspace`;
    /// only one that isn't the default is kept.
    pub fn set_layout(&mut self, workspace: u32, display: &str, layout: TilingLayout) {
        self.layouts.retain(|l| !(l.workspace == workspace && l.display == display));
        if layout != self.layout {
            self.layouts.push(LayoutChoice { workspace, display: display.to_owned(), layout });
        }
    }

    /// The preset (`LAYOUT_PRESETS`, by its index) the display's
    /// workspace is on, if any.
    pub fn preset_of(&self, workspace: u32, display: &str) -> Option<usize> {
        let (layout, place) = (self.layout_of(workspace, display), self.main_place_of(workspace, display));
        LAYOUT_PRESETS.iter().position(|(_, l, p)| *l == layout && p.is_none_or(|p| p == place))
    }

    /// Where Main and Stack's main window goes on the display `display`
    /// (its key) on its workspace `workspace`: its own place, else the
    /// default.
    pub fn main_place_of(&self, workspace: u32, display: &str) -> Tile {
        self.main_places.iter().find(|m| m.workspace == workspace && m.display == display).map_or(self.main_place, |m| m.place)
    }

    /// Choose where the main window goes on `display`'s workspace
    /// `workspace`; only a place that isn't the default is kept.
    pub fn set_main_place(&mut self, workspace: u32, display: &str, place: Tile) {
        self.main_places.retain(|m| !(m.workspace == workspace && m.display == display));
        if place != self.main_place {
            self.main_places.push(MainPlaceChoice { workspace, display: display.to_owned(), place });
        }
    }

    /// The workspace `app_id`'s new windows open on, if it has one.
    pub fn app_workspace(&self, app_id: &str) -> Option<u32> {
        self.app_workspaces.iter().find(|a| a.app_id == app_id).map(|a| a.workspace)
    }

    /// Where `app_id`'s new windows always go, if anywhere.
    pub fn app_pin(&self, app_id: &str) -> Option<Pin> {
        self.app_pins.iter().find(|a| a.app_id == app_id).map(|a| a.pin)
    }

    /// Pin `app_id`'s new windows, or (None) let them go as any app's.
    pub fn set_app_pin(&mut self, app_id: &str, pin: Option<Pin>) {
        self.app_pins.retain(|a| a.app_id != app_id);
        if let Some(pin) = pin {
            self.app_pins.push(AppPin { app_id: app_id.to_owned(), pin });
        }
    }

    /// Give `app_id` a workspace of its own, or (None) none.
    pub fn set_app_workspace(&mut self, app_id: &str, workspace: Option<u32>) {
        self.app_workspaces.retain(|a| a.app_id != app_id);
        if let Some(workspace) = workspace {
            self.app_workspaces.push(AppWorkspace { app_id: app_id.to_owned(), workspace });
        }
    }

    /// How many main windows Main and Stack has on `display`'s workspace
    /// `workspace`: its own number if it was given one, else the default;
    /// 1 to [`MAX_MAINS`].
    pub fn main_count_of(&self, workspace: u32, display: &str) -> u32 {
        self.main_counts.iter().find(|m| m.workspace == workspace && m.display == display).map_or(self.main_count, |m| m.count).clamp(1, MAX_MAINS)
    }

    /// Choose how many main windows `display`'s workspace `workspace`
    /// has (kept within 1 to [`MAX_MAINS`]); only a number that isn't the
    /// default is kept.
    pub fn set_main_count(&mut self, workspace: u32, display: &str, count: u32) {
        let count = count.clamp(1, MAX_MAINS);
        self.main_counts.retain(|m| !(m.workspace == workspace && m.display == display));
        if count != self.main_count.clamp(1, MAX_MAINS) {
            self.main_counts.push(MainCountChoice { workspace, display: display.to_owned(), count });
        }
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
    /// Each workspace's layout its own once chosen, else the default;
    /// the presets read back from the layout and place.
    /// An app's pin: kept, changed, taken away; the part a window's
    /// tile is, by its name; and how it's written.
    #[test]
    fn an_apps_pin() {
        let mut t = TilingSettings::default();
        assert_eq!(t.app_pin("olo.telegram"), None);
        t.set_app_pin("olo.telegram", Some(Pin::Part(Tile::TopLeft)));
        t.set_app_pin("olo.phone", Some(Pin::Main));
        t.set_app_pin("olo.telegram", Some(Pin::Part(Tile::TopRight)));
        assert_eq!((t.app_pin("olo.telegram"), t.app_pin("olo.phone"), t.app_pins.len()), (Some(Pin::Part(Tile::TopRight)), Some(Pin::Main), 2));
        let json = serde_json::to_string(&t.app_pins).unwrap();
        assert_eq!(json, r#"[{"app_id":"olo.phone","pin":"main"},{"app_id":"olo.telegram","pin":{"part":"TopRight"}}]"#);
        assert_eq!(serde_json::from_str::<Vec<AppPin>>(&json).unwrap(), t.app_pins);
        t.set_app_pin("olo.phone", None);
        assert_eq!(t.app_pin("olo.phone"), None);
        // (Settings from before there were pins have none.)
        assert_eq!(serde_json::from_str::<TilingSettings>("{}").unwrap().app_pins, []);
        assert_eq!((Pin::Main.title(), Pin::Part(Tile::TopLeft).title()), ("Always Main".to_owned(), "Always Top Left Quarter".to_owned()));
        // A tile's part, gaps and all, is the part with that name; one
        // dragged elsewhere is none.
        assert_eq!(Tile::of_part([0.0, 0.0, 0.5, 0.5]), Some(Tile::TopLeft));
        assert_eq!(Tile::of_part([0.505, 0.0, 0.495, 0.49]), Some(Tile::TopRight));
        assert_eq!(Tile::of_part([0.0, 0.0, 0.66, 1.0]), Some(Tile::LeftTwoThirds));
        assert_eq!(Tile::of_part([0.0, 0.0, 0.58, 1.0]), None);
        assert_eq!(Tile::of_part([0.0, 0.0, 1.0, 1.0]), None);
    }

    #[test]
    fn layouts_by_workspace() {
        let mut t = TilingSettings::default();
        assert_eq!((t.layout, t.main_place), (TilingLayout::MainAndStack, Tile::LeftTwoThirds), "today's defaults");
        assert_eq!(t.preset_of(0, "eDP-1"), Some(0), "Main Two Thirds, Stack a Third");
        t.set_layout(1, "eDP-1", TilingLayout::Halves);
        assert_eq!((t.layout_of(1, "eDP-1"), t.layout_of(1, "DP-2"), t.layout_of(0, "eDP-1")), (TilingLayout::Halves, TilingLayout::MainAndStack, TilingLayout::MainAndStack));
        assert_eq!(t.preset_of(1, "eDP-1"), Some(2), "Halves, whatever the place");
        t.set_main_place(2, "eDP-1", Tile::Left);
        assert_eq!(t.preset_of(2, "eDP-1"), Some(1), "Main Half, Stack Half");
        t.set_main_place(3, "eDP-1", Tile::CenterThird);
        assert_eq!(t.preset_of(3, "eDP-1"), None, "Main and Stack elsewhere");
        // Back to the default: not kept.
        t.set_layout(1, "eDP-1", TilingLayout::MainAndStack);
        assert!(t.layouts.is_empty());
        // Kept through a round trip; a copy without them reads none.
        t.set_layout(4, "eDP-1", TilingLayout::Halves);
        let back: TilingSettings = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.layout_of(4, "eDP-1"), TilingLayout::Halves);
    }

    /// Settings from before today's defaults move to them where they held
    /// the old ones; what was chosen stays; once.
    #[test]
    fn old_defaults_move_to_new_ones() {
        let old = r#"{"layout":"halves","main_place":"CenterTwoThirds","main_places":[{"workspace":0,"display":"Dell","place":"CenterThird"}],"modes":[]}"#;
        let mut t: TilingSettings = serde_json::from_str(old).unwrap();
        assert_eq!(t.version, 0, "written before versions");
        t.upgrade();
        assert_eq!((t.layout, t.main_place, t.version), (TilingLayout::MainAndStack, Tile::LeftTwoThirds, TILING_VERSION));
        assert_eq!(t.main_place_of(0, "Dell"), Tile::CenterThird, "a workspace's own place stays");
        // Chosen ones stay: a layout or a place that wasn't the old default.
        let chosen = r#"{"layout":"main_and_stack","main_place":"RightThird"}"#;
        let mut t: TilingSettings = serde_json::from_str(chosen).unwrap();
        t.upgrade();
        assert_eq!((t.layout, t.main_place), (TilingLayout::MainAndStack, Tile::RightThird));
        // Once: today's settings holding Halves (chosen since) keep it.
        let mut t = TilingSettings { layout: TilingLayout::Halves, ..TilingSettings::default() };
        t.upgrade();
        assert_eq!(t.layout, TilingLayout::Halves);
    }

    use super::*;

    #[test]
    fn a_chord_is_pressed_as_its_keys() {
        let c: Chord = "Shift+Cmd+Left".parse().unwrap();
        assert_eq!(c.physical(), [125, 42, 105]);
        let c: Chord = "Ctrl+Alt+Backspace".parse().unwrap();
        assert_eq!(c.physical(), [29, 56, 14]);
        assert_eq!("End".parse::<Chord>().unwrap().physical(), [107]);
    }

    #[test]
    fn chords_round_trip() {
        for info in SHORTCUTS {
            for k in info.keys.iter().copied().flatten() {
                let c: Chord = k.parse().unwrap_or_else(|e| panic!("{}: {e}", info.id));
                assert_eq!(c.to_string(), *k, "{}", info.id);
            }
        }
        let c: Chord = "ctrl+option+command+left".parse().unwrap();
        assert!(c.ctrl && c.alt && c.cmd && !c.shift && c.key == 105);
        assert_eq!(c.symbols(), "⌃⌥⌘←");
        assert_eq!("Alt+Print".parse::<Chord>().unwrap().symbols(), "⌥ Print Screen");
        assert!("Cmd+Nope".parse::<Chord>().is_err() && "Hyper+A".parse::<Chord>().is_err());
        // As each platform writes it.
        let c: Chord = "Ctrl+Shift+Cmd+Left".parse().unwrap();
        assert_eq!(c.label(Platform::Windows), "Win+Ctrl+Shift+←");
        assert_eq!(c.label(Platform::Beos), "Ctrl+Opt+Shift+←");
        assert_eq!("Ctrl+Alt+Delete".parse::<Chord>().unwrap().label(Platform::Beos), "Ctrl+Alt+Del");
        assert_eq!("win+s".parse::<Chord>().unwrap().label(Platform::Windows), "Win+S");
    }

    #[test]
    fn apps_own_workspaces() {
        let mut t = TilingSettings::default();
        assert_eq!(t.app_workspace("olo.logs"), None);
        t.set_app_workspace("olo.logs", Some(2));
        t.set_app_workspace("olo.kache", Some(1));
        assert_eq!((t.app_workspace("olo.logs"), t.app_workspace("olo.kache")), (Some(2), Some(1)));
        t.set_app_workspace("olo.logs", Some(0));
        assert_eq!(t.app_workspaces.len(), 2, "one each");
        t.set_app_workspace("olo.logs", None);
        assert_eq!((t.app_workspace("olo.logs"), t.app_workspace("olo.kache")), (None, Some(1)));
        // Read from settings written before there were any.
        let old: TilingSettings = serde_json::from_str(r#"{"gap": 4}"#).unwrap();
        assert!(old.app_workspaces.is_empty());
    }

    #[test]
    fn defaults_never_clash() {
        for platform in Platform::ALL {
            let all: Vec<Chord> = SHORTCUTS.iter().flat_map(|i| i.default_keys(platform)).collect();
            assert_eq!(all.len(), SHORTCUTS.iter().map(|i| i.keys[platform.index()].len()).sum::<usize>(), "{platform:?}: a key doesn't parse");
            for (i, c) in all.iter().enumerate() {
                assert!(!all[i + 1..].contains(c), "{platform:?}: {c} is used twice");
            }
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
        let mut p = KeyboardProfile::default();
        let c: Chord = "Ctrl+Alt+Left".parse().unwrap();
        assert_eq!(assign(&mut p, "center", Some(c)), ["Left half"]);
        assert_eq!(p.shortcuts["tile-left"], Vec::<String>::new());
        assert_eq!(p.shortcuts["center"], ["Ctrl+Alt+Left"]);
        let b = bindings(&p);
        assert!(b.contains(&(c, Shortcut::Center)));
        // Back to its default: forgotten.
        assign(&mut p, "center", "Ctrl+Alt+C".parse().ok());
        assert!(!p.shortcuts.contains_key("center"));
        // Off.
        assign(&mut p, "find", None);
        assert!(!bindings(&p).iter().any(|(_, s)| *s == Shortcut::Find));
        // The platform's keys are the profile's own.
        let w = KeyboardProfile::builtin(Platform::Windows);
        assert!(bindings(&w).contains(&("Cmd+Left".parse().unwrap(), Shortcut::Tile(Tile::Left))));
        assert!(bindings(&w).contains(&("Alt+Tab".parse().unwrap(), Shortcut::SwitchApps)));
    }

    #[test]
    fn app_keys_never_clash() {
        for platform in Platform::ALL {
            let all: Vec<(Entry, Chord)> = entries().flat_map(|e| e.default_keys(platform).into_iter().map(move |c| (e, c))).collect();
            let written: usize = entries().map(|e| e.defaults[platform.index()].len()).sum();
            assert_eq!(all.len(), written, "{platform:?}: a key doesn't parse");
            for (i, (a, c)) in all.iter().enumerate() {
                for (b, d) in &all[i + 1..] {
                    assert!(!(c == d && a.clashes_with(b)), "{platform:?}: {c} is both {} and {}", a.id, b.id);
                }
            }
        }
        let ids: std::collections::HashSet<&str> = entries().map(|e| e.id).collect();
        assert_eq!(ids.len(), entries().count());
        // Taking an app's keys for app_server's.
        let mut p = KeyboardProfile::default();
        assert_eq!(assign(&mut p, "lock", "Cmd+C".parse().ok()), ["Copy", "Copy", "Copy Username"]);
        assert!(crate::app_keys::by_id("app.copy").is_some());
        assert!(entry("app.copy").unwrap().keys(&p).is_empty());
        // An app's own clash with each other, not with another app's
        // (Logs' Live), nor every app's (an app's own is its choice).
        let mut p = KeyboardProfile::default();
        assert_eq!(assign(&mut p, "viewer.crop", "Cmd+L".parse().ok()), ["Rotate Left"]);
        assert_eq!(entry("logs.live").unwrap().keys(&p), ["Cmd+L".parse::<Chord>().unwrap()]);
        assert!(assign(&mut p, "viewer.crop", "Cmd+T".parse().ok()).is_empty());
    }

    /// Each of Chrome's commands says what Chrome takes for it, and a
    /// profile's keys for them come with that.
    #[test]
    fn chrome_keys_say_what_chrome_takes() {
        let chrome: Vec<&str> = crate::app_keys::APP_KEYS.iter().filter(|k| k.app == Some("google-chrome")).map(|k| k.id).collect();
        let native: Vec<&str> = crate::app_keys::CHROME_NATIVE.iter().map(|(id, _)| *id).collect();
        assert_eq!(chrome, native);
        for (id, n) in crate::app_keys::CHROME_NATIVE {
            assert!(n.parse::<Chord>().is_ok(), "{id}: {n}");
        }
        let keys = crate::app_keys::chrome_keys(&KeyboardProfile::default());
        assert!(keys.contains(&("Cmd+Y".parse().unwrap(), "Ctrl+H".parse().unwrap())));
        assert!(keys.contains(&("Ctrl+Cmd+F".parse().unwrap(), "F11".parse().unwrap())));
        // Changed in the profile: the new keys stand for it.
        let mut p = KeyboardProfile::default();
        assign(&mut p, "chrome.history", "Shift+Cmd+H".parse().ok());
        let keys = crate::app_keys::chrome_keys(&p);
        assert!(keys.contains(&("Shift+Cmd+H".parse().unwrap(), "Ctrl+H".parse().unwrap())));
        assert!(!keys.iter().any(|(from, _)| *from == "Cmd+Y".parse().unwrap()));
    }

    #[test]
    fn profiles_say_when_they_are_changed() {
        let mut p = KeyboardProfile::default();
        assert_eq!(p.name, "Mac");
        assert!(p.is_builtin());
        assign(&mut p, "lock", "Ctrl+Cmd+L".parse().ok());
        p.changed();
        assert_eq!(p.name, "Mac (Customized)");
        // As it came again: its own name back.
        assign(&mut p, "lock", "Ctrl+Cmd+Q".parse().ok());
        p.changed();
        assert_eq!(p.name, "Mac");
        // One saved under a name of its own keeps it.
        p.name = "Mine".into();
        p.caps_lock_control = false;
        p.changed();
        assert_eq!(p.name, "Mine");
    }
}
