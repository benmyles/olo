//! Every Olo app's menus, the same where they can be (the Olo Book,
//! "Menus"; Ben, 2026-11-13: "in all the olo apps make it so that if
//! there is one or more recommended settings apps/panels that are related
//! then those should also be launchable from that app's menus. also
//! standardize the olo apps menus … so that kb shortcuts are consistent
//! and always available and the menu locations/options where applicable
//! are consistent").
//!
//! A window's bar starts with its first menu (File, or what the app
//! holds when it isn't files: Terminal, Disk, Screen), which starts as
//! BeOS's did, with About, then Keyboard Shortcuts and the app's settings
//! in Preferences ([`Menu::app_items`]), and ends with Close and Quit
//! ([`Menu::close_quit`]). Then Edit (Undo, Redo, Cut, Copy, Paste,
//! Select All, Find…, Find Again, in that order), View, and the app's
//! own. The kit runs About, Keyboard Shortcuts and the settings items
//! itself, from the bar, its keys, the palette and agents alike
//! ([`run`]); the window review checks each window's bar against this
//! ([`review`]: `tsession review`, the layout lint).

use olo_interface::WindowCx;

use crate::menu::{Menu, MenuItem};

/// About the app: its name and Olo's version.
pub const ABOUT: &str = "about-app";
/// The shortcuts sheet (the palette's): the front app's keys, then the
/// desktop's.
pub const SHORTCUTS: &str = "keyboard-shortcuts";
/// `pref:PANEL`, or `pref:PANEL/TAB`: Preferences at that panel (and tab).
pub const PREF: &str = "pref:";

/// The app's name as people know it ("Activity Monitor"), from its id.
pub fn app_name() -> String {
    let id = olo_kit_base::keys::app_id();
    let bin = id.replacen("olo.", "olo-", 1);
    match olo_xdg::apps::OUR_APPS.iter().find(|(b, ..)| *b == bin) {
        Some((_, name, _)) => (*name).to_owned(),
        None => {
            // (Not one listed: its id, made a name, "olo.wifi-heal" Wifi Heal.)
            let rest = id.strip_prefix("olo.").unwrap_or(id);
            rest.split('-').map(|w| w[..1.min(w.len())].to_uppercase() + &w[1.min(w.len())..]).collect::<Vec<_>>().join(" ")
        }
    }
}

/// The app's icon (an icon name).
fn app_icon() -> String {
    let bin = olo_kit_base::keys::app_id().replacen("olo.", "olo-", 1);
    olo_xdg::apps::OUR_APPS.iter().find(|(b, ..)| *b == bin).map_or_else(|| "application".to_owned(), |(.., icon)| (*icon).to_owned())
}

/// What a settings item says: "Sound Settings…" for `sound`, the panel's
/// name as Preferences shows it.
pub fn settings_label(target: &str) -> String {
    let panel = target.split_once('/').map_or(target, |(p, _)| p);
    let name = olo_xdg::apps::PREFERENCE_PANELS.iter().find(|(arg, ..)| *arg == panel).map_or(panel, |(_, name, _)| name);
    format!("{name} Settings…")
}

/// The shortcuts sheet's keys, as the profile writes them (app_server's:
/// they work anywhere, so the item shows them without taking them).
fn shortcuts_keys() -> Option<String> {
    let p = olo_kit_base::keys::profile();
    olo_protocol::shortcuts::entry("shortcut-help")?.keys(&p).first().map(|c| c.label(p.platform))
}

impl Menu {
    /// The first menu's start: About, Keyboard Shortcuts, then the app's
    /// `settings`, each a Preferences panel ("sound", or with a tab,
    /// "security/Passwords"), and a separator.
    pub fn app_items(self, settings: &[&str]) -> Menu {
        let mut m = self.item(format!("About {}", app_name()), ABOUT).item("Keyboard Shortcuts", SHORTCUTS);
        if let Some(keys) = shortcuts_keys() {
            m = m.keys(keys);
        }
        for s in settings {
            m = m.item(settings_label(s), format!("{PREF}{s}"));
        }
        m.separator()
    }

    /// The first menu's end: a separator, Close (⌘W) and Quit (⌘Q).
    pub fn close_quit(self) -> Menu {
        self.separator().item("Close", "close").key("w").item("Quit", "quit").key("q")
    }

    /// The first menu's end for a window of a program that stays running
    /// (the notification service's history, Deskbar's): a separator and
    /// Close, no Quit.
    pub fn close_item(self) -> Menu {
        self.separator().item("Close", "close").key("w")
    }
}

/// Runs `command` if it's one of the kit's (About, Keyboard Shortcuts, a
/// Preferences panel), in the window it was chosen in: whether it was.
pub fn run(command: &str, cx: &mut WindowCx) -> bool {
    match command {
        ABOUT => {
            about(cx);
            true
        }
        SHORTCUTS => {
            spawn("olo-palette --shortcuts");
            true
        }
        c if c.starts_with(PREF) => {
            open_settings(&c[PREF.len()..]);
            true
        }
        _ => false,
    }
}

/// Opens Preferences at `target`: a panel ("sound"), or a panel and its
/// tab ("network/Wi-Fi"), as a `pref:` item names them.
pub fn open_settings(target: &str) {
    let (panel, tab) = target.split_once('/').map_or((target, None), |(p, t)| (p, Some(t)));
    let tab = tab.map(|t| format!(" --tab '{}'", t.replace('\'', "'\\''"))).unwrap_or_default();
    spawn(&format!("olo-preferences '{}'{tab}", panel.replace('\'', "'\\''")));
}

fn spawn(command: &str) {
    if let Err(e) = olo_xdg::spawn_detached(command) {
        tracing::warn!("{command}: {e}");
    }
}

/// About the app, over its window: its icon and name, and Olo's version;
/// About Olo Desktop… opens Preferences' About.
fn about(cx: &mut WindowCx) {
    let message = format!("Part of Olo Desktop, version {}.", env!("CARGO_PKG_VERSION"));
    crate::widgets::open_alert(cx, &app_name(), &app_icon(), &message, &["About Olo Desktop…", "OK"], |chosen, _| {
        if *chosen == Some(0) {
            spawn("olo-preferences about");
        }
    });
}

/// Each standard command, the menu it's in ("" the first, whatever its
/// title) and what its label starts with.
const STANDARD: &[(&str, &str, &str)] = &[
    ("new", "", "New"),
    ("open", "", "Open"),
    ("save", "", "Save"),
    ("save-as", "", "Save As…"),
    ("close", "", "Close"),
    ("quit", "", "Quit"),
    (ABOUT, "", "About "),
    (SHORTCUTS, "", "Keyboard Shortcuts"),
    ("undo", "Edit", "Undo"),
    ("redo", "Edit", "Redo"),
    ("cut", "Edit", "Cut"),
    ("copy", "Edit", "Copy"),
    ("paste", "Edit", "Paste"),
    ("select-all", "Edit", "Select All"),
    ("find", "Edit", "Find…"),
    ("find-again", "Edit", "Find Again"),
    ("find-previous", "Edit", "Find Previous"),
    ("find-selection", "Edit", "Find Selection"),
    ("zoom-in", "View", "Zoom In"),
    ("zoom-out", "View", "Zoom Out"),
];

/// What's wrong with a window's bar `menus` by the Olo Book's "Menus":
/// (kind, what) for each. None for a window without a bar.
pub fn review(menus: &[Menu]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let Some(first) = menus.first() else { return found };
    let mut say = |text: String| found.push(("menus".to_owned(), text));
    fn actions(m: &Menu) -> Vec<&crate::menu::Action> {
        fn walk<'a>(items: &'a [MenuItem], out: &mut Vec<&'a crate::menu::Action>) {
            for item in items {
                match item {
                    MenuItem::Action(a) => out.push(a),
                    MenuItem::Submenu(s) => walk(&s.items, out),
                    MenuItem::Separator => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&m.items, &mut out);
        out
    }
    // The first menu: About first, Quit last (Close, for a window of a
    // program that stays running).
    let top: Vec<&crate::menu::Action> = first.items.iter().filter_map(|i| if let MenuItem::Action(a) = i { Some(a) } else { None }).collect();
    if top.first().is_none_or(|a| a.command != ABOUT) {
        say(format!("{}: About isn't its first item (Menu::app_items)", first.title));
    }
    if top.last().is_none_or(|a| a.command != "quit" && a.command != "close") {
        say(format!("{}: Quit isn't its last item (Menu::close_quit)", first.title));
    }
    // Each standard command in its menu, called what it is.
    for (i, menu) in menus.iter().enumerate() {
        for a in actions(menu) {
            let Some((_, home, label)) = STANDARD.iter().find(|(c, ..)| *c == a.command) else {
                if a.command.starts_with(PREF) && i != 0 {
                    say(format!("{} ▸ {}: settings go in the first menu", menu.title, a.label));
                }
                if a.command.starts_with(PREF) && !a.label.ends_with(" Settings…") {
                    say(format!("{} ▸ {}: a settings item says “… Settings…”", menu.title, a.label));
                }
                continue;
            };
            let right_menu = if home.is_empty() { i == 0 } else { menu.title == *home };
            if !right_menu {
                let want = if home.is_empty() { "the first menu" } else { home };
                say(format!("{} ▸ {}: belongs in {want}", menu.title, a.label));
            }
            if !a.label.starts_with(label) {
                say(format!("{} ▸ {}: say “{label}”", menu.title, a.label));
            }
        }
    }
    // No two of a window's items pressed with the same keys.
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for menu in menus {
        for a in actions(menu).into_iter().filter(|a| a.enabled) {
            let Some(keys) = a.keys.as_deref().filter(|_| a.shortcut.is_some()) else { continue };
            match seen.iter().find(|(k, _)| *k == keys) {
                Some((_, other)) if *other != a.label => say(format!("{keys} is both {other} and {}", a.label)),
                Some(_) => {}
                None => seen.push((keys, &a.label)),
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard() -> Vec<Menu> {
        vec![
            Menu::new("File").app_items(&["sound"]).item("New", "new").key("n").close_quit(),
            Menu::new("Edit").item("Undo", "undo").key("z").item("Copy", "copy").key("c").item("Find…", "find").key("f"),
            Menu::new("View").item("Zoom In", "zoom-in"),
        ]
    }

    #[test]
    fn a_standard_bar_is_clean() {
        let mut menus = standard();
        crate::menu::resolve(&mut menus, false);
        assert_eq!(review(&menus), Vec::<(String, String)>::new());
    }

    #[test]
    fn settings_say_their_panel() {
        assert_eq!(settings_label("sound"), "Sound Settings…");
        assert_eq!(settings_label("security/Passwords"), "Security Settings…");
    }

    #[test]
    fn what_it_finds() {
        let menus = vec![
            Menu::new("File").item("New", "new").item("Find", "find").item("Close", "close").item("Print…", "print"),
            Menu::new("View").item("Copy", "copy").item("Sound Preferences…", "pref:sound"),
        ];
        let found: Vec<String> = review(&menus).into_iter().map(|(_, t)| t).collect();
        assert!(found.iter().any(|t| t.contains("About isn't its first")), "{found:?}");
        assert!(found.iter().any(|t| t.contains("Quit isn't its last")), "{found:?}");
        assert!(found.iter().any(|t| t == "File ▸ Find: belongs in Edit"), "{found:?}");
        assert!(found.iter().any(|t| t == "File ▸ Find: say “Find…”"), "{found:?}");
        assert!(found.iter().any(|t| t == "View ▸ Copy: belongs in Edit"), "{found:?}");
        assert!(found.iter().any(|t| t.contains("settings go in the first menu")), "{found:?}");
        assert!(found.iter().any(|t| t.contains("a settings item says")), "{found:?}");
    }

    #[test]
    fn the_same_keys_twice() {
        let mut menus = vec![Menu::new("File").app_items(&[]).item("Rescan", "rescan").key("r").close_quit(), Menu::new("Image").item("Rotate Right", "rotate-right").key("r")];
        crate::menu::resolve(&mut menus, false);
        let found: Vec<String> = review(&menus).into_iter().map(|(_, t)| t).collect();
        assert!(found.iter().any(|t| t.contains("is both Rescan and Rotate Right")), "{found:?}");
    }
}
