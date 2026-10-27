//! What the palette can find, and what choosing each does. Everything
//! that touches another process (app_server, an app's menus, Passwords)
//! or the disk runs on a worker thread.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use tabula_kit::{
    Proxy,
    apps::{OUR_APPS, PREFERENCE_PANELS, our_app},
    commands, desktop_entry,
};
use tabula_protocol::{Client, Reply, Request, ShortcutSettings, WindowInfo, shortcuts};

/// What a result is (shown at its right, and how it ranks among equals).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Calculator,
    Command,
    Window,
    Application,
    Preference,
    Shortcut,
    Setting,
    Password,
    File,
    Web,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Calculator => "Calculator",
            Kind::Command => "Command",
            Kind::Window => "Window",
            Kind::Application => "Application",
            Kind::Preference => "Preferences",
            Kind::Shortcut => "Shortcut",
            Kind::Setting => "Setting",
            Kind::Password => "Password",
            Kind::File => "File",
            Kind::Web => "Web",
        }
    }

    /// Added to a match's score: apps before files at the same match.
    pub fn weight(self) -> i32 {
        match self {
            Kind::Calculator => 1000,
            Kind::Application => 30,
            Kind::Command => 25,
            Kind::Shortcut | Kind::Setting => 35,
            Kind::Preference => 15,
            Kind::Window => 10,
            Kind::Password => 0,
            // Files come after anything else that matches as well.
            Kind::File => -60,
            Kind::Web => -1000,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// A desktop entry's command line.
    Launch(String),
    /// One of our programs, with arguments.
    Spawn(Vec<String>),
    /// An app_server shortcut by id, done to the window that was in front.
    Shortcut(String),
    Activate(u64),
    /// A command from the menus of the app with this process id.
    Menu { pid: u32, command: String },
    Open(PathBuf),
    Password { vault: String, id: String },
    Copy(String),
    /// The desktop's light or dark appearance.
    Theme(bool),
    /// Search the web in the web browser.
    Search(String),
}

impl Action {
    /// Names it among the recently chosen (stable across runs).
    pub fn key(&self) -> Option<String> {
        Some(match self {
            Action::Launch(c) => format!("launch:{c}"),
            Action::Spawn(a) => format!("spawn:{}", a.join(" ")),
            Action::Shortcut(id) => format!("shortcut:{id}"),
            Action::Open(p) => format!("open:{}", p.display()),
            Action::Password { vault, id } => format!("password:{vault}/{id}"),
            Action::Theme(dark) => format!("theme:{dark}"),
            // Windows and menus come and go; sums and searches are one-offs.
            Action::Activate(_) | Action::Menu { .. } | Action::Copy(_) | Action::Search(_) => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    /// Keys as a Mac shows them ("⌃⌘←").
    pub keys: Option<String>,
    pub icon: Option<String>,
    /// More words it answers to, lowercased, space separated.
    pub keywords: String,
    pub action: Action,
    pub enabled: bool,
}

impl Item {
    pub fn new(kind: Kind, title: impl Into<String>, action: Action) -> Item {
        Item { kind, title: title.into(), subtitle: String::new(), keys: None, icon: None, keywords: String::new(), action, enabled: true }
    }
    pub fn subtitle(mut self, s: impl Into<String>) -> Item {
        self.subtitle = s.into();
        self
    }
    pub fn icon(mut self, s: impl Into<String>) -> Item {
        self.icon = Some(s.into());
        self
    }
    pub fn keywords(mut self, s: impl AsRef<str>) -> Item {
        self.keywords = s.as_ref().to_lowercase();
        self
    }
}

/// What a worker found, for the UI thread.
pub enum Found {
    /// Applications, preference panels and settings (on start).
    Fixed(Vec<Item>),
    /// The windows and the window in front (before opening).
    Context { front: Option<WindowInfo>, items: Vec<Item> },
    /// The front app's menu commands.
    Menus(Vec<Item>),
    /// Files for query number `.0`.
    Files(u64, Vec<Item>),
    /// Passwords for query number `.0`.
    Passwords(u64, Vec<Item>),
}

/// Installed applications, our own first, the preference panels and the
/// desktop's appearance.
pub fn fixed() -> Vec<Item> {
    let mut items = Vec::new();
    let mut ours = Vec::new();
    for (bin, name, icon) in OUR_APPS.iter().filter(|(bin, _, _)| tabula_kit::find_program(bin).is_some()) {
        ours.push(*bin);
        items.push(Item::new(Kind::Application, *name, Action::Spawn(vec![bin.to_string()])).icon(*icon).keywords(bin.trim_start_matches("tabula-")));
    }
    items.push(Item::new(Kind::Application, "Preferences", Action::Spawn(vec!["tabula-preferences".into()])).icon("preferences").keywords("settings"));
    for e in desktop_entry::all() {
        let Some(cmd) = e.command() else { continue };
        // Our own apps' entries are listed already.
        if e.no_display || ours.iter().any(|b| cmd.split_whitespace().next().is_some_and(|c| c.ends_with(b))) {
            continue;
        }
        let keywords = format!("{} {}", e.generic_name.clone().unwrap_or_default(), e.id.replace(['.', '-'], " "));
        let mut item = Item::new(Kind::Application, e.name.clone(), Action::Launch(if e.terminal { format!("tabula-terminal -e {cmd}") } else { cmd }))
            .keywords(keywords)
            .subtitle(e.comment.clone().unwrap_or_default());
        item.icon = e.icon.clone();
        items.push(item);
    }
    for (arg, name, icon) in PREFERENCE_PANELS {
        items.push(
            Item::new(Kind::Preference, *name, Action::Spawn(vec!["tabula-preferences".into(), arg.to_string()]))
                .icon(*icon)
                .subtitle("Preferences")
                .keywords(format!("settings preferences {arg}")),
        );
    }
    items.push(Item::new(Kind::Setting, "Dark appearance", Action::Theme(true)).icon("appearance").keywords("dark mode theme night"));
    items.push(Item::new(Kind::Setting, "Light appearance", Action::Theme(false)).icon("appearance").keywords("light mode theme day"));
    items
}

/// The windows, and app_server's shortcuts with their keys now.
pub fn context() -> (Option<WindowInfo>, Vec<Item>) {
    let mut items = Vec::new();
    let Ok(mut client) = Client::connect() else { return (None, items) };
    let windows = match client.request(&Request::ListWindows) {
        Ok(Reply::Windows { windows }) => windows,
        _ => Vec::new(),
    };
    let changed: ShortcutSettings = match client.request(&Request::GetSettings) {
        Ok(Reply::Settings { settings }) => settings.shortcuts,
        _ => Default::default(),
    };
    let front = windows.iter().find(|w| w.focused).cloned();
    for info in shortcuts::SHORTCUTS.iter().filter(|i| i.action != tabula_protocol::Shortcut::Palette) {
        let keys = shortcuts::keys_for(info, &changed);
        let mut item = Item::new(Kind::Shortcut, info.title, Action::Shortcut(info.id.into()))
            .subtitle(info.group)
            .icon(shortcut_icon(info.group))
            .keywords(format!("{} {}{}", info.id.replace('-', " "), info.group, if info.action == tabula_protocol::Shortcut::Hide { " minimize minimise" } else { "" }));
        item.keys = keys.first().map(|c| c.symbols());
        // Window shortcuts need a window to act on.
        item.enabled = front.is_some() || info.group != shortcuts::WINDOWS;
        items.push(item);
    }
    for w in &windows {
        let (app, icon) = match our_app(&w.app_id) {
            Some((name, icon)) => (name.to_owned(), Some(icon.to_owned())),
            None => match desktop_entry::find(&w.app_id) {
                Some(e) => (e.name, e.icon),
                None => (w.app_id.clone(), None),
            },
        };
        let mut item = Item::new(Kind::Window, w.title.clone(), Action::Activate(w.id)).subtitle(app.clone()).keywords(format!("window switch {app}"));
        item.icon = icon.or(Some("application".into()));
        items.push(item);
    }
    (front, items)
}

fn shortcut_icon(group: &str) -> &'static str {
    match group {
        shortcuts::WORKSPACES => "workspaces",
        shortcuts::SCREENSHOTS => "screenshot",
        shortcuts::DESKTOP => "tabula",
        _ => "desktop",
    }
}

/// The commands in the menus of the app in front (as its menu bar has
/// them now).
pub fn menus(front: &WindowInfo) -> Vec<Item> {
    let Some(pid) = front.pid.and_then(|p| u32::try_from(p).ok()) else { return Vec::new() };
    let Some(m) = commands::menus(pid) else { return Vec::new() };
    let app = our_app(&m.app_id).map(|(n, _)| n.to_owned()).unwrap_or_else(|| m.app_id.clone());
    let icon = our_app(&m.app_id).map(|(_, i)| i.to_owned());
    m.entries
        .into_iter()
        .map(|e| {
            let path = e.path.join(" ▸ ");
            let mut item = Item::new(Kind::Command, e.label.trim_end_matches('…').to_owned(), Action::Menu { pid, command: e.command })
                .subtitle(format!("{app} ▸ {path}"))
                .keywords(format!("{} {}", e.path.join(" "), app));
            item.keys = e.keys;
            item.icon = icon.clone();
            item.enabled = e.enabled;
            item
        })
        .collect()
}

/// At most this many files are indexed.
const MAX_FILES: usize = 400_000;

/// Files under home, for finding by name: walked on start and every few
/// minutes (hidden files and what .gitignore files exclude are left out).
pub fn index_files(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(home).hidden(true).git_global(false).follow_links(false).max_depth(Some(12)).build().flatten() {
        if entry.depth() == 0 {
            continue;
        }
        out.push(entry.into_path());
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out
}

/// The best files for `words`, by file name.
pub fn find_files(paths: &[PathBuf], words: &[String], home: &Path) -> Vec<Item> {
    if words.iter().map(|w| w.chars().count()).sum::<usize>() < 2 {
        return Vec::new();
    }
    let mut hits: Vec<(i32, &PathBuf)> = paths
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            let lower = name.to_lowercase();
            if !words.iter().all(|w| lower.contains(w.as_str())) {
                return None;
            }
            crate::rank::score(words, name, "").map(|s| (s - p.components().count() as i32, p))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    hits.truncate(30);
    hits.into_iter()
        .map(|(_, p)| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let parent = p.parent().map(|d| match d.strip_prefix(home) {
                Ok(rel) if rel.as_os_str().is_empty() => "~".to_owned(),
                Ok(rel) => format!("~/{}", rel.display()),
                Err(_) => d.display().to_string(),
            });
            let icon = if p.is_dir() { "folder" } else { "document" };
            Item::new(Kind::File, name, Action::Open(p.clone())).subtitle(parent.unwrap_or_default()).icon(icon)
        })
        .collect()
}

/// Passwords' items for `query` (none while it's locked or not running).
pub fn find_passwords(query: &str) -> Vec<Item> {
    if query.trim().chars().count() < 2 {
        return Vec::new();
    }
    match tabula_keychain::call(&tabula_keychain::Request::Search { query: query.to_owned() }) {
        Ok(tabula_keychain::Response::Items { ref items }) => items
            .iter()
            .map(|f| {
                Item::new(Kind::Password, f.title.clone(), Action::Password { vault: f.vault.clone(), id: f.id.clone() })
                    .subtitle(f.subtitle.clone())
                    .icon("passwords")
            })
            .collect(),
        // Say why there are none, and offer the way in.
        Ok(tabula_keychain::Response::Locked) => vec![
            Item::new(Kind::Password, "Unlock Passwords to search its items", Action::Spawn(vec!["tabula-passwords".into()]))
                .subtitle("Passwords is locked")
                .icon("passwords")
                .keywords(query.to_lowercase()),
        ],
        _ => Vec::new(),
    }
}

/// The items matching `words`, best first (at most 60), with their scores:
/// how well each matches, what kind it is, how lately it was chosen
/// (`boost` of its recents key) and whether it can be done now.
pub fn ranked<'a>(words: &[String], items: impl Iterator<Item = &'a Item>, boost: impl Fn(&str) -> i32) -> Vec<(i32, Item)> {
    let mut scored: Vec<(i32, Item)> = Vec::new();
    for item in items {
        let haystack = format!("{} {}", item.keywords, item.subtitle.to_lowercase());
        let Some(s) = crate::rank::score(words, &item.title, &haystack) else { continue };
        let boost = item.action.key().map_or(0, |k| boost(&k));
        let disabled = if item.enabled { 0 } else { -20 };
        scored.push((s + item.kind.weight() + boost + disabled, item.clone()));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    scored.truncate(60);
    scored
}

/// Do `action` (on a worker thread: it may wait on other processes).
/// `front` is the window that was in front when the palette opened.
pub fn perform(action: Action, front: Option<u64>) {
    let _ = std::thread::Builder::new().name("palette-action".into()).spawn(move || {
        let ipc = |req: Request| {
            if let Ok(mut c) = Client::connect() {
                let _ = c.request(&req);
            }
        };
        let result = match action {
            Action::Launch(cmd) => tabula_kit::spawn_detached(&cmd).map_err(|e| e.to_string()),
            Action::Spawn(argv) => match tabula_kit::find_program(&argv[0]) {
                Some(path) => std::process::Command::new(path).args(&argv[1..]).spawn().map(|mut c| {
                    std::thread::spawn(move || c.wait());
                }).map_err(|e| e.to_string()),
                None => Err(format!("{} isn’t installed", argv[0])),
            },
            Action::Shortcut(id) => {
                ipc(Request::RunShortcut { id, window: front });
                Ok(())
            }
            Action::Activate(id) => {
                ipc(Request::Activate { id });
                Ok(())
            }
            Action::Menu { pid, command } => {
                // Its window comes forward as well, as after choosing from
                // its menu.
                if let Some(id) = front {
                    ipc(Request::Activate { id });
                }
                if commands::invoke(pid, &command) { Ok(()) } else { Err(format!("the app didn’t do “{command}”")) }
            }
            Action::Open(path) => tabula_kit::open::open(&tabula_kit::open::Target::Path(path)).map_err(|e| e.to_string()),
            Action::Password { vault, id } => {
                // Passwords may be quit: start it, then ask.
                if tabula_keychain::call(&tabula_keychain::Request::Show { vault: vault.clone(), id: id.clone() }).is_err()
                    && let Some(p) = tabula_kit::find_program("tabula-passwords")
                {
                    let _ = std::process::Command::new(p).spawn();
                    for _ in 0..40 {
                        std::thread::sleep(Duration::from_millis(250));
                        if tabula_keychain::call(&tabula_keychain::Request::Show { vault: vault.clone(), id: id.clone() }).is_ok() {
                            break;
                        }
                    }
                }
                Ok(())
            }
            Action::Copy(_) => Ok(()),
            Action::Theme(dark) => {
                if let Ok(mut c) = Client::connect()
                    && let Ok(Reply::Settings { mut settings }) = c.request(&Request::GetSettings)
                {
                    settings.appearance.theme = if dark { tabula_protocol::ThemePreference::Dark } else { tabula_protocol::ThemePreference::Light };
                    let _ = c.request(&Request::SetSettings { settings });
                }
                Ok(())
            }
            Action::Search(text) => tabula_kit::open::open(&tabula_kit::open::Target::Url(search_url(&text))).map_err(|e| e.to_string()),
        };
        if let Err(e) = result {
            tracing::warn!("palette: {e}");
        }
    });
}

/// A web search for `text` (Google, as Chrome would), percent-encoded.
fn search_url(text: &str) -> String {
    let mut q = String::new();
    for b in text.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => q.push(b as char),
            b' ' => q.push('+'),
            _ => q.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://www.google.com/search?q={q}")
}

/// Work done off the UI thread: one channel per kind, so a slow
/// Passwords never holds up files.
pub struct Workers {
    pub files: mpsc::Sender<(u64, Vec<String>)>,
    pub passwords: mpsc::Sender<(u64, String)>,
}

pub fn workers(proxy: Proxy) -> Workers {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    let (files, file_rx) = mpsc::channel::<(u64, Vec<String>)>();
    let to_ui = proxy.clone();
    let _ = std::thread::Builder::new().name("palette-files".into()).spawn(move || {
        let mut paths: Arc<Vec<PathBuf>> = Arc::new(Vec::new());
        let (fresh_tx, fresh_rx) = mpsc::channel();
        let walk_home = home.clone();
        let _ = std::thread::Builder::new().name("palette-index".into()).spawn(move || {
            loop {
                let found = index_files(&walk_home);
                if fresh_tx.send(found).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(300));
            }
        });
        let mut last: Option<(u64, Vec<String>)> = None;
        loop {
            // The newest query, or a fresh index for the last one.
            match file_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(mut q) => {
                    while let Ok(newer) = file_rx.try_recv() {
                        q = newer;
                    }
                    last = Some(q);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => match fresh_rx.try_recv() {
                    Ok(found) => paths = Arc::new(found),
                    Err(_) => continue,
                },
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            if let Ok(found) = fresh_rx.try_recv() {
                paths = Arc::new(found);
            }
            if let Some((n, words)) = &last {
                to_ui.send(Found::Files(*n, find_files(&paths, words, &home)));
            }
        }
    });
    let (passwords, pass_rx) = mpsc::channel::<(u64, String)>();
    let _ = std::thread::Builder::new().name("palette-passwords".into()).spawn(move || {
        while let Ok(mut q) = pass_rx.recv() {
            // Typing fast: wait for a pause, take the newest.
            std::thread::sleep(Duration::from_millis(120));
            while let Ok(newer) = pass_rx.try_recv() {
                q = newer;
            }
            proxy.send(Found::Passwords(q.0, find_passwords(&q.1)));
        }
    });
    Workers { files, passwords }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_urls() {
        assert_eq!(search_url(" rust & café "), "https://www.google.com/search?q=rust+%26+caf%C3%A9");
    }

    #[test]
    fn files_by_name() {
        let home = Path::new("/home/x");
        let paths: Vec<PathBuf> = ["/home/x/notes.txt", "/home/x/src/tabula/apps/palette", "/home/x/Pictures/beos-desktop.png"].iter().map(PathBuf::from).collect();
        let found = find_files(&paths, &crate::rank::words("beos"), home);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].subtitle, "~/Pictures");
        assert!(find_files(&paths, &crate::rank::words("p"), home).is_empty());
        assert_eq!(find_files(&paths, &crate::rank::words("palette"), home)[0].title, "palette");
    }

    fn titles(found: &[(i32, Item)]) -> Vec<&str> {
        found.iter().map(|(_, i)| i.title.as_str()).collect()
    }

    fn shortcut(id: &str, title: &str, group: &str, enabled: bool) -> Item {
        let mut i = Item::new(Kind::Shortcut, title, Action::Shortcut(id.into())).subtitle(group).keywords(format!("{} {group}", id.replace('-', " ")));
        i.enabled = enabled;
        i
    }

    fn file(path: &str) -> Item {
        let p = PathBuf::from(path);
        Item::new(Kind::File, p.file_name().unwrap().to_string_lossy(), Action::Open(p.clone())).subtitle("~/src")
    }

    #[test]
    fn shortcuts_before_files() {
        // Typing "tile" lists every tiling shortcut ahead of files named
        // tile…, even with no window in front to tile (disabled).
        let items = [
            file("/home/x/src/tile.h"),
            file("/home/x/src/tile_cache"),
            shortcut("tile-left", "Left half", "Windows", false),
            shortcut("tile-right", "Right half", "Windows", false),
            shortcut("tile-all", "Tile all windows on the display", "Arranging", true),
            shortcut("lock", "Lock the screen", "Desktop", true),
        ];
        let found = ranked(&crate::rank::words("tile"), items.iter(), |_| 0);
        assert_eq!(titles(&found), ["Tile all windows on the display", "Left half", "Right half", "tile.h", "tile_cache"]);
    }

    #[test]
    fn apps_before_commands_before_files() {
        let items = [
            file("/home/x/terminal.txt"),
            Item::new(Kind::Command, "Terminal Settings…", Action::Copy(String::new())),
            Item::new(Kind::Application, "Terminal", Action::Spawn(vec!["tabula-terminal".into()])),
        ];
        let found = ranked(&crate::rank::words("term"), items.iter(), |_| 0);
        assert_eq!(titles(&found), ["Terminal", "Terminal Settings…", "terminal.txt"]);
    }

    #[test]
    fn recently_chosen_comes_first() {
        let items = [shortcut("tile-left", "Left half", "Windows", true), shortcut("tile-right", "Right half", "Windows", true)];
        let found = ranked(&crate::rank::words("half"), items.iter(), |k| if k == "shortcut:tile-right" { 40 } else { 0 });
        assert_eq!(titles(&found), ["Right half", "Left half"]);
    }

    #[test]
    fn nothing_unmatched() {
        let items = [shortcut("lock", "Lock the screen", "Desktop", true), file("/home/x/src/notes.txt")];
        assert!(ranked(&crate::rank::words("zzzz"), items.iter(), |_| 0).is_empty());
    }

    #[test]
    fn recents_keys() {
        assert_eq!(Action::Shortcut("tile-left".into()).key().as_deref(), Some("shortcut:tile-left"));
        assert!(Action::Activate(3).key().is_none());
    }
}
