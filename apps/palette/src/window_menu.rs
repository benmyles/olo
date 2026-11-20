//! A window's menu: right-clicking its tab has app_server run
//! `olo-palette --window-menu ID X Y OUTPUT`, and the running palette
//! pops a menu there of everything that can be done to the window, the
//! same commands as the desktop's shortcuts. It is an ordinary kit popup
//! (over a clear full-screen layer that takes the click outside it), so
//! agents read and choose from it as from any menu; the layer goes when
//! the menu does.
//!
//! Close and Quit at its end are the app's own, as its keys press them:
//! a kit app's from that window's menus (`commands::menus_in`, chosen
//! with `invoke_in`: Terminal's Close Tab with a tab of several, a
//! question first where it asks one), with their labels and keys;
//! another program's closes the window, and Quit closes each of its
//! windows, as Deskbar's Close All does, under the profile's keys for
//! every app's Close and Quit (⌘W and ⌘Q on a Mac).

use std::path::PathBuf;

use olo_kit::{
    Cx, Element, InteractiveElement as _, LayerKeyboard, LocalWindow, MouseDownEvent, Pixels, Point, Proxy, Styled as _, View, WindowHandle, canvas,
    menu::{self, Menu, MenuPopup},
    point, px, tracker,
};
use olo_protocol::{Client, KeyboardProfile, Pin, Reply, Request, Tile, app_keys, shortcuts};

/// What `--window-menu` asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub window: u64,
    pub at: (f32, f32),
    pub output: Option<String>,
}

impl Ask {
    pub fn parse(args: &[String]) -> Option<Ask> {
        let i = args.iter().position(|a| a == "--window-menu")?;
        let num = |n: usize| args.get(i + n).and_then(|a| a.parse::<f32>().ok());
        Some(Ask { window: args.get(i + 1)?.parse().ok()?, at: (num(2)?, num(3)?), output: args.get(i + 4).filter(|o| !o.starts_with("--")).cloned() })
    }
}

/// The pin a menu command names (`app-pin:main`, `app-pin:part:N`, N
/// counting `Tile::CHOOSABLE`; `app-pin:none`: none).
fn pin_of(command: &str) -> Option<Pin> {
    match command.strip_prefix("app-pin:")? {
        "main" => Some(Pin::Main),
        part => Tile::CHOOSABLE.get(part.strip_prefix("part:")?.parse::<usize>().ok()?).copied().map(Pin::Part),
    }
}

/// What the menu needs to know, learned off the UI thread.
#[derive(Clone)]
pub struct Facts {
    pub ask: Ask,
    pub title: String,
    /// The window's workspace, and how many there are (each display has
    /// its own), and the number its display goes by among several ("D1").
    pub workspace: u32,
    pub workspaces: u32,
    pub display: Option<i32>,
    pub displays: usize,
    /// The other displays, by connector and number, to move it to one of
    /// their workspaces.
    pub others: Vec<(String, i32)>,
    /// Rolled up to its tab.
    pub rolled: bool,
    /// Kept above the others (Always on Top).
    pub on_top: bool,
    /// On every workspace of its display; in the scratchpad.
    pub all_workspaces: bool,
    pub scratch: bool,
    /// The windows it can be tiled with (Tile ▸ With Another Window): the others on its
    /// display's workspace, not minimized (auto-tiled: tiles only, not
    /// floating ones), each with its app's name and title.
    pub beside: Vec<(u64, String)>,
    /// In a group with other windows (their tabs side by side).
    pub grouped: bool,
    /// Its app's other windows it can go in with (Move to Window: one for
    /// each group, not its own, where it shows), by title.
    pub windows: Vec<(u64, String)>,
    /// Auto-tiling, or Focus, is its display's mode, and whether it
    /// floats over the tiles (or Focus's window) there.
    pub auto: bool,
    pub focus: bool,
    pub floating: bool,
    /// It floats whatever is asked (a dialog, an alert): Unfloat shows it,
    /// dimmed.
    pub floats_always: bool,
    /// A tile of Main and Stack's (auto-tiled, that layout): Make Main
    /// and Swap with Previous Main; whether it's the main window, and
    /// whether there is a main window before the one now.
    pub main_and_stack: bool,
    pub main: bool,
    pub previous_main: bool,
    /// It's the main window, pinned (Pin as Main is checked).
    pub main_pinned: bool,
    /// Its app's windows float always (Float Window ▸ Always:
    /// `TilingSettings::floating_apps`).
    pub app_floats: bool,
    /// The workspace its app's new windows open on, if it has one.
    pub app_workspace: Option<u32>,
    /// Where its app's new windows always go (`TilingSettings::app_pins`),
    /// and the part of its display this one is tiled into now, if it's
    /// one with a name (`Tile::of_part`).
    pub app_pin: Option<olo_protocol::Pin>,
    pub place: Option<olo_protocol::Tile>,
    /// Shortcut keys changed from the defaults, to show the ones in use.
    pub keys: KeyboardProfile,
    /// The file or folder it shows, if its program says
    /// (`Cx::set_path`): Open in Tracker. (Not Tracker's own windows.)
    pub path: Option<PathBuf>,
    /// Its program (its process and the window's surface, to choose from
    /// that window's menus), and the app's name.
    pub app: Owner,
    /// The agent screens it can be moved to (name, label), when its
    /// program can move (`WindowInfo::kept`) and this is the person's
    /// display; None, no Move to Agent Screen.
    pub screens: Option<Vec<olo_kit::moving::RunningScreen>>,
    /// The AirPlay receivers paired with, to show it on (Show on AirPlay).
    pub airplay: Vec<olo_protocol::airplay::Paired>,
}

/// Whose window it is, and the app's own Close and Quit if it's on the
/// kit.
#[derive(Clone, Debug, Default)]
pub struct Owner {
    pub pid: Option<u32>,
    pub surface: Option<u32>,
    pub app_id: String,
    pub name: String,
    /// The app's Close and Quit in that window's menus: label, keys,
    /// enabled.
    pub close: Option<(String, Option<String>, bool)>,
    pub quit: Option<(String, Option<String>, bool)>,
}

/// Ask app_server about the window, then hand the answer to the app.
pub fn learn(ask: Ask, proxy: Proxy) {
    let _ = std::thread::Builder::new().name("window-menu".into()).spawn(move || {
        let Ok(mut c) = Client::connect() else { return };
        let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
        let Some(w) = windows.iter().find(|w| w.id == ask.window).cloned() else { return };
        let workspaces = match c.request(&Request::Workspaces) {
            Ok(Reply::Workspaces { count, .. }) => count,
            _ => 1,
        };
        let outputs = match c.request(&Request::Outputs) {
            Ok(Reply::Outputs { outputs }) => outputs,
            _ => Vec::new(),
        };
        let displays = outputs.iter().filter(|o| o.mirror_of.is_none()).count().max(1);
        let display = w.output.as_deref().and_then(|o| olo_protocol::display_number(&outputs, o));
        let others =
            outputs.iter().filter(|o| o.mirror_of.is_none() && Some(&o.name) != w.output.as_ref()).filter_map(|o| Some((o.name.clone(), olo_protocol::display_number(&outputs, &o.name)?))).collect();
        let (keys, app_floats, app_workspace, app_pin) = match c.request(&Request::GetSettings) {
            Ok(Reply::Settings { settings }) => {
                (settings.keyboard.profile, !w.app_id.is_empty() && settings.tiling.floating_apps.contains(&w.app_id), settings.tiling.app_workspace(&w.app_id), settings.tiling.app_pin(&w.app_id))
            }
            _ => Default::default(),
        };
        let place = w.part.filter(|_| w.tiled).and_then(olo_protocol::Tile::of_part);
        let info = match c.request(&Request::WindowingMode { workspace: Some(w.workspace) }) {
            Ok(Reply::WindowingMode { displays, .. }) => displays.into_iter().find(|d| Some(&d.output) == w.output.as_ref()),
            _ => None,
        };
        let mode = info.as_ref().map(|d| d.mode);
        let (auto, focus) = (mode == Some(olo_protocol::WindowingMode::AutoTiling), mode == Some(olo_protocol::WindowingMode::Focus));
        let main_and_stack = auto && !w.floating && info.as_ref().is_some_and(|d| d.layout == olo_protocol::TilingLayout::MainAndStack);
        let previous_main = info.as_ref().is_some_and(|d| d.previous_main.is_some());
        // (A group once: its windows share their frame, so tiling one tiles
        // them all; its own group's not at all.)
        let mut groups = std::collections::HashSet::new();
        let beside = windows
            .iter()
            .filter(|o| o.id != w.id && o.output == w.output && o.workspace == w.workspace && !o.minimized && !(auto && o.floating))
            .filter(|o| w.group.is_none() || o.group != w.group)
            .filter(|o| o.group.is_none_or(|g| groups.insert(g)))
            .map(|o| {
                let entry = if olo_xdg::apps::our_app(&o.app_id).is_some() { None } else { olo_xdg::desktop_entry::find(&o.app_id) };
                let (name, _) = olo_xdg::apps::app_name(&o.app_id, entry.as_ref());
                let title: String = o.title.chars().take(48).collect();
                let label = if title.is_empty() || title == name {
                    name
                } else if o.title.chars().count() > 48 {
                    format!("{name} — {title}…")
                } else {
                    format!("{name} — {title}")
                };
                (o.id, label)
            })
            .collect();
        // Its app's other windows, one for each group but its own.
        let mut seen = std::collections::HashSet::new();
        let app_windows = windows
            .iter()
            .filter(|o| o.id != w.id && !w.app_id.is_empty() && o.app_id == w.app_id && w.groupable && o.groupable && !o.minimized)
            // (Where it shows: a group is on one display's workspace.)
            .filter(|o| o.output == w.output && o.workspace == w.workspace)
            .filter(|o| w.group.is_none() || o.group != w.group)
            .filter(|o| o.group.is_none_or(|g| seen.insert(g)))
            .map(|o| (o.id, if o.title.is_empty() { o.app_id.clone() } else { o.title.chars().take(60).collect() }))
            .collect();
        let path = match (w.pid, w.surface) {
            (Some(pid), Some(surface)) if w.app_id != "olo.tracker" => olo_kit::commands::path(pid as u32, surface),
            _ => None,
        };
        let (rolled, on_top, grouped, floating, floats_always, main) = (w.rolled, w.on_top, w.group.is_some(), w.floating, w.floats_always, w.main);
        let main_pinned = w.main_pinned;
        let (all_workspaces, scratch) = (w.all_workspaces, w.scratch);
        let pid = w.pid.map(|p| p as u32);
        let own = pid.zip(w.surface).and_then(|(pid, surface)| olo_kit::commands::menus_in(pid, surface)).map(|m| m.entries).unwrap_or_default();
        let find = |command: &str| own.iter().find(|e| e.command == command && e.path.len() == 1).map(|e| (e.label.clone(), e.keys.clone(), e.enabled));
        let entry = if olo_xdg::apps::our_app(&w.app_id).is_some() { None } else { olo_xdg::desktop_entry::find(&w.app_id) };
        let (name, _) = olo_xdg::apps::app_name(&w.app_id, entry.as_ref());
        let app = Owner { pid, surface: w.surface, app_id: w.app_id.clone(), name, close: find("close"), quit: find("quit") };
        let screens = (w.kept && olo_kit::moving::menus_offer()).then(olo_kit::moving::screens);
        let airplay = olo_protocol::airplay::paired();
        proxy.send(Facts {
            ask,
            title: w.title,
            workspace: w.workspace,
            workspaces,
            display,
            displays,
            others,
            rolled,
            on_top,
            all_workspaces,
            scratch,
            beside,
            grouped,
            windows: app_windows,
            auto,
            focus,
            floating,
            floats_always,
            main_and_stack,
            main,
            previous_main,
            main_pinned,
            app_floats,
            app_workspace,
            app_pin,
            place,
            keys,
            path,
            app,
            screens,
            airplay,
        });
    });
}

/// The keys every app's command `command` ("close", "quit") has in the
/// profile, as its platform writes them.
fn app_keys(f: &Facts, command: &str) -> Option<String> {
    let entry = app_keys::app_key(&f.app.app_id, command)?;
    shortcuts::entry(entry.id)?.keys(&f.keys).first().map(|c| c.label(f.keys.platform))
}

/// "Quit Terminal" (the app's own "Quit", or another program's).
fn quit_label(f: &Facts, own: Option<&str>) -> String {
    match own {
        Some(label) if label != "Quit" => label.to_owned(),
        _ if f.app.name.is_empty() => "Quit".to_owned(),
        _ => format!("Quit {}", f.app.name),
    }
}

/// The window's menu, in groups (Ben, 2026-11-18: "group the 3 zoom
/// options under a "Zoom ->" parent and the 3 float options under a
/// "Float ->" parent, the move options under a "Move ->" parent, and
/// anything else that makes sense to be grouped"): what's done to the
/// window where it is (Hide, Roll Up, Zoom ▸, Float ▸), where it goes
/// (Tile ▸, Pin ▸, Move ▸, Show ▸), what it is to its group and its
/// file, then Close and Quit. An item in a group shows the keys of two
/// parts that begin with its group's chord (⌃⌥⌘Z V), where it has some:
/// the same pattern as every menu of Olo's (HACKING, "Menus in groups").
pub fn build(f: &Facts) -> Menu {
    // A shortcut's item, by its id, with its keys: at the menu's top, its
    // one chord if it has one; in a group, its two in turn.
    let keyed = |m: Menu, id: &str, grouped: bool| {
        let all = shortcuts::by_id(id).map(|i| shortcuts::keys_for(i, &f.keys)).unwrap_or_default();
        debug_assert!(shortcuts::by_id(id).is_some(), "{id}");
        let pick = all.iter().find(|k| k.then.is_some() == grouped).or(all.first());
        match pick {
            Some(k) => m.keys(k.label(f.keys.platform)),
            None => m,
        }
    };
    let shortcut = |m: Menu, id: &str, label: &'static str| keyed(m.item(label, format!("shortcut:{id}")), id, false);
    let in_group = |m: Menu, id: &str, label: &'static str| keyed(m.item(label, format!("shortcut:{id}")), id, true);
    let has_app = !f.app.app_id.is_empty();
    let mut m = Menu::new(f.title.clone());
    // In a group, Hide and Close are each a menu: the window, the others
    // of its group, all of them (Ben, 2026-11-16: "we need a Close All
    // option when right clicking a window tab (we have Close and Close
    // Others already)"; "we need Hide Others and Hide All too. just nest
    // both of them under like Hide -> ... and Close -> ...").
    m = if f.grouped { m.submenu(shortcut(Menu::new("Hide"), "hide-window", "Hide").item("Hide Others", "hide-others").item("Hide All", "hide-all")) } else { shortcut(m, "hide-window", "Hide") };
    m = shortcut(m, "roll-up", if f.rolled { "Roll Down" } else { "Roll Up" });
    // (Behind them all: out of the way as Hide and Roll Up are, and no
    // way of showing it, so not Show's.)
    m = keyed(m.item("Send to Back", "send-to-back"), "send-to-back", false).separator();
    // Its size and shape: Zoom's three, and the way back from them;
    // Float; Tile (one group: Opus's look, 2026-11-20, of Zoom ▸ under
    // three commands that act at once).
    let mut zoom = Menu::new("Zoom");
    zoom = in_group(zoom, "zoom", "Zoom");
    zoom = in_group(zoom, "zoom-vertical", "Zoom Vertically");
    zoom = in_group(zoom, "zoom-horizontal", "Zoom Horizontally").separator();
    zoom = in_group(zoom, "restore", "Back to Its Own Size");
    m = m.submenu(zoom);
    if f.auto || f.focus {
        // Out of the tiles (or Focus's zoom), or back in (Ben, 2026-11-07:
        // "if we are in auto-tiling or focus mode i should be able to
        // choose "Float Window" and if it's part of a group then also an
        // option for "Float Window Group" … if it's part of a group and i
        // just choose to float window then it should also pull it out of
        // that group"; "an option to "Unfloat" a floating window"), and
        // every window of its app from now on (2026-11-14: "when always
        // is chosen it should remember so anytime that app window is
        // launched in the future it always floats when in tile mode":
        // `TilingSettings::floating_apps`; checked while it does, and
        // chosen again lets each of its windows go by that).
        let mut float = Menu::new("Float");
        if f.floating {
            // (A dialog or an alert floats whatever is asked: dimmed.)
            float = keyed(float.item("Unfloat", "unfloat"), "float-window", true).enabled(!f.floats_always);
        } else if f.grouped {
            float = keyed(float.item("Float This Window", "float"), "float-alone", true);
            float = keyed(float.item("Float Its Group", "float-group"), "float-window", true);
        } else {
            float = keyed(float.item("Float", "float"), "float-window", true);
        }
        let always = if f.app.name.is_empty() { olo_kit::words::tr("Always Float Its Application").to_owned() } else { olo_kit::tr!("Always Float {}", f.app.name) };
        let command = if f.app_floats { "unfloat-always" } else { "float-always" };
        float = keyed(float.separator().item(always, command), "float-app", true).checked(f.app_floats).enabled(has_app && !f.floats_always);
        m = m.submenu(float);
    }
    // Main and Stack: this one the main window, or the one before back
    // (Ben, 2026-11-14: "if a new window takes the main area i want to be
    // able to swap it back with the previous window").
    if f.main_and_stack {
        m = shortcut(m, "make-main", "Make Main").enabled(!f.main);
        m = shortcut(m, "swap-main", "Swap with Previous Main").enabled(f.previous_main);
    }
    // Halves, quarters, thirds and two thirds, a line between each; the
    // middle; beside another window (not in Focus, which zooms them all;
    // not a window floating over auto-tiled ones).
    let mut tile = Menu::new("Tile");
    let mut family = None;
    for t in olo_protocol::Tile::CHOOSABLE {
        let Some(id) = t.shortcut_id() else { continue };
        if family.is_some_and(|f| f != t.family()) {
            tile = tile.separator();
        }
        family = Some(t.family());
        tile = shortcut(tile, id, t.title());
    }
    tile = shortcut(tile.separator(), "center", "Center");
    if !f.focus && !(f.auto && f.floating) && !f.beside.is_empty() {
        tile = tile.separator().submenu(f.beside.iter().fold(Menu::new("With Another Window"), |m, (id, label)| m.item(label.clone(), format!("tile-with:{id}"))));
    }
    m = m.submenu(tile).separator();
    // Where it is and shows: Pin (Ben, 2026-11-17: "i often have some windows like telegram
    // where i want it to always open in the top-left quarter, and phone
    // in the top-right quarter (of whatever desktop-workspace they
    // happen to be opened on) … right click the window title tab and
    // choose Pin -> Always Main / Always [...] where Always [...] would
    // be e.g. \"Always Top Left Quarter\" based on where that window
    // currently is"). This window as the main one while it's here (Pin
    // as Main: windows that open leave it its place), and its app's
    // windows always, wherever they open: the main one, or in the part
    // of the display this one is in now. An app's pin that is another
    // place is listed too, checked, to take away. And the workspace its
    // app's new windows open on (i3's `assign`): checked if they open
    // here; chosen again, as any app's.
    let mut pin = Menu::new("Pin");
    if f.main_and_stack {
        pin = shortcut(pin, "pin-main", "Pin as Main").checked(f.main_pinned).separator();
        let on = f.app_pin == Some(Pin::Main);
        pin = pin.item(Pin::Main.title(), if on { "app-pin:none" } else { "app-pin:main" }).checked(on).enabled(has_app);
    } else if f.app_pin == Some(Pin::Main) {
        pin = pin.item(Pin::Main.title(), "app-pin:none").checked(true);
    }
    match f.place {
        Some(tile) => {
            let on = f.app_pin == Some(Pin::Part(tile));
            let n = Tile::CHOOSABLE.iter().position(|t| *t == tile).unwrap_or(0);
            pin = pin.item(Pin::Part(tile).title(), if on { "app-pin:none".to_owned() } else { format!("app-pin:part:{n}") }).checked(on).enabled(has_app);
        }
        // (Not in a part with a name: tiled into one first, Tile ▸.)
        None => pin = pin.item("Always Where It Is", "app-pin:none").enabled(false),
    }
    if let Some(Pin::Part(other)) = f.app_pin.filter(|p| Some(*p) != f.place.map(Pin::Part)) {
        pin = pin.item(Pin::Part(other).title(), "app-pin:none").checked(true);
    }
    if has_app && f.workspaces > 1 {
        let here = f.app_workspace == Some(f.workspace);
        let command = if here { "app-workspace:none".to_owned() } else { format!("app-workspace:{}", f.workspace) };
        pin = pin.separator().item(format!("Always Open {} on Workspace {}", f.app.name, f.workspace + 1), command).checked(here);
    }
    m = m.submenu(pin);
    // Move: to a workspace (every display's, left to right as Deskbar's
    // heading menus list them, its own display's with their keys), to
    // another display, an agent's screen, or another window of its app
    // as a tab (as every tabbed app's tabs go: `Menu::move_to_window`;
    // its whole group, if it's in one).
    let mut displays: Vec<(Option<&str>, Option<i32>)> = f.others.iter().map(|(o, n)| (Some(o.as_str()), Some(*n))).collect();
    displays.push((None, f.display));
    displays.sort_by_key(|(_, n)| *n);
    let mut moves = Menu::new("Move");
    for (k, (output, number)) in displays.into_iter().enumerate() {
        if k > 0 {
            moves = moves.separator();
        }
        for n in 0..f.workspaces.max(1) {
            let command = match output {
                Some(o) => format!("workspace:{n}:{o}"),
                None => format!("workspace:{n}"),
            };
            moves = moves.item(olo_protocol::workspace_name(number, n), command);
            if output.is_some() {
                continue;
            }
            moves = keyed(moves, &format!("window-to-workspace-{}", n + 1), true);
            if n == f.workspace {
                moves = moves.checked(true).enabled(false);
            }
        }
    }
    if f.displays > 1 {
        moves = in_group(moves.separator(), "next-display", "Next Display");
        moves = in_group(moves, "previous-display", "Previous Display");
    }
    let screens = f.screens.as_ref().map(|screens| Menu::agent_screens("Agent Screen", screens, ""));
    let windows = (!f.windows.is_empty()).then(|| Menu::move_to_window("Another Window", f.windows.iter().map(|(id, title)| (title.clone(), format!("window-to:{id}")))));
    if screens.is_some() || windows.is_some() {
        moves = moves.separator();
    }
    if let Some(screens) = screens {
        moves = moves.submenu(screens);
    }
    if let Some(windows) = windows {
        moves = moves.submenu(windows);
        if f.grouped {
            moves = moves.submenu(Menu::move_to_window("Its Group to Another Window", f.windows.iter().map(|(id, title)| (title.clone(), format!("group-to:{id}")))));
        }
    }
    m = m.submenu(moves);
    // Show: over the others, on every workspace (BeOS's All Workspaces,
    // i3's sticky), in the scratchpad (i3's and sway's, shown again by
    // its key), live on a television (Ben, 2026-11-12: "integrated
    // airplay … by right clicking the window title": `olo-airplay show
    // --window`).
    let mut show = Menu::new("Show");
    show = show.item("Always on Top", "on-top").checked(f.on_top);
    show = keyed(show.item("On All Workspaces", "shortcut:all-workspaces").checked(f.all_workspaces), "all-workspaces", false);
    show = keyed(show.item("In the Scratchpad", "shortcut:to-scratchpad").checked(f.scratch), "to-scratchpad", false);
    show = show.submenu(Menu::airplay("On AirPlay", &f.airplay, ""));
    m = m.submenu(show).separator();
    if f.path.is_some() {
        m = m.item(tracker::LABEL, tracker::COMMAND);
    }
    if f.grouped {
        // As ⌘-dragging its tab out of the group does.
        m = m.item("Remove from Group", "ungroup");
    }
    if f.path.is_some() || f.grouped {
        m = m.separator();
    }
    // The app's own Close and Quit, as its keys press them.
    let item = |m: Menu, label: String, command: &'static str, keys: Option<String>, enabled: bool| {
        let m = m.item(label, command).enabled(enabled);
        match keys {
            Some(k) => m.keys(k),
            None => m,
        }
    };
    let close = |m: Menu| match &f.app.close {
        Some((label, keys, enabled)) => item(m, label.clone(), "app-close", keys.clone(), *enabled),
        None => item(m, "Close".into(), "close", app_keys(f, "close"), true),
    };
    m = if f.grouped {
        // Every other window in its group (Ben, 2026-11-09: "we need a
        // "Close Others" option when right clicking a menu on a window tab
        // that's part of a group"), as a browser's Close Other Tabs; and
        // every one of them.
        m.submenu(close(Menu::new("Close")).item("Close Others", "close-others").item("Close All", "close-all"))
    } else {
        close(m)
    };
    match &f.app.quit {
        Some((label, keys, enabled)) => item(m, quit_label(f, Some(label)), "app-quit", keys.clone(), *enabled),
        None => item(m, quit_label(f, None), "quit-all", app_keys(f, "quit"), true),
    }
}

/// A command for the windows of a group: whether it closes (else hides),
/// and whether all of them (else all but the one asked on).
fn of_group(command: &str) -> Option<(bool, bool)> {
    match command {
        "close-others" => Some((true, false)),
        "close-all" => Some((true, true)),
        "hide-others" => Some((false, false)),
        "hide-all" => Some((false, true)),
        _ => None,
    }
}

/// The windows of `window`'s group a group command acts on: the others
/// in the order listed, then (`all`) the one asked on, last, so what it
/// shows stays put while the rest go. None for a window in no group.
/// (`windows`: each one's id and group.)
fn group_targets(windows: &[(u64, Option<u64>)], window: u64, all: bool) -> Vec<u64> {
    let Some(group) = windows.iter().find(|(id, _)| *id == window).and_then(|(_, g)| *g) else { return Vec::new() };
    let mut ids: Vec<u64> = windows.iter().filter(|(id, g)| *g == Some(group) && *id != window).map(|(id, _)| *id).collect();
    if all {
        ids.push(window);
    }
    ids
}

/// Do `command` to `window` (of `app`), off the UI thread.
pub fn perform(window: u64, app: &Owner, command: &str) {
    if let Some(screen) = olo_kit::moving::screen_command(command) {
        olo_kit::moving::take_to_screen(screen, vec![window]);
        return;
    }
    // The app's own Close and Quit, in that window.
    if let ("app-close" | "app-quit", Some(pid), Some(surface)) = (command, app.pid, app.surface) {
        let command = if command == "app-close" { "close" } else { "quit" };
        let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
            if !olo_kit::commands::invoke_in(pid, surface, command) {
                tracing::warn!("the app (pid {pid}) didn't take its {command}");
            }
        });
        return;
    }
    // The other windows of its group, or all of them (the others first,
    // the one asked on last): each closed as its own close box closes it
    // (an app that asks first, unsaved work or a program running, still
    // asks), or hidden as Hide hides it.
    if let Some((close, all)) = of_group(command) {
        let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
            let Ok(mut c) = Client::connect() else { return };
            let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
            let listed: Vec<(u64, Option<u64>)> = windows.iter().map(|w| (w.id, w.group)).collect();
            for id in group_targets(&listed, window, all) {
                let _ = c.request(&if close { Request::Close { id } } else { Request::Minimize { id } });
            }
        });
        return;
    }
    // Another program quit: each of its windows closed.
    if command == "quit-all" {
        let (pid, app_id) = (app.pid, app.app_id.clone());
        let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
            let Ok(mut c) = Client::connect() else { return };
            let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
            let same = |w: &olo_protocol::WindowInfo| w.id == window || (!app_id.is_empty() && w.app_id == app_id) || (pid.is_some() && w.pid.map(|p| p as u32) == pid);
            for w in windows.iter().filter(|w| same(w)) {
                let _ = c.request(&Request::Close { id: w.id });
            }
        });
        return;
    }
    let req = if let Some(id) = command.strip_prefix("shortcut:") {
        Request::RunShortcut { id: id.to_owned(), window: Some(window) }
    } else if let Some(with) = command.strip_prefix("tile-with:").and_then(|w| w.parse().ok()) {
        Request::TileWith { id: window, with }
    } else if let Some(with) = command.strip_prefix("window-to:").and_then(|w| w.parse().ok()) {
        Request::GroupWindow { id: window, with, index: None }
    } else if let Some(with) = command.strip_prefix("group-to:").and_then(|w| w.parse::<u64>().ok()) {
        // Each of its group's windows, in their order, into `with`'s.
        let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
            let Ok(mut c) = Client::connect() else { return };
            let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
            let Some(group) = windows.iter().find(|w| w.id == window).and_then(|w| w.group) else { return };
            for w in windows.iter().filter(|w| w.group == Some(group)) {
                let _ = c.request(&Request::GroupWindow { id: w.id, with, index: None });
            }
        });
        return;
    } else if let Some(rest) = command.strip_prefix("workspace:") {
        let (n, output) = match rest.split_once(':') {
            Some((n, o)) => (n, Some(o.to_owned())),
            None => (rest, None),
        };
        let Ok(index) = n.parse() else { return };
        Request::MoveToWorkspace { id: window, index, output }
    } else {
        match command {
            "send-to-back" => Request::SendToBack { id: window },
            "on-top" => Request::SetOnTop { id: window, on: None },
            "float" => Request::SetFloating { id: window, floating: Some(true), alone: true },
            "float-group" => Request::SetFloating { id: window, floating: Some(true), alone: false },
            "unfloat" => Request::SetFloating { id: window, floating: Some(false), alone: false },
            "float-always" | "unfloat-always" if !app.app_id.is_empty() => Request::SetAppFloating { app_id: app.app_id.clone(), floating: command == "float-always" },
            c if c.starts_with("app-workspace:") && !app.app_id.is_empty() => Request::SetAppWorkspace { app_id: app.app_id.clone(), workspace: c["app-workspace:".len()..].parse().ok() },
            c if c.starts_with("app-pin:") && !app.app_id.is_empty() => Request::SetAppPin { app_id: app.app_id.clone(), pin: pin_of(c), window: Some(window) },
            "ungroup" => Request::UngroupWindow { id: window },
            "close" => Request::Close { id: window },
            _ => return,
        }
    };
    let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
        if let Ok(mut c) = Client::connect() {
            let _ = c.request(&req);
        }
    });
}

/// The menu's clear layer: it pops the menu once it's there, and goes
/// when the menu does (chosen from, put away with Escape, or a click on
/// the layer: elsewhere on the screen).
pub struct Layer {
    window: u64,
    /// The window's title, naming it where it's shown (Deskbar's tray).
    title: String,
    app: Owner,
    path: Option<PathBuf>,
    at: Point<Pixels>,
    menu: Option<Menu>,
    popup: Option<LocalWindow<MenuPopup>>,
}

/// What the layer is told.
pub enum LayerMsg {
    /// It's there: its menu pops.
    Pop,
    /// The menu has gone: so does the layer.
    Gone,
}

impl View for Layer {
    type Message = LayerMsg;

    fn message(&mut self, message: LayerMsg, cx: &mut Cx<Self>) {
        match message {
            LayerMsg::Pop => {
                let Some(menu) = self.menu.take() else { return };
                let (target, app, path, title) = (self.window, self.app.clone(), self.path.clone(), self.title.clone());
                let gone = cx.messenger();
                self.popup = menu::context_menu_then(
                    cx,
                    menu,
                    self.at,
                    move |cmd, _| match (&path, olo_protocol::airplay::menu_command(cmd)) {
                        (Some(path), _) if cmd == tracker::COMMAND => {
                            if let Err(e) = tracker::open(path) {
                                tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
                            }
                        }
                        (_, Some(Some(receiver))) => {
                            if let Err(e) = olo_protocol::airplay::show_on(receiver, &olo_protocol::airplay::Shown::Window { id: target, title: title.clone() }) {
                                tracing::warn!("couldn't show the window on AirPlay: {e}");
                            }
                        }
                        (_, Some(None)) => {
                            let _ = olo_protocol::airplay::open_settings();
                        }
                        _ => perform(target, &app, cmd),
                    },
                    move |_| {
                        gone.send(LayerMsg::Gone);
                    },
                );
                // No menu: no layer.
                if self.popup.is_none() {
                    cx.close_window();
                }
            }
            LayerMsg::Gone => cx.close_window(),
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // A click off the menu lands here (the same program's, so the
        // menu's grab lets it through): away with it.
        canvas(|_, _| {}).paint_key(0).id("window-menu-layer").size_full().on_any_mouse_down(cx.listener(|l: &mut Self, _: &MouseDownEvent, cx| match l.popup.take() {
            Some(popup) => {
                popup.update(cx, |m, cx| m.close(cx));
            }
            None => cx.close_window(),
        }))
    }
}

/// Covers the window's output with the clear layer and pops its menu.
pub fn open(facts: Facts) -> WindowHandle<Layer> {
    let output = facts.ask.output.clone().filter(|o| olo_kit::display_names().iter().any(|d| d.as_deref() == Some(o.as_str())));
    // The menu is a popup of the layer and takes the keyboard itself
    // (app_server gives every key to a layer that holds it exclusively).
    let options = crate::overlay("Window Menu", "window-menu", output).layer_options(|l| l.keyboard = LayerKeyboard::OnDemand);
    olo_kit::open_window(options, move |cx| {
        cx.send(LayerMsg::Pop);
        Layer {
            window: facts.ask.window,
            title: facts.title.clone(),
            app: facts.app.clone(),
            path: facts.path.clone(),
            at: point(px(facts.ask.at.0), px(facts.ask.at.1)),
            menu: Some(build(&facts)),
            popup: None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_and_items() {
        let args: Vec<String> = ["--window-menu", "7", "120", "40.5", "HEADLESS-1"].iter().map(|s| s.to_string()).collect();
        let ask = Ask::parse(&args).unwrap();
        assert_eq!(ask, Ask { window: 7, at: (120.0, 40.5), output: Some("HEADLESS-1".into()) });
        assert_eq!(Ask::parse(&args[..4]).unwrap().output, None);
        // Every item is a shortcut app_server knows, or one of ours.
        let mut changed = KeyboardProfile::default();
        changed.shortcuts.insert("zoom".into(), vec!["Ctrl+Cmd+Z".into()]);
        changed.shortcuts.insert("center".into(), vec![]);
        let facts = Facts {
            ask,
            title: "Notes".into(),
            workspace: 1,
            workspaces: 3,
            displays: 2,
            rolled: false,
            on_top: false,
            all_workspaces: false,
            scratch: false,
            beside: vec![(9, "StyledEdit — Untitled 1".into())],
            grouped: true,
            windows: vec![(12, "Notes 2".into())],
            auto: false,
            focus: false,
            floating: false,
            floats_always: false,
            main_and_stack: false,
            main: false,
            previous_main: false,
            main_pinned: false,
            app_floats: false,
            app_workspace: None,
            app_pin: None,
            place: None,
            keys: changed,
            path: None,
            display: Some(1),
            others: vec![("HDMI-A-1".into(), 0)],
            app: Owner { app_id: "org.gnome.gedit".into(), name: "Gedit".into(), ..Default::default() },
            screens: Some(vec![olo_kit::moving::RunningScreen {
                name: "agent".into(),
                label: "Research".into(),
                wayland_display: "wayland-3".into(),
                olo_socket: "/run/olo-wayland-3.sock".into(),
                pid: 7,
                sound: false,
                bus: None,
                a11y: None,
                size: (1440, 900),
            }]),
            airplay: vec![olo_protocol::airplay::Paired { device_id: "AA:BB:CC:DD:EE:FF".into(), name: "Front Room".into() }],
        };
        fn commands(m: &Menu, out: &mut Vec<String>) {
            for item in &m.items {
                match item {
                    menu::MenuItem::Action(a) => out.push(a.command.to_string()),
                    menu::MenuItem::Submenu(s) => commands(s, out),
                    _ => {}
                }
            }
        }
        let mut all = Vec::new();
        commands(&build(&facts), &mut all);
        for c in &all {
            let known = c.strip_prefix("shortcut:").is_some_and(|id| shortcuts::SHORTCUTS.iter().any(|s| s.id == id))
                || c.starts_with("workspace:")
                || olo_kit::moving::screen_command(c).is_some()
                || olo_protocol::airplay::menu_command(c).is_some()
                || c.strip_prefix("tile-with:").is_some_and(|id| id.parse::<u64>().is_ok())
                || c.strip_prefix("window-to:").is_some_and(|id| id.parse::<u64>().is_ok())
                || c.strip_prefix("group-to:").is_some_and(|id| id.parse::<u64>().is_ok())
                || c.strip_prefix("app-workspace:").is_some_and(|n| n == "none" || n.parse::<u32>().is_ok())
                || (c.starts_with("app-pin:") && (*c == "app-pin:none" || pin_of(c).is_some()))
                || ["send-to-back", "on-top", "close", "close-others", "close-all", "hide-others", "hide-all", "quit-all", "ungroup", "float", "float-group", "unfloat", tracker::COMMAND]
                    .contains(&c.as_str());
            assert!(known, "{c}");
        }
        // Each agent screen running, and a new one; none where the
        // program can't move or this is a screen.
        assert!(all.contains(&"agent-screen:agent".to_owned()) && all.contains(&"agent-screen-new".to_owned()));
        let mut stuck = Vec::new();
        commands(&build(&Facts { screens: None, ..facts.clone() }), &mut stuck);
        assert!(!stuck.iter().any(|c| c.starts_with("agent-screen")));
        // Each receiver paired with, and the way to pair another.
        assert!(all.contains(&"airplay-on:AA:BB:CC:DD:EE:FF".to_owned()) && all.contains(&"airplay-settings".to_owned()));
        // Remove from Group only for a window in one.
        assert!(all.contains(&"ungroup".to_owned()));
        let mut alone = Vec::new();
        commands(&build(&Facts { grouped: false, ..facts.clone() }), &mut alone);
        assert!(!alone.contains(&"ungroup".to_owned()));
        // Others and All, of Close and of Hide, only for a window in one:
        // alone, a plain Hide first and a plain Close before Quit.
        assert!(!alone.iter().any(|c| of_group(c).is_some()) && alone.ends_with(&["close".to_owned(), "quit-all".to_owned()]));
        assert_eq!(alone.first().map(String::as_str), Some("shortcut:hide-window"));
        assert!(matches!(build(&Facts { grouped: false, ..facts.clone() }).items.first(), Some(menu::MenuItem::Action(a)) if a.label == "Hide"));
        // Into another of its app's windows; its group too, if in one.
        assert!(all.contains(&"window-to:12".to_owned()) && all.contains(&"group-to:12".to_owned()));
        assert!(alone.contains(&"window-to:12".to_owned()) && !alone.contains(&"group-to:12".to_owned()));
        let mut only = Vec::new();
        commands(&build(&Facts { windows: Vec::new(), ..facts.clone() }), &mut only);
        assert!(!only.iter().any(|c| c.starts_with("window-to:") || c.starts_with("group-to:")), "no other windows: no Move to Window");
        // Every part a window tiles into, the two thirds of each side too.
        for t in olo_protocol::Tile::CHOOSABLE {
            assert!(all.contains(&format!("shortcut:{}", t.shortcut_id().unwrap())), "{t:?}");
        }
        // Make Main and Swap with Previous Main only for a tile of Main
        // and Stack's: Make Main dimmed for the main window, Swap with
        // Previous Main while there's none before it.
        let mains = |f: &Facts| {
            let mut out = Vec::new();
            for i in &build(f).items {
                if let menu::MenuItem::Action(a) = i
                    && ["shortcut:make-main", "shortcut:swap-main"].contains(&a.command.as_str())
                {
                    out.push((a.label.to_string(), a.enabled, a.keys.is_some()));
                }
            }
            out
        };
        assert!(mains(&facts).is_empty() && mains(&Facts { auto: true, ..facts.clone() }).is_empty());
        let tile = Facts { auto: true, main_and_stack: true, ..facts.clone() };
        assert_eq!(mains(&tile), [("Make Main".into(), true, true), ("Swap with Previous Main".into(), false, true)]);
        assert_eq!(mains(&Facts { main: true, previous_main: true, ..tile.clone() }), [("Make Main".into(), false, true), ("Swap with Previous Main".into(), true, true)]);
        // Pin: its items as (label, command, checked, enabled).
        let pins = |f: &Facts| {
            build(f)
                .items
                .iter()
                .find_map(|i| match i {
                    menu::MenuItem::Submenu(m) if m.title == "Pin" => Some(
                        m.items
                            .iter()
                            .filter_map(|i| match i {
                                // (Its app's own workspace, at Pin's foot: below.)
                                menu::MenuItem::Action(a) if !a.command.starts_with("app-workspace:") => Some((a.label.to_string(), a.command.to_string(), a.checked, a.enabled)),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let it = |label: &str, command: &str, checked: bool, enabled: bool| (label.to_owned(), command.to_owned(), checked, enabled);
        // A window in no part with a name: nothing to pin it to yet.
        assert_eq!(pins(&facts), [it("Always Where It Is", "app-pin:none", false, false)]);
        // In the top left quarter: always there, by where it is; chosen
        // again, taken away.
        let quarter = Facts { place: Some(Tile::TopLeft), ..facts.clone() };
        assert_eq!(pins(&quarter), [it("Always Top Left Quarter", "app-pin:part:4", false, true)]);
        assert_eq!(pin_of("app-pin:part:4"), Some(Pin::Part(Tile::TopLeft)));
        assert_eq!(pins(&Facts { app_pin: Some(Pin::Part(Tile::TopLeft)), ..quarter.clone() }), [it("Always Top Left Quarter", "app-pin:none", true, true)]);
        // Pinned elsewhere: where it is now offered, the pin there is
        // listed and checked.
        assert_eq!(
            pins(&Facts { app_pin: Some(Pin::Part(Tile::Right)), ..quarter.clone() }),
            [it("Always Top Left Quarter", "app-pin:part:4", false, true), it("Always Right Half", "app-pin:none", true, true)]
        );
        // Main and Stack's tile: Pin as Main (this window, with its
        // key), Always Main (its app's), and where it is.
        let main_tile = Facts { place: Some(Tile::LeftTwoThirds), ..tile.clone() };
        assert_eq!(
            pins(&main_tile),
            [it("Pin as Main", "shortcut:pin-main", false, true), it("Always Main", "app-pin:main", false, true), it("Always Left Two Thirds", "app-pin:part:11", false, true)]
        );
        assert_eq!((pin_of("app-pin:main"), pin_of("app-pin:none"), pin_of("app-pin:part:99")), (Some(Pin::Main), None, None));
        let pinned = pins(&Facts { main: true, main_pinned: true, app_pin: Some(Pin::Main), ..main_tile.clone() });
        assert_eq!(pinned[..2], [it("Pin as Main", "shortcut:pin-main", true, true), it("Always Main", "app-pin:none", true, true)]);
        // (An app pinned as Main, its window where tiles aren't Main and
        // Stack's: still there to take away.)
        assert_eq!(pins(&Facts { app_pin: Some(Pin::Main), ..quarter.clone() })[0], it("Always Main", "app-pin:none", true, true));
        // Its app's own workspace is Pin's too: the one it's on, checked
        // once its app's windows open there, chosen again to let go.
        fn within<'a>(menu: &'a Menu, title: &str) -> &'a Menu {
            menu.items.iter().find_map(|i| if let menu::MenuItem::Submenu(s) = i { (s.title == title).then_some(s) } else { None }).unwrap_or_else(|| panic!("no {title} menu"))
        }
        let own_space = |f: &Facts| {
            within(&build(f), "Pin").items.iter().find_map(|i| match i {
                menu::MenuItem::Action(a) if a.command.starts_with("app-workspace:") => Some((a.label.to_string(), a.command.to_string(), a.checked)),
                _ => None,
            })
        };
        assert_eq!(own_space(&facts), Some(("Always Open Gedit on Workspace 2".into(), "app-workspace:1".into(), false)));
        assert_eq!(own_space(&Facts { app_workspace: Some(1), ..facts.clone() }), Some(("Always Open Gedit on Workspace 2".into(), "app-workspace:none".into(), true)));
        // Open in Tracker only for a window that shows a file or folder.
        assert!(!all.contains(&tracker::COMMAND.to_owned()));
        let mut doc = Vec::new();
        commands(&build(&Facts { path: Some("/home/me/notes.txt".into()), ..facts.clone() }), &mut doc);
        assert!(doc.contains(&tracker::COMMAND.to_owned()));
        // Floating only where auto-tiling or Focus is the mode: each item
        // as (submenu, label, enabled, keys, checked).
        let floats = |f: &Facts| {
            let mut out = Vec::new();
            for i in &build(f).items {
                let mut take = |sub: &str, a: &menu::Action| {
                    if ["float", "float-group", "unfloat", "float-always", "unfloat-always"].contains(&a.command.as_str()) {
                        out.push((sub.to_owned(), a.label.to_string(), a.enabled, a.keys.is_some(), a.checked));
                    }
                };
                match i {
                    menu::MenuItem::Action(a) => take("", a),
                    menu::MenuItem::Submenu(m) => {
                        for i in &m.items {
                            if let menu::MenuItem::Action(a) = i {
                                take(&m.title, a);
                            }
                        }
                    }
                    _ => {}
                }
            }
            out
        };
        let item = |sub: &str, label: &str, enabled: bool, keys: bool, checked: bool| (sub.to_owned(), label.to_owned(), enabled, keys, checked);
        assert!(floats(&facts).is_empty());
        // In a group: the window alone or its group (each with the keys
        // that begin with Float's chord, ⌃⌥⌘F), and its app always.
        let grouped = floats(&Facts { auto: true, ..facts.clone() });
        assert_eq!(grouped, [item("Float", "Float This Window", true, true, false), item("Float", "Float Its Group", true, true, false), item("Float", "Always Float Gedit", true, true, false)]);
        assert_eq!(floats(&Facts { focus: true, ..facts.clone() }), grouped);
        let keys_in = |f: &Facts, label: &str| {
            within(&build(f), "Float").items.iter().find_map(|i| match i {
                menu::MenuItem::Action(a) if a.label == label => a.keys.as_ref().map(|k| k.to_string()),
                _ => None,
            })
        };
        let auto = Facts { auto: true, ..facts.clone() };
        assert_eq!(
            (keys_in(&auto, "Float This Window"), keys_in(&auto, "Float Its Group"), keys_in(&auto, "Always Float Gedit")),
            (Some("⌃⌥⌘F  W".into()), Some("⌃⌥⌘F  F".into()), Some("⌃⌥⌘F  A".into()))
        );
        // On its own: the window; Always checked while its app floats
        // always (a window of it tiled by hand).
        assert_eq!(
            floats(&Facts { auto: true, grouped: false, app_floats: true, ..facts.clone() }),
            [item("Float", "Float", true, true, false), item("Float", "Always Float Gedit", true, true, true)]
        );
        // No app id to remember: Always dimmed.
        let nameless = Owner::default();
        assert_eq!(
            floats(&Facts { auto: true, grouped: false, app: nameless, ..facts.clone() }),
            [item("Float", "Float", true, true, false), item("Float", "Always Float Its Application", false, true, false)]
        );
        // Floating: Unfloat, and its app always (checked if it does); a
        // dialog or an alert floats whatever is asked: shown, dimmed.
        assert_eq!(floats(&Facts { focus: true, floating: true, ..facts.clone() }), [item("Float", "Unfloat", true, true, false), item("Float", "Always Float Gedit", true, true, false)]);
        assert_eq!(
            floats(&Facts { auto: true, floating: true, app_floats: true, ..facts.clone() }),
            [item("Float", "Unfloat", true, true, false), item("Float", "Always Float Gedit", true, true, true)]
        );
        assert_eq!(
            floats(&Facts { auto: true, floating: true, floats_always: true, ..facts.clone() }),
            [item("Float", "Unfloat", false, true, false), item("Float", "Always Float Gedit", false, true, false)]
        );
        assert!(all.contains(&"shortcut:next-display".to_owned()) && all.contains(&"workspace:2".to_owned()));
        // Each shows the keys it has now: changed, default, or none.
        let menu = build(&facts);
        // (The menu's own items, and those of its groups.)
        let said = |menu: &Menu| -> Vec<(String, String, Option<String>)> {
            menu.items
                .iter()
                .filter_map(|i| if let menu::MenuItem::Action(a) = i { Some((a.label.to_string(), a.command.to_string(), a.keys.as_ref().map(|k| k.to_string()))) } else { None })
                .collect()
        };
        let item = |label: &str, command: &str, keys: Option<&str>| (label.to_owned(), command.to_owned(), keys.map(str::to_owned));
        let keys_of = |label: &str| {
            menu.items.iter().chain(["Hide", "Close", "Zoom", "Tile"].iter().flat_map(|t| &within(&menu, t).items)).find_map(|i| match i {
                menu::MenuItem::Action(a) if a.label == label => Some(a.keys.clone()),
                _ => None,
            })
        };
        // In a group: Hide ▸ first, Close ▸ before Quit, each the window,
        // the others, all.
        assert!(matches!(menu.items.first(), Some(menu::MenuItem::Submenu(s)) if s.title == "Hide"));
        assert_eq!(said(within(&menu, "Hide")), [item("Hide", "shortcut:hide-window", Some("⌘M")), item("Hide Others", "hide-others", None), item("Hide All", "hide-all", None)]);
        assert_eq!(said(within(&menu, "Close")), [item("Close", "close", Some("⌘W")), item("Close Others", "close-others", None), item("Close All", "close-all", None)]);
        assert!(matches!(&menu.items[menu.items.len() - 2..], [menu::MenuItem::Submenu(s), menu::MenuItem::Action(q)] if s.title == "Close" && q.label == "Quit Gedit"));
        assert_eq!(keys_of("Zoom"), Some(Some("⌃⌘Z".into())));
        assert_eq!(keys_of("Hide"), Some(Some("⌘M".into())));
        assert_eq!(keys_of("Center"), Some(None));
        // Another program's Close and Quit: the window, and all of its
        // windows, under every app's keys in the profile.
        assert_eq!(keys_of("Close"), Some(Some("⌘W".into())));
        assert_eq!(keys_of("Quit Gedit"), Some(Some("⌘Q".into())));
        assert!(all.ends_with(&["close".to_owned(), "close-others".to_owned(), "close-all".to_owned(), "quit-all".to_owned()]));
        let mut windows = facts.clone();
        windows.keys.platform = olo_protocol::shortcuts::Platform::Windows;
        let on_windows = build(&windows);
        assert_eq!(said(within(&on_windows, "Close"))[0], item("Close", "close", Some("Ctrl+W")));
        assert_eq!(said(&on_windows).last(), Some(&item("Quit Gedit", "quit-all", Some("Ctrl+Q"))));
        // A kit app's are its own, from that window's menus: Terminal's
        // Close Tab with a tab of several, chosen in the app.
        let kit = Facts {
            app: Owner {
                pid: Some(42),
                surface: Some(9),
                app_id: "olo.terminal".into(),
                name: "Terminal".into(),
                close: Some(("Close Tab".into(), Some("⌘W".into()), true)),
                quit: Some(("Quit".into(), Some("⌘Q".into()), true)),
            },
            ..facts.clone()
        };
        let kit_menu = build(&kit);
        assert_eq!(said(within(&kit_menu, "Close")), [item("Close Tab", "app-close", Some("⌘W")), item("Close Others", "close-others", None), item("Close All", "close-all", None)]);
        assert_eq!(said(&kit_menu).last(), Some(&item("Quit Terminal", "app-quit", Some("⌘Q"))));
        // Whom a group's commands act on: the others as listed, then, for
        // All, the one asked on; nobody for a window in no group.
        let listed = [(1, Some(7)), (2, None), (3, Some(7)), (4, Some(9)), (5, Some(7))];
        assert_eq!(group_targets(&listed, 3, false), [1, 5]);
        assert_eq!(group_targets(&listed, 3, true), [1, 5, 3]);
        assert_eq!(group_targets(&listed, 2, true), Vec::<u64>::new());
        assert_eq!((of_group("close-all"), of_group("hide-others"), of_group("close")), (Some((true, true)), Some((false, false)), None));
        // Its groups as every menu's are (the kit's rule: no "Move ▸ Move
        // to …"), whatever the window.
        for f in [&facts, &Facts { auto: true, ..facts.clone() }, &Facts { floating: true, focus: true, ..facts.clone() }] {
            assert_eq!(menu::review_groups(&build(f)), Vec::<String>::new());
        }
        // The menu in its groups, top to bottom (a window in a group, on
        // one of two displays, where auto-tiling isn't on).
        let tops: Vec<String> = menu
            .items
            .iter()
            .map(|i| match i {
                menu::MenuItem::Action(a) => a.label.to_string(),
                menu::MenuItem::Submenu(s) => format!("{} ▸", s.title),
                _ => "—".to_owned(),
            })
            .collect();
        assert_eq!(tops, ["Hide ▸", "Roll Up", "Send to Back", "—", "Zoom ▸", "Tile ▸", "—", "Pin ▸", "Move ▸", "Show ▸", "—", "Remove from Group", "—", "Close ▸", "Quit Gedit"]);
        assert_eq!(said(within(&menu, "Zoom")).iter().map(|(l, ..)| l.as_str()).collect::<Vec<_>>(), ["Zoom", "Zoom Vertically", "Zoom Horizontally", "Back to Its Own Size"]);
        assert_eq!(said(within(&menu, "Show")).iter().map(|(l, ..)| l.as_str()).collect::<Vec<_>>(), ["Always on Top", "On All Workspaces", "In the Scratchpad"]);
        // Zoom's others have the keys that begin with its chord.
        assert_eq!(said(within(&menu, "Zoom"))[1], item("Zoom Vertically", "shortcut:zoom-vertical", Some("⌃⌥⌘Z  V")));
        let spaces = within(&menu, "Move");
        let menu::MenuItem::Action(first) = &spaces.items[0] else { panic!() };
        // Every display's workspaces, named for theirs, left to right: its
        // own with keys and a check, another's moving it there.
        assert_eq!(first.label, "D0 Workspace 1");
        assert_eq!(first.command, "workspace:0:HDMI-A-1");
        assert_eq!(first.keys.clone(), None);
        let own: Vec<&menu::Action> = spaces.items.iter().filter_map(|i| if let menu::MenuItem::Action(a) = i { Some(a) } else { None }).filter(|a| a.label.starts_with("D1")).collect();
        assert_eq!(own.len(), 3);
        assert_eq!((own[0].command.as_ref(), own[0].keys.as_ref().map(|k| k.as_ref())), ("workspace:0", Some("⌃⌥⌘V  1")));
        assert!(own[1].checked && !own[0].checked);
    }
}
