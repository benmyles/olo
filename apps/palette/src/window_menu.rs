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
use olo_protocol::{Client, KeyboardProfile, Reply, Request, app_keys, shortcuts};

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
    /// The windows it can be tiled with (Tile With): the others on its
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
        let keys = match c.request(&Request::GetSettings) {
            Ok(Reply::Settings { settings }) => settings.keyboard.profile,
            _ => Default::default(),
        };
        let mode = match c.request(&Request::WindowingMode { workspace: Some(w.workspace) }) {
            Ok(Reply::WindowingMode { displays, .. }) => displays.into_iter().find(|d| Some(&d.output) == w.output.as_ref()).map(|d| d.mode),
            _ => None,
        };
        let (auto, focus) = (mode == Some(olo_protocol::WindowingMode::AutoTiling), mode == Some(olo_protocol::WindowingMode::Focus));
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
        let (rolled, on_top, grouped, floating, floats_always) = (w.rolled, w.on_top, w.group.is_some(), w.floating, w.floats_always);
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
            beside,
            grouped,
            windows: app_windows,
            auto,
            focus,
            floating,
            floats_always,
            keys,
            path,
            app,
            screens,
            airplay,
        });
    });
}

/// The keys shortcut `id` has now, as the profile's platform writes them
/// ("⌃⌘H", "Win+D").
fn keys(f: &Facts, id: &str) -> Option<String> {
    let info = shortcuts::by_id(id);
    debug_assert!(info.is_some(), "{id}");
    shortcuts::keys_for(info?, &f.keys).first().map(|c| c.label(f.keys.platform))
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

pub fn build(f: &Facts) -> Menu {
    // A shortcut's item, by its id, with its keys.
    let shortcut = |m: Menu, id: &str, label: &str| {
        let m = m.item(label, format!("shortcut:{id}"));
        match keys(f, id) {
            Some(k) => m.keys(k),
            None => m,
        }
    };
    let tile = [
        ("tile-left", "Left Half"),
        ("tile-right", "Right Half"),
        ("tile-top", "Top Half"),
        ("tile-bottom", "Bottom Half"),
        ("", ""),
        ("tile-top-left", "Top Left Quarter"),
        ("tile-top-right", "Top Right Quarter"),
        ("tile-bottom-left", "Bottom Left Quarter"),
        ("tile-bottom-right", "Bottom Right Quarter"),
        ("", ""),
        ("tile-left-third", "Left Third"),
        ("tile-center-third", "Center Third"),
        ("tile-right-third", "Right Third"),
        ("tile-left-two-thirds", "Left Two Thirds"),
        ("tile-right-two-thirds", "Right Two Thirds"),
    ]
    .iter()
    .fold(Menu::new("Tile"), |m, (id, label)| if id.is_empty() { m.separator() } else { shortcut(m, id, label) });
    // Every display's workspaces, left to right as Deskbar's heading
    // menus list them; its own display's with their keys.
    let mut displays: Vec<(Option<&str>, Option<i32>)> = f.others.iter().map(|(o, n)| (Some(o.as_str()), Some(*n))).collect();
    displays.push((None, f.display));
    displays.sort_by_key(|(_, n)| *n);
    let mut spaces = Menu::new("Move to Workspace");
    for (k, (output, number)) in displays.into_iter().enumerate() {
        if k > 0 {
            spaces = spaces.separator();
        }
        for n in 0..f.workspaces.max(1) {
            let command = match output {
                Some(o) => format!("workspace:{n}:{o}"),
                None => format!("workspace:{n}"),
            };
            spaces = spaces.item(olo_protocol::workspace_name(number, n), command);
            if output.is_some() {
                continue;
            }
            if let Some(k) = keys(f, &format!("window-to-workspace-{}", n + 1)) {
                spaces = spaces.keys(k);
            }
            if n == f.workspace {
                spaces = spaces.checked(true).enabled(false);
            }
        }
    }
    let mut m = Menu::new(f.title.clone());
    m = shortcut(m, "hide-window", "Hide");
    m = shortcut(m, "roll-up", if f.rolled { "Roll Down" } else { "Roll Up" });
    m = shortcut(m, "zoom", "Zoom");
    m = shortcut(m, "zoom-vertical", "Zoom Vertically");
    m = shortcut(m, "zoom-horizontal", "Zoom Horizontally");
    m = shortcut(m, "restore", "Back to Its Own Size");
    m = shortcut(m, "center", "Center");
    if f.auto || f.focus {
        // Out of the tiles (or Focus's zoom), or back in (Ben, 2026-11-07:
        // "if we are in auto-tiling or focus mode i should be able to
        // choose "Float Window" and if it's part of a group then also an
        // option for "Float Window Group" … if it's part of a group and i
        // just choose to float window then it should also pull it out of
        // that group"; "an option to "Unfloat" a floating window"). The
        // keys float or tile it with its group, as Float Window Group.
        let with_keys = |m: Menu| match keys(f, "float-window") {
            Some(k) => m.keys(k),
            None => m,
        };
        if f.floating {
            // (A dialog or an alert floats whatever is asked: dimmed.)
            m = with_keys(m.item("Unfloat", "unfloat")).enabled(!f.floats_always);
        } else if f.grouped {
            m = m.item("Float Window", "float");
            m = with_keys(m.item("Float Window Group", "float-group"));
        } else {
            m = with_keys(m.item("Float Window", "float"));
        }
    }
    m = m.separator().submenu(tile);
    // Beside another window (not in Focus, which zooms them all; not a
    // window floating over auto-tiled ones).
    if !f.focus && !(f.auto && f.floating) && !f.beside.is_empty() {
        let with = f.beside.iter().fold(Menu::new("Tile With"), |m, (id, label)| m.item(label.clone(), format!("tile-with:{id}")));
        m = m.submenu(with);
    }
    m = m.submenu(spaces);
    if f.displays > 1 {
        m = shortcut(m, "next-display", "To the Next Display");
        m = shortcut(m, "previous-display", "To the Previous Display");
    }
    if let Some(screens) = &f.screens {
        m = m.submenu(Menu::agent_screens("Move to Agent Screen", screens, ""));
    }
    // Live on a television (Ben, 2026-11-12: "integrated airplay … by
    // right clicking the window title"): `olo-airplay show --window`.
    m = m.submenu(Menu::airplay("Show on AirPlay", &f.airplay, ""));
    m = m.separator();
    if f.path.is_some() {
        m = m.item(tracker::LABEL, tracker::COMMAND).separator();
    }
    if f.grouped {
        // As ⌘-dragging its tab out of the group does.
        m = m.item("Remove from Group", "ungroup");
    }
    // A tab into another window of its app (its group), as every tabbed
    // app's tabs go (`Menu::move_to_window`); its whole group, if in one.
    if !f.windows.is_empty() {
        m = m.submenu(Menu::move_to_window("Move to Window", f.windows.iter().map(|(id, title)| (title.clone(), format!("window-to:{id}")))));
        if f.grouped {
            m = m.submenu(Menu::move_to_window("Move Group to Window", f.windows.iter().map(|(id, title)| (title.clone(), format!("group-to:{id}")))));
        }
    }
    m = m.item("Always on Top", "on-top").checked(f.on_top);
    m = m.item("Send to Back", "send-to-back").separator();
    // The app's own Close and Quit, as its keys press them.
    let item = |m: Menu, label: String, command: &str, keys: Option<String>, enabled: bool| {
        let m = m.item(label, command).enabled(enabled);
        match keys {
            Some(k) => m.keys(k),
            None => m,
        }
    };
    m = match &f.app.close {
        Some((label, keys, enabled)) => item(m, label.clone(), "app-close", keys.clone(), *enabled),
        None => item(m, "Close".into(), "close", app_keys(f, "close"), true),
    };
    if f.grouped {
        // Every other window in its group (Ben, 2026-11-09: "we need a
        // "Close Others" option when right clicking a menu on a window tab
        // that's part of a group"), as a browser's Close Other Tabs.
        m = m.item("Close Others", "close-others");
    }
    match &f.app.quit {
        Some((label, keys, enabled)) => item(m, quit_label(f, Some(label)), "app-quit", keys.clone(), *enabled),
        None => item(m, quit_label(f, None), "quit-all", app_keys(f, "quit"), true),
    }
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
    // The other windows of its group, each closed as its own close box
    // closes it (an app that asks first, unsaved work, still asks).
    if command == "close-others" {
        let _ = std::thread::Builder::new().name("window-menu-do".into()).spawn(move || {
            let Ok(mut c) = Client::connect() else { return };
            let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
            let Some(group) = windows.iter().find(|w| w.id == window).and_then(|w| w.group) else { return };
            for w in windows.iter().filter(|w| w.group == Some(group) && w.id != window) {
                let _ = c.request(&Request::Close { id: w.id });
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
            beside: vec![(9, "StyledEdit — Untitled 1".into())],
            grouped: true,
            windows: vec![(12, "Notes 2".into())],
            auto: false,
            focus: false,
            floating: false,
            floats_always: false,
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
                    menu::MenuItem::Action(a) => out.push(a.command.clone()),
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
                || ["send-to-back", "on-top", "close", "close-others", "quit-all", "ungroup", "float", "float-group", "unfloat", tracker::COMMAND].contains(&c.as_str());
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
        // Close Others only for a window in one, after its Close.
        assert!(!alone.contains(&"close-others".to_owned()) && alone.ends_with(&["close".to_owned(), "quit-all".to_owned()]));
        // Into another of its app's windows; its group too, if in one.
        assert!(all.contains(&"window-to:12".to_owned()) && all.contains(&"group-to:12".to_owned()));
        assert!(alone.contains(&"window-to:12".to_owned()) && !alone.contains(&"group-to:12".to_owned()));
        let mut only = Vec::new();
        commands(&build(&Facts { windows: Vec::new(), ..facts.clone() }), &mut only);
        assert!(!only.iter().any(|c| c.starts_with("window-to:") || c.starts_with("group-to:")), "no other windows: no Move to Window");
        // Open in Tracker only for a window that shows a file or folder.
        assert!(!all.contains(&tracker::COMMAND.to_owned()));
        let mut doc = Vec::new();
        commands(&build(&Facts { path: Some("/home/me/notes.txt".into()), ..facts.clone() }), &mut doc);
        assert!(doc.contains(&tracker::COMMAND.to_owned()));
        // Floating only where auto-tiling or Focus is the mode.
        let floats = |f: &Facts| {
            build(f)
                .items
                .iter()
                .filter_map(|i| match i {
                    menu::MenuItem::Action(a) if ["float", "float-group", "unfloat"].contains(&a.command.as_str()) => Some((a.label.clone(), a.enabled, a.keys.is_some())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(floats(&facts).is_empty());
        // In a group: the window alone, or its group (the keys', ⌃⌥⌘F).
        let grouped = floats(&Facts { auto: true, ..facts.clone() });
        assert_eq!(grouped, [("Float Window".to_owned(), true, false), ("Float Window Group".to_owned(), true, true)]);
        assert_eq!(floats(&Facts { focus: true, ..facts.clone() }), grouped);
        // On its own: the window, with the keys.
        assert_eq!(floats(&Facts { auto: true, grouped: false, ..facts.clone() }), [("Float Window".to_owned(), true, true)]);
        // Floating: Unfloat; a dialog or an alert floats whatever is
        // asked: shown, dimmed.
        assert_eq!(floats(&Facts { focus: true, floating: true, ..facts.clone() }), [("Unfloat".to_owned(), true, true)]);
        assert_eq!(floats(&Facts { auto: true, floating: true, floats_always: true, ..facts.clone() }), [("Unfloat".to_owned(), false, true)]);
        assert!(all.contains(&"shortcut:next-display".to_owned()) && all.contains(&"workspace:2".to_owned()));
        // Each shows the keys it has now: changed, default, or none.
        let menu = build(&facts);
        let keys_of = |label: &str| {
            menu.items.iter().find_map(|i| match i {
                menu::MenuItem::Action(a) if a.label == label => Some(a.keys.clone()),
                _ => None,
            })
        };
        assert_eq!(keys_of("Zoom"), Some(Some("⌃⌘Z".into())));
        assert_eq!(keys_of("Hide"), Some(Some("⌘M".into())));
        assert_eq!(keys_of("Center"), Some(None));
        // Another program's Close and Quit: the window, and all of its
        // windows, under every app's keys in the profile.
        assert_eq!(keys_of("Close"), Some(Some("⌘W".into())));
        assert_eq!(keys_of("Quit Gedit"), Some(Some("⌘Q".into())));
        assert!(all.ends_with(&["close".to_owned(), "close-others".to_owned(), "quit-all".to_owned()]));
        let mut windows = facts.clone();
        windows.keys.platform = olo_protocol::shortcuts::Platform::Windows;
        let on_windows = build(&windows);
        let said: Vec<(String, Option<String>)> =
            on_windows.items.iter().rev().take(3).filter_map(|i| if let menu::MenuItem::Action(a) = i { Some((a.label.clone(), a.keys.clone())) } else { None }).collect();
        assert_eq!(said, [("Quit Gedit".to_owned(), Some("Ctrl+Q".to_owned())), ("Close Others".to_owned(), None), ("Close".to_owned(), Some("Ctrl+W".to_owned()))]);
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
        let last: Vec<(String, String, Option<String>)> =
            kit_menu.items.iter().rev().take(3).filter_map(|i| if let menu::MenuItem::Action(a) = i { Some((a.label.clone(), a.command.clone(), a.keys.clone())) } else { None }).collect();
        assert_eq!(
            last,
            [
                ("Quit Terminal".to_owned(), "app-quit".to_owned(), Some("⌘Q".to_owned())),
                ("Close Others".to_owned(), "close-others".to_owned(), None),
                ("Close Tab".to_owned(), "app-close".to_owned(), Some("⌘W".to_owned()))
            ]
        );
        let Some(menu::MenuItem::Submenu(spaces)) = menu.items.iter().find(|i| matches!(i, menu::MenuItem::Submenu(s) if s.title == "Move to Workspace")) else { panic!() };
        let menu::MenuItem::Action(first) = &spaces.items[0] else { panic!() };
        // Every display's workspaces, named for theirs, left to right: its
        // own with keys and a check, another's moving it there.
        assert_eq!(first.label, "D0 Workspace 1");
        assert_eq!(first.command, "workspace:0:HDMI-A-1");
        assert_eq!(first.keys.clone(), None);
        let own: Vec<&menu::Action> = spaces.items.iter().filter_map(|i| if let menu::MenuItem::Action(a) = i { Some(a) } else { None }).filter(|a| a.label.starts_with("D1")).collect();
        assert_eq!(own.len(), 3);
        assert_eq!((own[0].command.as_str(), own[0].keys.as_deref()), ("workspace:0", Some("⌃⇧1")));
        assert!(own[1].checked && !own[0].checked);
    }
}
