//! A window's menu: right-clicking its tab has app_server run
//! `olo-palette --window-menu ID X Y OUTPUT`, and the running palette
//! pops a menu there of everything that can be done to the window, the
//! same commands as the desktop's shortcuts. It is an ordinary kit popup
//! (over a clear full-screen layer that takes the click outside it), so
//! agents read and choose from it as from any menu; the layer goes when
//! the menu does.

use std::path::PathBuf;

use olo_ui::{
    Frame, Proxy,
    gpui::{App, AppContext as _, Context, IntoElement, LayerKeyboard, MouseDownEvent, Pixels, Point, Render, Styled as _, Window, WindowHandle, canvas, point, px},
    menu::{self, Menu, MenuPopup},
    tracker,
};
use olo_protocol::{Client, KeyboardProfile, Reply, Request, shortcuts};

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
        Some(Ask {
            window: args.get(i + 1)?.parse().ok()?,
            at: (num(2)?, num(3)?),
            output: args.get(i + 4).filter(|o| !o.starts_with("--")).cloned(),
        })
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
    /// In a group with other windows (their tabs side by side).
    pub grouped: bool,
    /// Auto-tiling is on for its display, and whether it floats over the
    /// tiles there.
    pub auto: bool,
    pub floating: bool,
    /// Shortcut keys changed from the defaults, to show the ones in use.
    pub keys: KeyboardProfile,
    /// The file or folder it shows, if its program says
    /// (`Cx::set_path`): Open in Tracker. (Not Tracker's own windows.)
    pub path: Option<PathBuf>,
}

/// Ask app_server about the window, then hand the answer to the app.
pub fn learn(ask: Ask, proxy: Proxy) {
    let _ = std::thread::Builder::new().name("window-menu".into()).spawn(move || {
        let Ok(mut c) = Client::connect() else { return };
        let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return };
        let Some(w) = windows.into_iter().find(|w| w.id == ask.window) else { return };
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
        let others = outputs
            .iter()
            .filter(|o| o.mirror_of.is_none() && Some(&o.name) != w.output.as_ref())
            .filter_map(|o| Some((o.name.clone(), olo_protocol::display_number(&outputs, &o.name)?)))
            .collect();
        let keys = match c.request(&Request::GetSettings) {
            Ok(Reply::Settings { settings }) => settings.keyboard.profile,
            _ => Default::default(),
        };
        let auto = match c.request(&Request::AutoTiling { workspace: Some(w.workspace) }) {
            Ok(Reply::AutoTiling { displays, .. }) => displays.iter().any(|d| d.on && Some(&d.output) == w.output.as_ref()),
            _ => false,
        };
        let path = match (w.pid, w.surface) {
            (Some(pid), Some(surface)) if w.app_id != "olo.tracker" => olo_ui::commands::path(pid as u32, surface),
            _ => None,
        };
        let (rolled, grouped, floating) = (w.rolled, w.group.is_some(), w.floating);
        proxy.send(Facts { ask, title: w.title, workspace: w.workspace, workspaces, display, displays, others, rolled, grouped, auto, floating, keys, path });
    });
}

/// The keys shortcut `id` has now, as the profile's platform writes them
/// ("⌃⌘H", "Win+D").
fn keys(f: &Facts, id: &str) -> Option<String> {
    let info = shortcuts::by_id(id);
    debug_assert!(info.is_some(), "{id}");
    shortcuts::keys_for(info?, &f.keys).first().map(|c| c.label(f.keys.platform))
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
    if f.auto {
        // Auto-tiled: out of the tiles (or back in).
        m = shortcut(m, "float-window", "Float Over the Tiles").checked(f.floating);
    }
    m = m.separator().submenu(tile).submenu(spaces);
    if f.displays > 1 {
        m = shortcut(m, "next-display", "To the Next Display");
        m = shortcut(m, "previous-display", "To the Previous Display");
    }
    m = m.separator();
    if f.path.is_some() {
        m = m.item(tracker::LABEL, tracker::COMMAND).separator();
    }
    if f.grouped {
        // As ⌘-dragging its tab out of the group does.
        m = m.item("Remove from Group", "ungroup");
    }
    m.item("Send to Back", "send-to-back").separator().item("Close", "close")
}

/// Do `command` to `window`, off the UI thread.
pub fn perform(window: u64, command: &str) {
    let req = if let Some(id) = command.strip_prefix("shortcut:") {
        Request::RunShortcut { id: id.to_owned(), window: Some(window) }
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

/// The menu's clear layer: it pops the menu as it is first drawn, and
/// goes when the menu does (chosen from, put away with Escape, or a
/// click on the layer: elsewhere on the screen).
struct Layer {
    window: u64,
    path: Option<PathBuf>,
    at: Point<Pixels>,
    menu: Option<Menu>,
    popup: Option<WindowHandle<MenuPopup>>,
}

impl Render for Layer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                // A click off the menu lands here (the same program's, so
                // the menu's grab lets it through): away with it.
                let clicked = me.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    match clicked.update(cx, |l, _| l.popup.take()) {
                        Some(popup) => {
                            let _ = popup.update(cx, |m, window, cx| m.close(window, cx));
                        }
                        None => window.remove_window(),
                    }
                });
                let Some(menu) = me.update(cx, |l, _| l.menu.take()) else { return };
                let (target, path, at) = me.read_with(cx, |l, _| (l.window, l.path.clone(), l.at));
                let layer = window.window_handle();
                // After this frame: a window opened while one paints
                // would draw into the frame's arena.
                let shown = me.clone();
                window.defer(cx, move |window, cx| {
                    let popup = menu::context_menu_then(
                        window,
                        cx,
                        menu,
                        at,
                        move |cmd, _, _| match &path {
                            Some(path) if cmd == tracker::COMMAND => {
                                if let Err(e) = tracker::open(path) {
                                    tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
                                }
                            }
                            _ => perform(target, cmd),
                        },
                        move |cx: &mut App| {
                            let _ = layer.update(cx, |_, window, _| window.remove_window());
                        },
                    );
                    match popup {
                        Some(popup) => shown.update(cx, |l, _| l.popup = Some(popup)),
                        // No menu: no layer.
                        None => window.remove_window(),
                    }
                });
            },
        )
        .size_full()
    }
}

/// Covers the window's output with the clear layer and pops its menu.
pub fn open(facts: Facts, cx: &mut App) -> Option<WindowHandle<Frame>> {
    let output = facts.ask.output.clone().filter(|o| cx.displays().iter().any(|d| d.name().as_deref() == Some(o.as_str())));
    let layer = cx.new(|_| Layer { window: facts.ask.window, path: facts.path.clone(), at: point(px(facts.ask.at.0), px(facts.ask.at.1)), menu: Some(build(&facts)), popup: None });
    // The menu is a popup of the layer and takes the keyboard itself
    // (app_server gives every key to a layer that holds it exclusively).
    let options = crate::overlay("Window Menu", "window-menu", output).layer_options(|l| l.keyboard = LayerKeyboard::OnDemand);
    olo_ui::open_window(cx, options, move |_, _| layer)
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
        let facts = Facts { ask, title: "Notes".into(), workspace: 1, workspaces: 3, displays: 2, rolled: false, grouped: true, auto: false, floating: false, keys: changed, path: None, display: Some(1), others: vec![("HDMI-A-1".into(), 0)] };
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
                || ["send-to-back", "close", "ungroup", tracker::COMMAND].contains(&c.as_str());
            assert!(known, "{c}");
        }
        // Remove from Group only for a window in one.
        assert!(all.contains(&"ungroup".to_owned()));
        let mut alone = Vec::new();
        commands(&build(&Facts { grouped: false, ..facts.clone() }), &mut alone);
        assert!(!alone.contains(&"ungroup".to_owned()));
        // Open in Tracker only for a window that shows a file or folder.
        assert!(!all.contains(&tracker::COMMAND.to_owned()));
        let mut doc = Vec::new();
        commands(&build(&Facts { path: Some("/home/me/notes.txt".into()), ..facts.clone() }), &mut doc);
        assert!(doc.contains(&tracker::COMMAND.to_owned()));
        // Float Over the Tiles only where auto-tiling is on.
        assert!(!all.contains(&"shortcut:float-window".to_owned()));
        let mut auto = Vec::new();
        commands(&build(&Facts { auto: true, ..facts.clone() }), &mut auto);
        assert!(auto.contains(&"shortcut:float-window".to_owned()));
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
