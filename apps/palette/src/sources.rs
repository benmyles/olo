//! What the palette can find, and what choosing each does. Everything
//! that touches another process (app_server, an app's menus, Passwords)
//! or the disk runs on a worker thread.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use olo_ui::{Proxy, commands};
use olo_xdg::{
    apps::{OUR_APPS, PREFERENCE_PANELS, our_app},
    desktop_entry,
};
use olo_protocol::{Client, Reply, Request, WindowInfo, shortcuts};

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
    /// A desktop entry's command line, and its id.
    Launch(String, String),
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
            Action::Launch(c, _) => format!("launch:{c}"),
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
    for (bin, name, icon) in OUR_APPS.iter().filter(|(bin, _, _)| olo_xdg::find_program(bin).is_some()) {
        ours.push(*bin);
        items.push(Item::new(Kind::Application, *name, Action::Spawn(vec![bin.to_string()])).icon(*icon).keywords(bin.trim_start_matches("olo-")));
    }
    items.push(Item::new(Kind::Application, "Preferences", Action::Spawn(vec!["olo-preferences".into()])).icon("preferences").keywords("settings"));
    for e in desktop_entry::all() {
        let Some(cmd) = e.command() else { continue };
        // Our own apps' entries are listed already.
        if e.no_display || ours.iter().any(|b| cmd.split_whitespace().next().is_some_and(|c| c.ends_with(b))) {
            continue;
        }
        let keywords = format!("{} {}", e.generic_name.clone().unwrap_or_default(), e.id.replace(['.', '-'], " "));
        let action = if e.terminal { Action::Launch(format!("olo-terminal -e {cmd}"), String::new()) } else { Action::Launch(cmd, e.id.clone()) };
        let mut item = Item::new(Kind::Application, e.name.clone(), action)
            .keywords(keywords)
            .subtitle(e.comment.clone().unwrap_or_default());
        item.icon = e.icon.clone();
        items.push(item);
    }
    for (arg, name, icon) in PREFERENCE_PANELS {
        items.push(
            Item::new(Kind::Preference, *name, Action::Spawn(vec!["olo-preferences".into(), arg.to_string()]))
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
    let profile = match client.request(&Request::GetSettings) {
        Ok(Reply::Settings { settings }) => settings.keyboard.profile,
        _ => Default::default(),
    };
    let front = windows.iter().find(|w| w.focused).cloned();
    for info in shortcuts::SHORTCUTS.iter().filter(|i| i.action != olo_protocol::Shortcut::Palette) {
        let keys = shortcuts::keys_for(info, &profile);
        let mut item = Item::new(Kind::Shortcut, info.title, Action::Shortcut(info.id.into()))
            .subtitle(info.group)
            .icon(shortcut_icon(info.group))
            .keywords(format!("{} {}{}", info.id.replace('-', " "), info.group, if info.action == olo_protocol::Shortcut::Hide { " minimize minimise" } else { "" }));
        item.keys = keys.first().map(|c| c.label(profile.platform));
        // Window shortcuts need a window to act on.
        item.enabled = front.is_some() || info.group != shortcuts::WINDOWS;
        items.push(item);
    }
    // The one in front last: switching to it does nothing.
    for w in windows.iter().filter(|w| !w.focused).chain(windows.iter().filter(|w| w.focused)) {
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
        shortcuts::DESKTOP => "olo",
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
/// How deep under home the index goes.
const MAX_DEPTH: usize = 12;

/// Files under home, for finding by name (hidden files and what
/// .gitignore files exclude are left out): walked once, then each folder
/// is followed (`olo_ui::watch`) and only a folder that changed is
/// listed again, so a new download is found at once and nothing is
/// walked on a timer. The kernel's watches are shared by every program:
/// past a share of them the index is walked again instead, when asked
/// and a few minutes old.
#[derive(Default)]
pub struct Index {
    pub files: std::collections::BTreeSet<PathBuf>,
    dirs: std::collections::BTreeSet<PathBuf>,
}

impl Index {
    fn depth(home: &Path, p: &Path) -> usize {
        p.strip_prefix(home).map_or(0, |r| r.components().count())
    }

    /// Add `root`'s items, and everything under them.
    pub fn walk(&mut self, home: &Path, root: &Path) {
        let left = MAX_DEPTH.saturating_sub(Self::depth(home, root));
        for entry in ignore::WalkBuilder::new(root).hidden(true).git_global(false).follow_links(false).max_depth(Some(left)).build().flatten() {
            if entry.depth() == 0 {
                continue;
            }
            if self.files.len() >= MAX_FILES {
                break;
            }
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                self.dirs.insert(entry.path().to_owned());
            }
            self.files.insert(entry.into_path());
        }
    }

    /// Forget `p` and everything under it.
    fn forget(&mut self, p: &Path) {
        for set in [&mut self.files, &mut self.dirs] {
            let gone: Vec<PathBuf> = set.range(p.to_owned()..).take_while(|q| q.starts_with(p)).cloned().collect();
            for g in gone {
                set.remove(&g);
            }
        }
    }

    /// Folder `dir` changed (or went): list it again, walking what is new
    /// in it.
    pub fn changed(&mut self, home: &Path, dir: &Path) {
        // (An item in a folder followed: its folder is listed again.)
        if dir != home && !self.dirs.contains(dir) {
            return;
        }
        if !dir.is_dir() {
            self.forget(dir);
            return;
        }
        let now: std::collections::BTreeSet<PathBuf> = ignore::WalkBuilder::new(dir)
            .hidden(true)
            .git_global(false)
            .follow_links(false)
            .max_depth(Some(1))
            .build()
            .flatten()
            .filter(|e| e.depth() == 1)
            .map(|e| e.into_path())
            .collect();
        let had: Vec<PathBuf> = self.files.range(dir.to_owned()..).take_while(|q| q.starts_with(dir)).filter(|q| q.parent() == Some(dir)).cloned().collect();
        for gone in had.iter().filter(|p| !now.contains(*p)) {
            self.forget(gone);
        }
        let new: Vec<PathBuf> = now.into_iter().filter(|p| !self.files.contains(p)).collect();
        for new in new {
            if self.files.len() >= MAX_FILES {
                break;
            }
            if new.is_dir() && Self::depth(home, &new) < MAX_DEPTH {
                self.dirs.insert(new.clone());
                self.walk(home, &new);
            }
            self.files.insert(new);
        }
    }

    /// The folders to follow: home and every folder in the index.
    pub fn folders(&self, home: &Path) -> Vec<PathBuf> {
        std::iter::once(home.to_owned()).chain(self.dirs.iter().cloned()).collect()
    }
}

/// How many folders the index may follow: a share of the kernel's watches.
fn watch_budget() -> usize {
    let max = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(8192);
    max / 8
}

/// The best files for `words`, by file name.
pub fn find_files<'a>(paths: impl IntoIterator<Item = &'a PathBuf>, words: &[String], home: &Path) -> Vec<Item> {
    if words.iter().map(|w| w.chars().count()).sum::<usize>() < 2 {
        return Vec::new();
    }
    let mut hits: Vec<(i32, &PathBuf)> = paths
        .into_iter()
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
    match olo_keychain::call(&olo_keychain::Request::Search { query: query.to_owned() }) {
        Ok(olo_keychain::Response::Items { ref items }) => items
            .iter()
            .map(|f| {
                Item::new(Kind::Password, f.title.clone(), Action::Password { vault: f.vault.clone(), id: f.id.clone() })
                    .subtitle(f.subtitle.clone())
                    .icon("passwords")
            })
            .collect(),
        // Say why there are none, and offer the way in.
        Ok(olo_keychain::Response::Locked) => vec![
            Item::new(Kind::Password, "Unlock Passwords to search its items", Action::Spawn(vec!["olo-passwords".into()]))
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
        let own = crate::rank::score(words, &item.title, &haystack);
        // A window is also found by its app's name, as the app is.
        let own = if item.kind == Kind::Window { own.max(crate::rank::score(words, &item.subtitle, "")) } else { own };
        let Some(s) = own else { continue };
        let boost = item.action.key().map_or(0, |k| boost(&k));
        let disabled = if item.enabled { 0 } else { -20 };
        scored.push((s + item.kind.weight() + boost + disabled, item.clone()));
    }
    // An app's open windows come just before the app: switching to one
    // is likelier wanted than another window.
    let apps: Vec<(String, i32)> = scored.iter().filter(|(_, i)| i.kind == Kind::Application).map(|(s, i)| (i.title.clone(), *s)).collect();
    for (s, item) in scored.iter_mut().filter(|(_, i)| i.kind == Kind::Window) {
        if let Some((_, a)) = apps.iter().find(|(t, _)| *t == item.subtitle) {
            *s = (*s).max(a + 1);
        }
    }
    // Windows that tie keep their order (the one in front last).
    let both_windows = |a: &Item, b: &Item| a.kind == Kind::Window && b.kind == Kind::Window;
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| if both_windows(&a.1, &b.1) { std::cmp::Ordering::Equal } else { a.1.title.cmp(&b.1.title) }));
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
            Action::Launch(cmd, id) => olo_xdg::spawn_detached_app(&cmd, &id).map_err(|e| e.to_string()),
            Action::Spawn(argv) => match olo_xdg::find_program(&argv[0]) {
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
            Action::Open(path) => olo_xdg::open::open(&olo_xdg::open::Target::Path(path)).map_err(|e| e.to_string()),
            Action::Password { vault, id } => {
                // Passwords may be quit: start it, then ask.
                if olo_keychain::call(&olo_keychain::Request::Show { vault: vault.clone(), id: id.clone() }).is_err()
                    && let Some(p) = olo_xdg::find_program("olo-passwords")
                {
                    let _ = std::process::Command::new(p).spawn();
                    for _ in 0..40 {
                        std::thread::sleep(Duration::from_millis(250));
                        if olo_keychain::call(&olo_keychain::Request::Show { vault: vault.clone(), id: id.clone() }).is_ok() {
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
                    settings.appearance.theme = if dark { olo_protocol::ThemePreference::Dark } else { olo_protocol::ThemePreference::Light };
                    let _ = c.request(&Request::SetSettings { settings });
                }
                Ok(())
            }
            Action::Search(text) => olo_xdg::open::open(&olo_xdg::open::Target::Url(search_url(&text))).map_err(|e| e.to_string()),
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
    let index = Arc::new(olo_sync::RwLock::new(Index::default()));
    // What wakes the search: a query, or the index changed under the last.
    enum Wake {
        Query(u64, Vec<String>),
        Index,
    }
    let (wake_tx, wake_rx) = mpsc::channel::<Wake>();
    let queries = wake_tx.clone();
    let _ = std::thread::Builder::new().name("palette-queries".into()).spawn(move || {
        while let Ok((n, words)) = file_rx.recv() {
            if queries.send(Wake::Query(n, words)).is_err() {
                return;
            }
        }
    });
    // The index: walked once, then kept as folders change.
    let (changed_tx, changed_rx) = mpsc::channel::<Vec<PathBuf>>();
    let rewalk = changed_tx.clone();
    let following = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (walk_home, walked, follows) = (home.clone(), index.clone(), following.clone());
    let _ = std::thread::Builder::new().name("palette-index".into()).spawn(move || {
        walked.write().unwrap_or_else(|e| e.into_inner()).walk(&walk_home, &walk_home);
        let _ = wake_tx.send(Wake::Index);
        let budget = watch_budget();
        let folders = walked.read().unwrap_or_else(|e| e.into_inner()).folders(&walk_home);
        let follow = folders.len() <= budget;
        let watch = if follow {
            let tx = changed_tx.clone();
            olo_ui::watch::watch(folders.clone(), move |dirs| {
                let _ = tx.send(dirs.to_vec());
            })
        } else {
            None
        };
        follows.store(watch.is_some(), std::sync::atomic::Ordering::Relaxed);
        if watch.is_none() {
            tracing::info!(folders = folders.len(), budget, "not following the file index's folders: it is walked again when asked");
        }
        let mut followed = folders.len();
        while let Ok(mut dirs) = changed_rx.recv() {
            // A burst (a copy, a checkout): every folder in it at once.
            while let Ok(more) = changed_rx.try_recv() {
                dirs.extend(more);
            }
            let mut ix = walked.write().unwrap_or_else(|e| e.into_inner());
            if dirs.is_empty() {
                // Asked again, not followed: walked whole.
                *ix = Index::default();
                ix.walk(&walk_home, &walk_home);
            }
            dirs.sort();
            dirs.dedup();
            for d in &dirs {
                ix.changed(&walk_home, d);
            }
            if let Some(w) = &watch {
                let folders = ix.folders(&walk_home);
                if folders.len() != followed {
                    followed = folders.len();
                    w.set(folders);
                }
            }
            drop(ix);
            let _ = wake_tx.send(Wake::Index);
        }
    });
    let (to_ui, search_index) = (proxy.clone(), index);
    let _ = std::thread::Builder::new().name("palette-files".into()).spawn(move || {
        let mut last: Option<(u64, Vec<String>)> = None;
        let mut walked_at = std::time::Instant::now();
        while let Ok(wake) = wake_rx.recv() {
            match wake {
                Wake::Query(n, words) => {
                    // Typing: only the newest matters.
                    let mut q = (n, words);
                    while let Ok(next) = wake_rx.try_recv() {
                        if let Wake::Query(n, w) = next {
                            q = (n, w);
                        }
                    }
                    last = Some(q);
                    // Not followed (too many folders): a fresh walk now and
                    // then, when asked.
                    if !following.load(std::sync::atomic::Ordering::Relaxed) && walked_at.elapsed() > Duration::from_secs(300) {
                        walked_at = std::time::Instant::now();
                        let _ = rewalk.send(Vec::new());
                    }
                }
                Wake::Index => {}
            }
            if let Some((n, words)) = &last {
                let ix = search_index.read().unwrap_or_else(|e| e.into_inner());
                let found = find_files(&ix.files, words, &home);
                drop(ix);
                to_ui.send(Found::Files(*n, found));
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
    fn the_index_follows_folders_that_change() {
        let home = std::env::temp_dir().join(format!("olo-palette-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Documents/old")).unwrap();
        std::fs::write(home.join("Documents/old/a.txt"), "a").unwrap();
        std::fs::write(home.join("Documents/b.txt"), "b").unwrap();
        let mut ix = Index::default();
        ix.walk(&home, &home);
        assert!(ix.files.contains(&home.join("Documents/old/a.txt")));
        // A folder gone and one new (with things in it), seen as their
        // parent changing: only that one is listed again.
        std::fs::remove_dir_all(home.join("Documents/old")).unwrap();
        std::fs::create_dir_all(home.join("Documents/new/deep")).unwrap();
        std::fs::write(home.join("Documents/new/deep/c.txt"), "c").unwrap();
        ix.changed(&home, &home.join("Documents"));
        assert!(!ix.files.iter().any(|p| p.starts_with(home.join("Documents/old"))));
        assert!(ix.files.contains(&home.join("Documents/new/deep/c.txt")) && ix.files.contains(&home.join("Documents/b.txt")));
        assert!(ix.folders(&home).contains(&home.join("Documents/new/deep")));
        // A folder that went, reported itself: forgotten with all inside.
        std::fs::remove_dir_all(home.join("Documents")).unwrap();
        ix.changed(&home, &home.join("Documents"));
        assert!(ix.files.is_empty(), "{:?}", ix.files);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn open_windows_before_their_app() {
        let app = Item::new(Kind::Application, "StyledEdit", Action::Spawn(vec!["olo-styled-edit".into()])).keywords("styled-edit");
        let w = |id, title: &str| Item::new(Kind::Window, title, Action::Activate(id)).subtitle("StyledEdit").keywords("window switch styledit");
        let other = Item::new(Kind::Window, "styledit notes", Action::Activate(9)).subtitle("Terminal");
        let items = [app, w(2, "zeta.txt"), w(1, "alpha.txt"), other];
        let titles: Vec<String> = ranked(&crate::rank::words("styled"), items.iter(), |_| 0).into_iter().map(|(_, i)| i.title).collect();
        assert_eq!(titles[..3], ["zeta.txt", "alpha.txt", "StyledEdit"]);
        let titles: Vec<String> = ranked(&crate::rank::words("StyledEdit"), items.iter(), |_| 0).into_iter().map(|(_, i)| i.title).collect();
        assert_eq!(titles[..3], ["zeta.txt", "alpha.txt", "StyledEdit"]);
    }

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
            Item::new(Kind::Application, "Terminal", Action::Spawn(vec!["olo-terminal".into()])),
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
