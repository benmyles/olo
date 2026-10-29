//! A window's menu: right-clicking its tab has app_server run
//! `olo-palette --window-menu ID X Y OUTPUT`, and the running palette
//! pops a menu there of everything that can be done to the window, the
//! same commands as the desktop's shortcuts. It is an ordinary kit popup
//! (over a clear full-screen layer that takes the click outside it), so
//! agents read and choose from it as from any menu.

use std::path::PathBuf;

use olo_kit::{
    Cx, Keyboard, Layer, LayerOptions, Proxy, WindowId, WindowKind, WindowOptions,
    egui::{self, Pos2, pos2},
    menu::{self, Menu},
    tracker, widgets,
};
use olo_protocol::{
    Client, Reply, Request,
    shortcuts::{self, ShortcutSettings},
};

/// What `--window-menu` asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub window: u64,
    pub at: Pos2,
    pub output: Option<String>,
}

impl Ask {
    pub fn parse(args: &[String]) -> Option<Ask> {
        let i = args.iter().position(|a| a == "--window-menu")?;
        let num = |n: usize| args.get(i + n).and_then(|a| a.parse::<f32>().ok());
        Some(Ask {
            window: args.get(i + 1)?.parse().ok()?,
            at: pos2(num(2)?, num(3)?),
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
    pub display: Option<u32>,
    pub displays: usize,
    /// Rolled up to its tab.
    pub rolled: bool,
    /// In a group with other windows (their tabs side by side).
    pub grouped: bool,
    /// Auto-tiling is on for its display, and whether it floats over the
    /// tiles there.
    pub auto: bool,
    pub floating: bool,
    /// Shortcut keys changed from the defaults, to show the ones in use.
    pub keys: ShortcutSettings,
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
        let keys = match c.request(&Request::GetSettings) {
            Ok(Reply::Settings { settings }) => settings.shortcuts,
            _ => Default::default(),
        };
        let auto = match c.request(&Request::AutoTiling { workspace: Some(w.workspace) }) {
            Ok(Reply::AutoTiling { displays, .. }) => displays.iter().any(|d| d.on && Some(&d.output) == w.output.as_ref()),
            _ => false,
        };
        let path = match (w.pid, w.surface) {
            (Some(pid), Some(surface)) if w.app_id != "olo.tracker" => olo_kit::commands::path(pid as u32, surface),
            _ => None,
        };
        let (rolled, grouped, floating) = (w.rolled, w.group.is_some(), w.floating);
        proxy.send(Facts { ask, title: w.title, workspace: w.workspace, workspaces, display, displays, rolled, grouped, auto, floating, keys, path });
    });
}

/// The keys shortcut `id` has now, as on a Mac ("⌃⌘H").
fn keys(f: &Facts, id: &str) -> Option<String> {
    let info = shortcuts::by_id(id);
    debug_assert!(info.is_some(), "{id}");
    shortcuts::keys_for(info?, &f.keys).first().map(|c| c.symbols())
}

pub fn build(f: &Facts) -> Menu {
    // A shortcut's item, by its id, with its keys.
    let shortcut = |m: Menu, id: &str, label: &str| {
        let m = m.item(label, format!("shortcut:{id}"), None);
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
    let mut spaces = Menu::new("Move to Workspace");
    for n in 0..f.workspaces.max(1) {
        spaces = spaces.item(olo_protocol::workspace_name(f.display, n), format!("workspace:{n}"), None);
        if let Some(k) = keys(f, &format!("window-to-workspace-{}", n + 1)) {
            spaces = spaces.keys(k);
        }
        if n == f.workspace {
            spaces = spaces.checked(true).enabled(false);
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
        m = m.item(tracker::LABEL, tracker::COMMAND, None).separator();
    }
    if f.grouped {
        // As ⌘-dragging its tab out of the group does.
        m = m.item("Remove from Group", "ungroup", None);
    }
    m.item("Send to Back", "send-to-back", None).separator().item("Close", "close", None)
}

/// Do `command` to `window`, off the UI thread.
pub fn perform(window: u64, command: &str) {
    let req = if let Some(id) = command.strip_prefix("shortcut:") {
        Request::RunShortcut { id: id.to_owned(), window: Some(window) }
    } else if let Some(n) = command.strip_prefix("workspace:").and_then(|n| n.parse().ok()) {
        Request::MoveToWorkspace { id: window, index: n }
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

/// The menu's clear layer, while it shows.
pub struct Shown {
    pub win: WindowId,
    pub window: u64,
    pub path: Option<PathBuf>,
    pub at: Pos2,
    pub menu: Menu,
    /// The popup has been asked for, and has been seen open.
    pub asked: bool,
    pub seen: bool,
}

pub fn open(cx: &mut Cx, facts: Facts) -> Shown {
    let win = cx.open_window(WindowOptions {
        title: "Window Menu".into(),
        size: (1, 1),
        kind: WindowKind::Layer(LayerOptions {
            layer: Layer::Overlay,
            anchor: (true, true, true, true),
            exclusive_zone: -1,
            margin: (0, 0, 0, 0),
            keyboard: Keyboard::Exclusive,
            namespace: "window-menu".into(),
            output: facts.ask.output.clone().filter(|o| cx.outputs().contains(o)),
        }),
        panel: false,
        ..Default::default()
    });
    Shown { win, window: facts.ask.window, at: facts.ask.at, menu: build(&facts), path: facts.path, asked: false, seen: false }
}

/// Draw the layer: pop the menu, and say when it's done (chosen or not).
pub fn ui(shown: &mut Shown, ui: &mut egui::Ui) -> bool {
    let ctx = ui.ctx().clone();
    let source = egui::Id::unique("window-menu");
    if !shown.asked {
        shown.asked = true;
        widgets::popup_menu_at(&ctx, source, shown.at, shown.menu.clone());
        ctx.request_repaint();
        return false;
    }
    if let Some(cmd) = widgets::popup_result(&ctx, source) {
        match &shown.path {
            Some(path) if cmd == tracker::COMMAND => {
                if let Err(e) = tracker::open(path) {
                    tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
                }
            }
            _ => perform(shown.window, &cmd),
        }
        return true;
    }
    if menu::is_open(&ctx, source) {
        shown.seen = true;
        return false;
    }
    // Put away without a choice (a click outside, or Escape).
    if shown.seen {
        return true;
    }
    ctx.request_repaint();
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_and_items() {
        let args: Vec<String> = ["--window-menu", "7", "120", "40.5", "HEADLESS-1"].iter().map(|s| s.to_string()).collect();
        let ask = Ask::parse(&args).unwrap();
        assert_eq!(ask, Ask { window: 7, at: pos2(120.0, 40.5), output: Some("HEADLESS-1".into()) });
        assert_eq!(Ask::parse(&args[..4]).unwrap().output, None);
        // Every item is a shortcut app_server knows, or one of ours.
        let mut changed = ShortcutSettings::new();
        changed.insert("zoom".into(), vec!["Ctrl+Cmd+Z".into()]);
        changed.insert("center".into(), vec![]);
        let facts = Facts { ask, title: "Notes".into(), workspace: 1, workspaces: 3, displays: 2, rolled: false, grouped: true, auto: false, floating: false, keys: changed, path: None, display: Some(1) };
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
                menu::MenuItem::Action(a) if a.label == label => Some(a.key_label()),
                _ => None,
            })
        };
        assert_eq!(keys_of("Zoom"), Some(Some("⌃⌘Z".into())));
        assert_eq!(keys_of("Hide"), Some(Some("⌘M".into())));
        assert_eq!(keys_of("Center"), Some(None));
        let Some(menu::MenuItem::Submenu(spaces)) = menu.items.iter().find(|i| matches!(i, menu::MenuItem::Submenu(s) if s.title == "Move to Workspace")) else { panic!() };
        let menu::MenuItem::Action(first) = &spaces.items[0] else { panic!() };
        assert_eq!(first.key_label().as_deref(), Some("⌃⇧⌘1"));
        // Its display's workspaces, named for it among several.
        assert_eq!(first.label, "D1 Workspace 1");
    }
}
