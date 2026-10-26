//! Tracker: the desktop (icons for volumes, Home, Trash and ~/Desktop) and
//! spatial folder windows, one per folder, as on BeOS.

mod columns;
mod desktop;
mod find;
mod fs;
mod ops;
mod view;

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key},
    metrics as m,
    widgets::{self, Button, Column, ColumnListView, ListState, TextControl},
};
use tabula_protocol::{Client, Reply, Request};

pub use fs::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ViewMode {
    #[default]
    Icons,
    MiniIcons,
    List,
    /// macOS-style columns: each selected folder opens in the next column.
    Columns,
}

/// A column to the right of the root in column view.
pub struct Pane {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub list: ListState,
    pub stamp: Option<SystemTime>,
    /// The listing request whose result we are waiting for (see `Loader`).
    pub generation: u64,
    pub loading: bool,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Pane { path, entries: Vec::new(), list: ListState::default(), stamp: None, generation: 0, loading: true }
    }
}

/// Folders are listed on worker threads (listing stats every item and can
/// take a while on big or slow folders); results come back as
/// `Msg::Listed` to the window that asked.
#[derive(Clone)]
pub struct Loader {
    win: WindowId,
    proxy: tabula_kit::Proxy,
}

/// Put `v` in place of `entries`, keeping the selection (by path) and
/// selecting `extra` too.
fn replace_entries(entries: &mut Vec<Entry>, list: &mut ListState, mut v: Vec<Entry>, sort: (SortKey, bool), extra: &mut Vec<PathBuf>) {
    let mut keep: Vec<PathBuf> = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
    keep.append(extra);
    sort_entries(&mut v, sort.0, sort.1);
    *entries = v;
    list.selected = entries.iter().enumerate().filter(|(_, e)| keep.contains(&e.path)).map(|(i, _)| i).collect();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

pub struct FolderWin {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub view: ViewMode,
    pub list: ListState,
    pub sort: (SortKey, bool),
    pub stamp: Option<SystemTime>,
    pub checked: Instant,
    pub error: Option<String>,
    /// Rubber-band selection start, in content coordinates.
    pub band: Option<egui::Pos2>,
    /// Type-ahead buffer and when it was last typed into.
    pub typed: (String, Instant),
    /// The item whose name is being edited in place, and the new name.
    pub editing: Option<(PathBuf, String)>,
    /// A click on an already-selected item's name starts editing it after
    /// a moment, unless a double-click (open) follows.
    pub pending_edit: Option<(PathBuf, Instant)>,
    /// Column view: the columns after the root, which column has the
    /// keyboard (0 is the root), and how far the columns are scrolled.
    pub panes: Vec<Pane>,
    pub active: usize,
    pub col_scroll: f32,
    /// Whether hidden files are listed (Tracker-wide; kept for new columns).
    pub hidden: bool,
    loader: Loader,
    /// Listing requests: the last one made, the one the root waits for,
    /// and how many items the root's listing has found so far.
    generation: u64,
    root_generation: u64,
    pub loading: bool,
    pub listed: Arc<AtomicU64>,
    /// Paths to select once a listing in progress brings them in.
    pending_select: Vec<PathBuf>,
}

struct InfoWin {
    entry: Entry,
    size: Option<(u64, u64)>,
}

struct FindWin {
    query: String,
    root: usize,
    /// Index into `find::Mode::LABELS`.
    mode: usize,
    everything: bool,
    results: Vec<find::Hit>,
    /// The mode the current results were found with.
    shown_mode: find::Mode,
    list: ListState,
    running: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<find::Progress>,
    /// The last search was stopped before it finished.
    stopped: bool,
    started: Instant,
    took: Duration,
}

enum DialogKind {
    EmptyTrash,
    Error,
}

struct Dialog {
    kind: DialogKind,
    message: String,
}

enum Win {
    Folder(FolderWin),
    Desktop(desktop::Desktop),
    Info(InfoWin),
    Find(FindWin),
    Dialog(Dialog),
    /// "Tracker Status": progress of running file operations.
    Status,
}

enum Msg {
    /// Another `tabula-tracker` invocation forwarded its arguments.
    Remote(Vec<String>),
    DirSize(WindowId, u64, u64),
    Found(WindowId, u64, Vec<find::Hit>, bool),
    JobDone(u64, ops::Outcome),
    Listed { win: WindowId, path: PathBuf, generation: u64, result: Result<Vec<Entry>, String> },
    /// A job has been running a while: time to show the status window.
    JobSlow(u64),
}

pub struct Tracker {
    windows: HashMap<WindowId, Win>,
    /// Paths cut or copied with ⌘X / ⌘C, and whether it was a cut.
    clipboard: Option<(Vec<PathBuf>, bool)>,
    hidden: bool,
    /// Running file operations, each with the window that started it.
    jobs: Vec<(ops::Job, Option<WindowId>)>,
    next_job: u64,
    status_win: Option<WindowId>,
    /// Content height the status window was last sized to.
    status_size: u32,
}

const STATUS_WIDTH: u32 = 420;
/// Jobs that finish sooner than this never show the status window.
const STATUS_DELAY: Duration = Duration::from_millis(400);

fn sort_entries(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| {
        // Folders first, then the chosen key; names break ties.
        let folders = b.is_dir.cmp(&a.is_dir);
        let name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let o = match key {
            SortKey::Name => name(),
            SortKey::Size => a.size.cmp(&b.size).then_with(name),
            SortKey::Modified => a.modified.cmp(&b.modified).then_with(name),
            SortKey::Kind => a.kind.name().cmp(b.kind.name()).then_with(name),
        };
        folders.then(if asc { o } else { o.reverse() })
    });
}

impl FolderWin {
    fn new(path: PathBuf, hidden: bool, loader: Loader) -> Self {
        let mut w = FolderWin {
            path,
            entries: Vec::new(),
            view: ViewMode::Icons,
            list: ListState { multiple: true, ..Default::default() },
            sort: (SortKey::Name, true),
            stamp: None,
            checked: Instant::now(),
            error: None,
            band: None,
            typed: (String::new(), Instant::now()),
            editing: None,
            pending_edit: None,
            panes: Vec::new(),
            active: 0,
            col_scroll: 0.0,
            hidden,
            loader,
            generation: 0,
            root_generation: 0,
            loading: true,
            listed: Arc::default(),
            pending_select: Vec::new(),
        };
        w.reload(hidden);
        w
    }

    /// Relist the folder (and any columns) in the background.
    pub fn reload(&mut self, hidden: bool) {
        self.hidden = hidden;
        self.request(None);
        for k in 0..self.panes.len() {
            self.request(Some(k));
        }
    }

    /// List the root (`None`) or column `k` on a worker thread.
    fn request(&mut self, pane: Option<usize>) {
        self.generation += 1;
        let generation = self.generation;
        let path = match pane {
            None => {
                self.root_generation = generation;
                self.loading = true;
                self.listed = Arc::default();
                // Taken before listing, so changes made meanwhile show up
                // on the next check.
                self.stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
                self.path.clone()
            }
            Some(k) => {
                let p = &mut self.panes[k];
                p.generation = generation;
                p.loading = true;
                p.stamp = std::fs::metadata(&p.path).and_then(|m| m.modified()).ok();
                p.path.clone()
            }
        };
        let (hidden, counter, Loader { win, proxy }) = (self.hidden, self.listed.clone(), self.loader.clone());
        let counter = if pane.is_none() { counter } else { Arc::default() };
        std::thread::spawn(move || {
            let result = fs::list_counting(&path, hidden, &counter).map_err(|e| e.to_string());
            proxy.send(Msg::Listed { win, path, generation, result });
        });
    }

    /// A listing came back; ignore it if a newer one was asked for since.
    fn listed(&mut self, path: &Path, generation: u64, result: Result<Vec<Entry>, String>) {
        let sort = self.sort;
        if path == self.path && generation == self.root_generation {
            self.loading = false;
            match result {
                Ok(v) => {
                    replace_entries(&mut self.entries, &mut self.list, v, sort, &mut Vec::new());
                    self.error = None;
                }
                Err(e) => {
                    self.entries.clear();
                    self.list.selected.clear();
                    self.error = Some(e);
                }
            }
        } else if let Some(k) = self.panes.iter().position(|p| p.path == path && p.generation == generation) {
            match result {
                Ok(v) => {
                    let p = &mut self.panes[k];
                    p.loading = false;
                    replace_entries(&mut p.entries, &mut p.list, v, sort, &mut Vec::new());
                }
                // The folder is gone: so are it and the columns after it.
                Err(_) => {
                    self.panes.truncate(k);
                    self.active = self.active.min(self.panes.len());
                }
            }
        } else {
            return;
        }
        if !self.pending_select.is_empty() {
            let pending = std::mem::take(&mut self.pending_select);
            self.select_paths(&pending);
        }
    }

    /// Re-sort what is shown (no relisting needed).
    pub fn resort(&mut self) {
        let sort = self.sort;
        let v = std::mem::take(&mut self.entries);
        replace_entries(&mut self.entries, &mut self.list, v, sort, &mut Vec::new());
        for p in &mut self.panes {
            let v = std::mem::take(&mut p.entries);
            replace_entries(&mut p.entries, &mut p.list, v, sort, &mut Vec::new());
        }
    }

    /// Show a new item right away (before the next listing lands), so it
    /// can be selected or renamed at once.
    pub fn add_entry(&mut self, path: &Path) {
        let Some(e) = fs::entry(path) else { return };
        let sort = self.sort;
        let (entries, list) = self.current_mut();
        if entries.iter().any(|x| x.path == e.path) {
            return;
        }
        let mut v = entries.clone();
        v.push(e);
        replace_entries(entries, list, v, sort, &mut Vec::new());
    }

    /// The folder commands act on: the window's, or in column view the
    /// active column's.
    pub fn current_dir(&self) -> PathBuf {
        match self.active_pane() {
            Some(p) => p.path.clone(),
            None => self.path.clone(),
        }
    }

    fn active_pane(&self) -> Option<&Pane> {
        if self.view == ViewMode::Columns && self.active > 0 { self.panes.get(self.active - 1) } else { None }
    }

    /// The entries and selection commands act on (see `current_dir`).
    pub fn current(&self) -> (&[Entry], &ListState) {
        match self.active_pane() {
            Some(p) => (&p.entries, &p.list),
            None => (&self.entries, &self.list),
        }
    }

    pub fn current_mut(&mut self) -> (&mut Vec<Entry>, &mut ListState) {
        if self.view == ViewMode::Columns && self.active > 0 && self.active <= self.panes.len() {
            let p = &mut self.panes[self.active - 1];
            (&mut p.entries, &mut p.list)
        } else {
            (&mut self.entries, &mut self.list)
        }
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        let (entries, list) = self.current();
        list.selected.iter().filter_map(|&i| entries.get(i)).cloned().collect()
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries().into_iter().map(|e| e.path).collect()
    }

    /// Column view: show `dir` as the column after column `k`.
    pub fn open_column(&mut self, k: usize, dir: PathBuf, hidden: bool) {
        self.hidden = hidden;
        self.panes.truncate(k);
        self.panes.push(Pane::new(dir));
        self.request(Some(k));
    }

    /// Start editing the name of `p` in place.
    pub fn edit_name(&mut self, p: &Path) {
        self.select_path(p);
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.editing = Some((p.to_owned(), name));
        self.pending_edit = None;
    }

    fn select_path(&mut self, p: &Path) {
        let (entries, list) = self.current_mut();
        if let Some(i) = entries.iter().position(|e| e.path == p) {
            list.select(i);
        }
    }

    /// Select exactly `paths`; any not listed yet are selected when the
    /// listing in progress brings them in.
    fn select_paths(&mut self, paths: &[PathBuf]) {
        let (entries, list) = self.current_mut();
        let rows: Vec<usize> = (0..entries.len()).filter(|&i| paths.contains(&entries[i].path)).collect();
        let missing: Vec<PathBuf> = paths.iter().filter(|p| !entries.iter().any(|e| &&e.path == p)).cloned().collect();
        if let Some(&first) = rows.first() {
            list.select(first);
            list.selected.extend(rows);
        } else {
            list.selected.clear();
        }
        self.pending_select = missing;
    }
}

fn folder_options(path: &Path) -> WindowOptions {
    WindowOptions::new(fs::display_name(path), 560, 380).min_size(240, 160)
}

/// Bring an existing Tracker window for `title` forward via app_server.
fn activate_existing(title: &str) -> bool {
    let Ok(mut c) = Client::connect() else { return false };
    if let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) {
        if let Some(w) = windows.iter().find(|w| w.app_id == "tabula.tracker" && w.title == title) {
            let _ = c.request(&Request::Activate { id: w.id });
            return true;
        }
    }
    false
}

impl Tracker {
    fn open_folder(&mut self, cx: &mut Cx, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let open = self.windows.iter().find(|(_, w)| matches!(w, Win::Folder(f) if f.path == path)).map(|(id, _)| *id);
        if open.is_some() {
            activate_existing(&fs::display_name(&path));
            return;
        }
        let id = cx.open_window(folder_options(&path));
        let loader = Loader { win: id, proxy: cx.proxy() };
        self.windows.insert(id, Win::Folder(FolderWin::new(path, self.hidden, loader)));
    }

    fn open_entry(&mut self, cx: &mut Cx, e: &Entry) {
        if e.is_dir {
            self.open_folder(cx, &e.path);
        } else {
            fs::open(e);
        }
    }

    fn open_info(&mut self, cx: &mut Cx, e: Entry) {
        let h = view::info_height(view::info_rows(&e)).ceil() as u32;
        let id = cx.open_window(WindowOptions::new(format!("{} info", e.name), 360, h).min_size(300, h));
        if e.is_dir {
            let proxy = cx.proxy();
            let path = e.path.clone();
            std::thread::spawn(move || {
                let mut limit = 200_000;
                let (b, f) = fs::dir_size(&path, &mut limit);
                proxy.send(Msg::DirSize(id, b, f));
            });
        }
        self.windows.insert(id, Win::Info(InfoWin { size: None, entry: e }));
    }

    fn open_find(&mut self, cx: &mut Cx) {
        let id = cx.open_window(WindowOptions::new("Find", 680, 420).min_size(440, 260));
        self.windows.insert(
            id,
            Win::Find(FindWin {
                query: String::new(),
                root: 0,
                mode: 0,
                everything: false,
                results: Vec::new(),
                shown_mode: find::Mode::Names,
                list: ListState::default(),
                running: false,
                generation: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::default(),
                stopped: false,
                started: Instant::now(),
                took: Duration::ZERO,
            }),
        );
    }

    fn dialog(&mut self, cx: &mut Cx, title: &str, kind: DialogKind, message: String) {
        let h = widgets::alert_height(&message);
        let id = cx.open_window(WindowOptions::new(title, widgets::ALERT_WIDTH, h).min_size(widgets::ALERT_WIDTH, h));
        self.windows.insert(id, Win::Dialog(Dialog { kind, message }));
    }

    fn error(&mut self, cx: &mut Cx, message: String) {
        self.dialog(cx, "Tracker", DialogKind::Error, message);
    }

    fn reload_all(&mut self) {
        let hidden = self.hidden;
        for w in self.windows.values_mut() {
            match w {
                Win::Folder(f) => f.reload(hidden),
                Win::Desktop(d) => d.reload(),
                _ => {}
            }
        }
    }

    fn handle_args(&mut self, cx: &mut Cx, args: &[String]) {
        let mut opened = false;
        for a in args {
            match a.as_str() {
                "--find" => {
                    self.open_find(cx);
                    opened = true;
                }
                "--desktop" => {}
                p => {
                    let path = PathBuf::from(p);
                    if path.is_dir() {
                        self.open_folder(cx, &path);
                    } else if let Some(e) = fs::entry(&path) {
                        fs::open(&e);
                    }
                    opened = true;
                }
            }
        }
        if !opened && !args.iter().any(|a| a == "--desktop") {
            self.open_folder(cx, &fs::home());
        }
    }

    // ---- folder commands --------------------------------------------------

    fn folder_command(&mut self, cx: &mut Cx, win: WindowId, cmd: &str) {
        let Some(Win::Folder(f)) = self.windows.get_mut(&win) else { return };
        let selected = f.selected_paths();
        let dir = f.current_dir();
        match cmd {
            "open" => {
                for e in f.selected_entries() {
                    self.open_entry(cx, &e);
                }
            }
            "parent" => {
                if let Some(p) = dir.parent() {
                    let p = p.to_owned();
                    self.open_folder(cx, &p);
                }
            }
            "new-folder" => match fs::new_folder(&dir) {
                Ok(p) => {
                    f.add_entry(&p);
                    f.edit_name(&p);
                }
                Err(e) => self.error(cx, format!("Couldn't create a folder: {e}")),
            },
            "info" => {
                let entries = f.selected_entries();
                if entries.is_empty() {
                    if let Some(e) = fs::entry(&dir) {
                        self.open_info(cx, e);
                    }
                }
                for e in entries {
                    self.open_info(cx, e);
                }
            }
            "rename" => {
                if let [p] = selected.as_slice() {
                    f.edit_name(p);
                }
            }
            "duplicate" => {
                if !selected.is_empty() {
                    self.start_job(cx, ops::Kind::Duplicate, selected, None, Some(win));
                }
            }
            "link" => {
                for p in &selected {
                    let _ = fs::make_link(p);
                }
                self.reload_all();
            }
            "trash" => {
                if !selected.is_empty() {
                    self.start_job(cx, ops::Kind::Trash, selected, None, Some(win));
                }
            }
            "empty-trash" => self.dialog(
                cx,
                "Empty Trash",
                DialogKind::EmptyTrash,
                "Are you sure you want to permanently remove all items from the Trash?".into(),
            ),
            "cut" | "copy" => {
                if !selected.is_empty() {
                    let text = selected.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
                    cx.copy(text);
                    self.clipboard = Some((selected, cmd == "cut"));
                }
            }
            "paste" => {
                if let Some((paths, cut)) = self.clipboard.clone() {
                    if cut {
                        self.clipboard = None;
                    }
                    let kind = if cut { ops::Kind::Move } else { ops::Kind::Copy };
                    self.start_job(cx, kind, paths, Some(dir), Some(win));
                }
            }
            "select-all" => {
                let (entries, list) = f.current_mut();
                list.selected = (0..entries.len()).collect();
            }
            "invert" => {
                let (entries, list) = f.current_mut();
                list.selected = (0..entries.len()).filter(|i| !list.selected.contains(i)).collect();
            }
            "view-icons" => f.view = ViewMode::Icons,
            "view-mini" => f.view = ViewMode::MiniIcons,
            "view-list" => f.view = ViewMode::List,
            "view-columns" => {
                f.view = ViewMode::Columns;
                f.active = 0;
            }
            "hidden" => {
                self.hidden = !self.hidden;
                self.reload_all();
            }
            "sort-name" | "sort-size" | "sort-modified" | "sort-kind" => {
                let key = match cmd {
                    "sort-size" => SortKey::Size,
                    "sort-modified" => SortKey::Modified,
                    "sort-kind" => SortKey::Kind,
                    _ => SortKey::Name,
                };
                f.sort = (key, if f.sort.0 == key { !f.sort.1 } else { true });
                f.resort();
            }
            "go-home" => self.open_folder(cx, &fs::home()),
            "go-desktop" => self.open_folder(cx, &fs::home().join("Desktop")),
            "go-root" => self.open_folder(cx, Path::new("/")),
            "go-trash" => {
                let t = fs::trash_files();
                let _ = std::fs::create_dir_all(&t);
                self.open_folder(cx, &t);
            }
            "find" => self.open_find(cx),
            "close" => {
                cx.close_window(win);
                self.windows.remove(&win);
            }
            "quit" => cx.quit(),
            _ => {}
        }
        cx.redraw(win);
    }

    fn folder_menus(&self, f: &FolderWin) -> Vec<Menu> {
        let current = f.current().1;
        let has_sel = !current.selected.is_empty();
        let in_trash = f.current_dir().starts_with(fs::trash_files());
        let mut file = Menu::new("File")
            .item("Find…", "find", Some(Key::F))
            .item("New Folder", "new-folder", Some(Key::N))
            .enabled(!in_trash)
            .separator()
            .item("Open", "open", Some(Key::O))
            .enabled(has_sel)
            .item("Open Parent", "parent", Some(Key::ArrowUp))
            .enabled(f.path.parent().is_some())
            .item("Get Info", "info", Some(Key::I))
            .item("Edit Name", "rename", Some(Key::E))
            .enabled(current.selected.len() == 1)
            .item("Duplicate", "duplicate", Some(Key::D))
            .enabled(has_sel)
            .item("Create Link", "link", None)
            .enabled(has_sel)
            .separator()
            .item("Move to Trash", "trash", Some(Key::T))
            .enabled(has_sel && !in_trash);
        if in_trash {
            file = file.item("Empty Trash", "empty-trash", None).enabled(!fs::trash_is_empty());
        }
        let file = file.separator().item("Close", "close", Some(Key::W)).item("Quit", "quit", Some(Key::Q));
        let edit = Menu::new("Edit")
            .item("Cut", "cut", Some(Key::X))
            .enabled(has_sel)
            .item("Copy", "copy", Some(Key::C))
            .enabled(has_sel)
            .item("Paste", "paste", Some(Key::V))
            .enabled(self.clipboard.is_some())
            .separator()
            .item("Select All", "select-all", Some(Key::A))
            .item("Invert Selection", "invert", Some(Key::S));
        let sort = |k: SortKey| f.sort.0 == k;
        let window = Menu::new("Window")
            .item("Icon View", "view-icons", Some(Key::Num1))
            .checked(f.view == ViewMode::Icons)
            .item("Mini Icon View", "view-mini", Some(Key::Num2))
            .checked(f.view == ViewMode::MiniIcons)
            .item("List View", "view-list", Some(Key::Num3))
            .checked(f.view == ViewMode::List)
            .item("Column View", "view-columns", Some(Key::Num4))
            .checked(f.view == ViewMode::Columns)
            .separator()
            .submenu(
                Menu::new("Sort By")
                    .item("Name", "sort-name", None)
                    .checked(sort(SortKey::Name))
                    .item("Size", "sort-size", None)
                    .checked(sort(SortKey::Size))
                    .item("Modified", "sort-modified", None)
                    .checked(sort(SortKey::Modified))
                    .item("Kind", "sort-kind", None)
                    .checked(sort(SortKey::Kind)),
            )
            .item("Show Hidden Files", "hidden", None)
            .checked(self.hidden);
        let go = Menu::new("Go")
            .item("Home", "go-home", None)
            .item("Desktop", "go-desktop", None)
            .item(fs::volume_label(), "go-root", None)
            .item("Trash", "go-trash", None);
        vec![file, edit, window, go]
    }

    // ---- folder window UI ---------------------------------------------------

    fn folder_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let hidden = self.hidden;
        let Some(Win::Folder(f)) = self.windows.get_mut(&win) else { return };

        // Keep up with changes on disk.
        if f.checked.elapsed() >= Duration::from_secs(1) {
            f.checked = Instant::now();
            let stamp = std::fs::metadata(&f.path).and_then(|m| m.modified()).ok();
            let panes_changed =
                f.panes.iter().any(|p| std::fs::metadata(&p.path).and_then(|m| m.modified()).ok() != p.stamp);
            if stamp != f.stamp || panes_changed {
                f.reload(hidden);
            }
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));

        let actions = view::folder_view(ui, f);
        if !actions.is_empty() {
            // Actions change state the next frame shows (an edit starting).
            ui.ctx().request_repaint();
        }
        for a in actions {
            match a {
                view::Action::Open(e) => self.open_entry(cx, &e),
                view::Action::Command(c) => self.folder_command(cx, win, &c),
                view::Action::Rename(p, name) => {
                    if let Some(new) = self.rename(cx, &p, &name)
                        && let Some(Win::Folder(f)) = self.windows.get_mut(&win)
                    {
                        f.select_paths(&[new]);
                    }
                }
            }
        }
    }

    /// Rename `p` to `name`; reports failures and returns the new path.
    fn rename(&mut self, cx: &mut Cx, p: &Path, name: &str) -> Option<PathBuf> {
        let current = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = name.trim();
        if name.is_empty() || name == current {
            return None;
        }
        let r = fs::rename(p, name);
        self.reload_all();
        match r {
            Ok(new) => Some(new),
            Err(e) => {
                self.error(cx, format!("Couldn't rename “{current}”: {e}"));
                None
            }
        }
    }

    fn info_ui(&mut self, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Info(info)) = self.windows.get(&win) else { return };
        view::info_view(ui, &info.entry, info.size);
    }

    fn find_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Find(fw)) = self.windows.get_mut(&win) else { return };
        let mut start = false;
        let mut open = None;
        widgets::window_content(ui, |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 80.0 - m::SPACING;
                let r = ui.add(TextControl::new(&mut fw.query).label("Find:").width(w).hint("Name or text").autofocus(true));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    start = true;
                }
                if ui.add(Button::new(if fw.running { "Stop" } else { "Find" }).default(true).min_width(80.0)).clicked() {
                    start = true;
                }
            });
            ui.horizontal(|ui| {
                ui.add(widgets::MenuField::new(("find-mode", win), &mut fw.mode, &find::Mode::LABELS).label("Match:"));
                ui.add_space(m::SPACING);
                ui.add(widgets::MenuField::new(("find-root", win), &mut fw.root, &["Home", "Entire disk"]).label("In:"));
                ui.add_space(m::SPACING);
                ui.add(widgets::CheckBox::new(&mut fw.everything, "Hidden and ignored files"));
            });
            ui.add_space(m::TIGHT);
            let contents = fw.shown_mode != find::Mode::Names;
            let mut cols = vec![Column::new("Name", 200.0), Column::new("Location", 170.0)];
            if contents {
                cols.push(Column::new("Line", 44.0).right());
                cols.push(Column::new("Match", 300.0));
            } else {
                cols.push(Column::new("Size", 70.0).right());
                cols.push(Column::new("Modified", 170.0));
            }
            let results = &fw.results;
            // Leave a row under the list for the status line.
            let list_size = egui::vec2(ui.available_width(), ui.available_height() - m::CONTROL_HEIGHT);
            let r = ui.allocate_ui(list_size, |ui| ColumnListView::new(("find-results", win), &cols, results.len(), &mut fw.list).show(ui, |cell| {
                let h = &results[cell.row];
                let e = &h.entry;
                match (cell.column, contents) {
                    (0, _) => view::name_cell(cell, e),
                    (1, _) => cell.text(&e.path.parent().map(fs::display_path).unwrap_or_default()),
                    (2, true) => cell.text(&h.line.as_ref().map(|(n, _)| n.to_string()).unwrap_or_default()),
                    (_, true) => cell.text(h.line.as_ref().map(|(_, t)| t.as_str()).unwrap_or("")),
                    (2, false) => cell.text(&if e.is_dir { "—".into() } else { fs::format_size(e.size) }),
                    _ => cell.text(&fs::format_time(e.modified)),
                }
            })).inner;
            if let Some(i) = r.activated {
                open = results.get(i).map(|h| h.entry.clone());
            }
            let status = if fw.running {
                // Progress comes from the workers' counters; poll it while
                // they run.
                ui.ctx().request_repaint_after(Duration::from_millis(100));
                let scanned = fw.progress.scanned.load(Ordering::Relaxed);
                let folder = fw.progress.folder.lock().map(|f| fs::display_path(&f)).unwrap_or_default();
                format!("{} found · {scanned} items searched · {folder}", fw.results.len())
            } else if fw.generation > 0 {
                let found = match fw.results.len() {
                    1 => "1 item found".to_owned(),
                    n => format!("{n} items found"),
                };
                if fw.stopped {
                    format!("Stopped · {found}")
                } else {
                    format!("{found} in {:.1} s", fw.took.as_secs_f32())
                }
            } else {
                String::new()
            };
            ui.add(widgets::StringView::new(&status).dim(true));
        });
        if fw.running && ui.input(|i| i.key_pressed(Key::Escape)) {
            start = true; // Escape stops, like the Stop button
        }
        if start {
            fw.cancel.store(true, Ordering::Relaxed);
            if fw.running {
                fw.running = false;
                fw.stopped = true;
                fw.generation += 1;
            } else if !fw.query.trim().is_empty() {
                fw.stopped = false;
                fw.started = Instant::now();
                fw.generation += 1;
                fw.running = true;
                fw.results.clear();
                fw.list = ListState::default();
                fw.cancel = Arc::new(AtomicBool::new(false));
                fw.progress = Arc::default();
                fw.shown_mode = find::Mode::from_index(fw.mode);
                let query = find::Query {
                    root: if fw.root == 0 { fs::home() } else { PathBuf::from("/") },
                    text: fw.query.clone(),
                    mode: fw.shown_mode,
                    everything: fw.everything,
                };
                let (proxy, generation_id) = (cx.proxy(), fw.generation);
                find::spawn(query, fw.cancel.clone(), fw.progress.clone(), move |hits, done| {
                    proxy.send(Msg::Found(win, generation_id, hits, done));
                });
            }
        }
        if let Some(e) = open {
            self.open_entry(cx, &e);
        }
    }

    fn dialog_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Dialog(d)) = self.windows.get(&win) else { return };
        let (icon, buttons): (&str, &[&str]) = match d.kind {
            DialogKind::EmptyTrash => ("trash-full", &["Cancel", "Empty Trash"]),
            DialogKind::Error => ("about", &["OK"]),
        };
        let Some(choice) = widgets::alert(ui, icon, &d.message, buttons) else { return };
        let Some(Win::Dialog(d)) = self.windows.remove(&win) else { return };
        cx.close_window(win);
        if choice + 1 == buttons.len()
            && let DialogKind::EmptyTrash = d.kind
        {
            self.start_job(cx, ops::Kind::EmptyTrash, Vec::new(), None, None);
        }
    }
}


impl App for Tracker {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get(&window) {
            Some(Win::Folder(_)) => self.folder_ui(cx, window, ui),
            Some(Win::Desktop(_)) => {
                let Some(Win::Desktop(d)) = self.windows.get_mut(&window) else { return };
                let actions = d.ui(ui);
                if !actions.is_empty() {
                    ui.ctx().request_repaint();
                }
                for a in actions {
                    match a {
                        view::Action::Open(e) => self.open_entry(cx, &e),
                        view::Action::Rename(p, name) => {
                            self.rename(cx, &p, &name);
                        }
                        view::Action::Command(c) => match c.as_str() {
                            "new-folder" => {
                                let dir = fs::home().join("Desktop");
                                let _ = std::fs::create_dir_all(&dir);
                                if let Ok(p) = fs::new_folder(&dir)
                                    && let Some(Win::Desktop(d)) = self.windows.get_mut(&window)
                                {
                                    d.reload();
                                    d.edit_name(&p);
                                }
                            }
                            "empty-trash" => self.folder_command_global(cx, "empty-trash"),
                            "backgrounds" => {
                                let _ = tabula_kit::spawn_detached("tabula-preferences backgrounds");
                            }
                            "trash" => {
                                let paths = match self.windows.get(&window) {
                                    Some(Win::Desktop(d)) => d.selected_paths(),
                                    _ => Vec::new(),
                                };
                                if !paths.is_empty() {
                                    self.start_job(cx, ops::Kind::Trash, paths, None, Some(window));
                                }
                            }
                            "info" => {
                                let sel: Vec<Entry> = match self.windows.get(&window) {
                                    Some(Win::Desktop(d)) => d.selected_entries(),
                                    _ => Vec::new(),
                                };
                                for e in sel {
                                    self.open_info(cx, e);
                                }
                            }
                            _ => {}
                        },
                    }
                }
            }
            Some(Win::Info(_)) => self.info_ui(window, ui),
            Some(Win::Find(_)) => self.find_ui(cx, window, ui),
            Some(Win::Dialog(_)) => self.dialog_ui(cx, window, ui),
            Some(Win::Status) => self.status_ui(cx, window, ui),
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        match self.windows.get(&window)? {
            Win::Folder(f) => Some(self.folder_menus(f)),
            _ => None,
        }
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        self.folder_command(cx, window, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if matches!(self.windows.get(&window), Some(Win::Desktop(_))) {
            return;
        }
        match self.windows.remove(&window) {
            Some(Win::Find(fw)) => fw.cancel.store(true, Ordering::Relaxed),
            Some(Win::Status) => self.status_win = None,
            _ => {}
        }
        cx.close_window(window);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Remote(args) => self.handle_args(cx, &args),
            Msg::DirSize(win, b, f) => {
                if let Some(Win::Info(i)) = self.windows.get_mut(&win) {
                    i.size = Some((b, f));
                    cx.redraw(win);
                }
            }
            Msg::JobSlow(id) => {
                if self.jobs.iter().any(|(j, _)| j.id == id) && self.status_win.is_none() {
                    let id = cx.open_window(
                        WindowOptions::new("Tracker Status", STATUS_WIDTH, 82).min_size(300, 60),
                    );
                    self.windows.insert(id, Win::Status);
                    self.status_win = Some(id);
                    self.status_size = 0;
                }
            }
            Msg::JobDone(id, outcome) => self.job_done(cx, id, outcome),
            Msg::Listed { win, path, generation, result } => {
                if let Some(Win::Folder(f)) = self.windows.get_mut(&win) {
                    f.listed(&path, generation, result);
                    cx.redraw(win);
                }
            }
            Msg::Found(win, generation_id, entries, done) => {
                if let Some(Win::Find(fw)) = self.windows.get_mut(&win)
                    && fw.generation == generation_id
                {
                    fw.results.extend(entries);
                    if done {
                        fw.running = false;
                        fw.took = fw.started.elapsed();
                    }
                    cx.redraw(win);
                }
            }
        }
    }
}

impl Tracker {
    /// Run a file operation in the background. Short ones finish without a
    /// trace; longer ones show in the Tracker Status window.
    fn start_job(&mut self, cx: &mut Cx, kind: ops::Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, origin: Option<WindowId>) {
        let id = self.next_job;
        self.next_job += 1;
        let proxy = cx.proxy();
        let job = ops::start(id, kind, sources, dest, move |outcome| proxy.send(Msg::JobDone(id, outcome)));
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            std::thread::sleep(STATUS_DELAY);
            proxy.send(Msg::JobSlow(id));
        });
        self.jobs.push((job, origin));
        if let Some(w) = self.status_win {
            cx.redraw(w);
        }
    }

    fn job_done(&mut self, cx: &mut Cx, id: u64, outcome: ops::Outcome) {
        let Some(i) = self.jobs.iter().position(|(j, _)| j.id == id) else { return };
        let (job, origin) = self.jobs.remove(i);
        self.reload_all();
        // New items come up selected, as after a drop.
        if let Some(w) = origin
            && let Some(Win::Folder(f)) = self.windows.get_mut(&w)
            && !outcome.created.is_empty()
        {
            f.select_paths(&outcome.created);
            cx.redraw(w);
        }
        if let Some(e) = outcome.error {
            let what = job.title.chars().next().map(|c| c.to_lowercase().chain(job.title.chars().skip(1)).collect::<String>());
            self.error(cx, format!("An error occurred while {}: {e}", what.unwrap_or_default()));
        }
        if self.jobs.is_empty() {
            if let Some(w) = self.status_win.take() {
                self.windows.remove(&w);
                cx.close_window(w);
            }
        } else if let Some(w) = self.status_win {
            cx.redraw(w);
        }
    }

    fn status_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let mut stop = None;
        let top = ui.max_rect().top();
        let content = widgets::window_content(ui, |ui| {
            for (k, (job, _)) in self.jobs.iter().enumerate() {
                if k > 0 {
                    widgets::separator(ui);
                }
                let amount = ops::amount(job);
                let detail = ops::detail(job);
                ui.horizontal_top(|ui| {
                        let w = ui.available_width() - 64.0 - m::SPACING;
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            ui.add(widgets::StatusBar::new(job.progress.fraction()).label(&job.title).trailing(&amount));
                            ui.add(widgets::StringView::new(&detail).dim(true));
                        });
                        ui.add_space(m::SPACING);
                        ui.vertical(|ui| {
                            ui.add_space(m::LABEL_ROW + m::TIGHT - (m::CONTROL_HEIGHT - 14.0) / 2.0);
                            if ui.add(Button::new("Stop").min_width(64.0)).clicked() {
                                stop = Some(job.id);
                            }
                        });
                });
            }
            ui.min_rect().bottom()
        });
        // Fit the window to its rows.
        let h = (content - top + m::WINDOW_INSET).ceil() as u32;
        if self.status_size != h {
            self.status_size = h;
            cx.resize(win, (STATUS_WIDTH, h));
        }
        if let Some(id) = stop
            && let Some((job, _)) = self.jobs.iter().find(|(j, _)| j.id == id)
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
        // Progress lives in the workers' counters; poll while they run.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    fn folder_command_global(&mut self, cx: &mut Cx, cmd: &str) {
        if cmd == "empty-trash" {
            self.dialog(
                cx,
                "Empty Trash",
                DialogKind::EmptyTrash,
                "Are you sure you want to permanently remove all items from the Trash?".into(),
            );
        }
    }
}

// ---- single instance ------------------------------------------------------

fn socket_path() -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    rt.join(format!("tabula-tracker-{display}.sock"))
}

/// If Tracker is already running, hand it our arguments and return true.
fn forward_to_running(args: &[String]) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    let line = serde_json::to_string(args).unwrap_or_default();
    s.write_all(line.as_bytes()).is_ok() && s.write_all(b"\n").is_ok()
}

fn listen(proxy: tabula_kit::Proxy) {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(s).read_line(&mut line).is_ok()
                && let Ok(args) = serde_json::from_str::<Vec<String>>(&line)
            {
                proxy.send(Msg::Remote(args));
            }
        }
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if forward_to_running(&args) {
        return;
    }
    tabula_kit::run("tabula.tracker", |cx| {
        listen(cx.proxy());
        let mut t = Tracker { windows: HashMap::new(), clipboard: None, hidden: false, jobs: Vec::new(), next_job: 1, status_win: None, status_size: 0 };
        if args.iter().any(|a| a == "--desktop") {
            let id = cx.open_window(desktop::options());
            t.windows.insert(id, Win::Desktop(desktop::Desktop::new()));
        }
        t.handle_args(cx, &args);
        t
    });
}
