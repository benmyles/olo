//! Tracker: the desktop (icons for volumes, Home, Trash and ~/Desktop) and
//! spatial folder windows, one per folder, as on BeOS.

mod columns;
mod desktop;
mod find;
mod fs;
mod ops;
mod quicklook;
mod view;
mod watch;

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key},
    metrics as m,
    widgets::{self, Button, Column, ColumnListView, ListState, TextControl},
};
use olo_protocol::{Client, Reply, Request};

pub use fs::Entry;

/// How a window shows its folder; new windows follow Preferences ▸ Tracker.
pub use olo_protocol::FolderView as ViewMode;

/// A column to the right of the root in column view.
pub struct Pane {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub list: ListState,
    /// Changed on disk while being listed: list again when it lands.
    dirty: bool,
    /// The listing request whose result we are waiting for (see `Loader`).
    pub generation: u64,
    pub loading: bool,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Pane { path, entries: Vec::new(), list: ListState::default(), dirty: false, generation: 0, loading: true }
    }
}

/// Folders are listed on worker threads (listing stats every item and can
/// take a while on big or slow folders); results come back as
/// `Msg::Listed` to the window that asked.
#[derive(Clone)]
pub struct Loader {
    win: WindowId,
    proxy: olo_kit::Proxy,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Changed on disk while being listed: list again when it lands.
    dirty: bool,
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
    /// What the scroll last brought into view (active column, columns,
    /// preview): scrolling follows the keyboard only when that changes.
    pub col_focus: Option<(usize, usize, bool)>,
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

impl FindWin {
    /// Move the selection by `d` results (Quick Look's arrow keys).
    fn step_selection(&mut self, d: isize) -> Option<Entry> {
        let n = self.results.len();
        if n == 0 {
            return None;
        }
        let k = self.list.single().map_or(0, |k| (k as isize + d).clamp(0, n as isize - 1) as usize);
        self.list.select(k);
        self.results.get(k).map(|h| h.entry.clone())
    }
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
    /// The Quick Look window (its state is `Tracker::quicklook`).
    Preview,
}

enum Msg {
    /// Another `olo-tracker` invocation forwarded its arguments.
    DirSize(WindowId, u64, u64),
    Found(WindowId, u64, Vec<find::Hit>, bool),
    JobDone(u64, ops::Outcome),
    Listed { win: WindowId, path: PathBuf, generation: u64, result: Result<Vec<Entry>, String> },
    /// A job has been running a while: time to show the status window.
    JobSlow(u64),
    /// Files changed in folders being shown.
    Changed(Vec<watch::Change>),
    /// Quick Look loaded (part of) the item of a generation.
    Preview(u64, quicklook::Update),
    /// Quick Look of a generation is taking a while: open the window.
    PreviewSlow(u64),
    /// Quick Look rotated (or saved) the picture at a path.
    Edited(PathBuf, quicklook::EditEvent),
    /// Preferences ▸ Tracker changed (or was first read).
    Settings(olo_protocol::TrackerSettings),
}

pub struct Tracker {
    windows: HashMap<WindowId, Win>,
    /// Paths cut or copied with ⌘X / ⌘C, and whether it was a cut.
    clipboard: Option<(Vec<PathBuf>, bool)>,
    hidden: bool,
    /// How new folder windows show their folder (Preferences ▸ Tracker).
    default_view: ViewMode,
    /// Running file operations, each with the window that started it.
    jobs: Vec<(ops::Job, Option<WindowId>)>,
    next_job: u64,
    status_win: Option<WindowId>,
    /// Long jobs say when they're done; Show opens where the items went.
    notes: olo_kit::notify::Notifier,
    note_dirs: HashMap<olo_kit::notify::NoteId, PathBuf>,
    /// Content height the status window was last sized to.
    status_size: u32,
    /// Live updates for every folder shown (None if inotify is missing).
    watcher: Option<watch::Watcher>,
    quicklook: Option<quicklook::QuickLook>,
    preview_generation: u64,
    /// Files dropped with the secondary button, waiting for Move Here /
    /// Copy Here / Create Link Here from the menu shown where they fell.
    asked_drop: Option<AskedDrop>,
}

struct AskedDrop {
    window: WindowId,
    paths: Vec<PathBuf>,
    dest: PathBuf,
    pos: egui::Pos2,
    shown: bool,
}

fn ask_drop_id() -> egui::Id {
    egui::Id::unique("tracker-drop-ask")
}

const STATUS_WIDTH: u32 = 420;
/// Jobs that finish sooner than this never show the status window.
const STATUS_DELAY: Duration = Duration::from_millis(400);
/// Jobs that ran this long notify when they finish.
const NOTIFY_AFTER: Duration = Duration::from_secs(10);

/// [`NOTIFY_AFTER`]; debug builds take `OLO_TRACKER_NOTIFY_AFTER_MS`
/// (tests have no ten-second copies).
fn notify_after() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("OLO_TRACKER_NOTIFY_AFTER_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    NOTIFY_AFTER
}

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
    fn new(path: PathBuf, view: ViewMode, hidden: bool, loader: Loader) -> Self {
        let mut w = FolderWin {
            path,
            entries: Vec::new(),
            view,
            list: ListState { multiple: true, ..Default::default() },
            sort: (SortKey::Name, true),
            dirty: false,
            error: None,
            band: None,
            typed: (String::new(), Instant::now()),
            editing: None,
            pending_edit: None,
            panes: Vec::new(),
            active: 0,
            col_scroll: 0.0,
            col_focus: None,
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
                self.dirty = false;
                self.path.clone()
            }
            Some(k) => {
                let p = &mut self.panes[k];
                p.generation = generation;
                p.loading = true;
                p.dirty = false;
                p.path.clone()
            }
        };
        let (hidden, counter, Loader { win, proxy }) = (self.hidden, self.listed.clone(), self.loader.clone());
        let counter = if pane.is_none() { counter } else { Arc::default() };
        std::thread::spawn(move || {
            let result = fs::list_counting(&path, hidden, &counter).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => "This folder was moved or deleted.".to_owned(),
                std::io::ErrorKind::PermissionDenied => "You don't have permission to see inside this folder.".to_owned(),
                _ => e.to_string(),
            });
            proxy.send(Msg::Listed { win, path, generation, result });
        });
    }

    /// A listing came back; ignore it if a newer one was asked for since.
    fn listed(&mut self, path: &Path, generation: u64, result: Result<Vec<Entry>, String>) {
        let sort = self.sort;
        if path == self.path && generation == self.root_generation {
            self.loading = false;
            if self.dirty {
                // Something changed after the listing began: what it found
                // may already be stale, so look again (keeping what is
                // shown meanwhile).
                self.request(None);
            }
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
                    let again = p.dirty;
                    replace_entries(&mut p.entries, &mut p.list, v, sort, &mut Vec::new());
                    if again {
                        self.request(Some(k));
                    }
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

    /// Move the selection by `d` items (Quick Look's arrow keys); the
    /// newly selected item.
    pub fn step_selection(&mut self, d: isize) -> Option<Entry> {
        let (entries, list) = self.current_mut();
        let n = entries.len();
        if n == 0 {
            return None;
        }
        let k = list.selected.iter().next().copied().map_or(0, |k| (k as isize + d).clamp(0, n as isize - 1) as usize);
        list.select(k);
        entries.get(k).cloned()
    }

    /// The folders shown, for the watcher.
    pub fn watched(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.path.as_path()).chain(self.panes.iter().map(|p| p.path.as_path()))
    }

    /// Splice changes seen on disk into the root and columns: items that
    /// appeared, changed or went away, keeping the selection and sort.
    pub fn apply(&mut self, changes: &[watch::Change]) {
        let (sort, hidden) = (self.sort, self.hidden);
        let mut rescan = Vec::new();
        for pane in std::iter::once(None).chain((0..self.panes.len()).map(Some)) {
            let dir = match pane {
                None => self.path.clone(),
                Some(k) => self.panes[k].path.clone(),
            };
            let mine: Vec<&watch::Change> = changes
                .iter()
                .filter(|c| matches!(c, watch::Change::Item { dir: d, .. } | watch::Change::Rescan(d) if *d == dir))
                .collect();
            if mine.is_empty() {
                continue;
            }
            let (entries, list, loading, dirty) = match pane {
                None => (&mut self.entries, &mut self.list, self.loading, &mut self.dirty),
                Some(k) => {
                    let p = &mut self.panes[k];
                    (&mut p.entries, &mut p.list, p.loading, &mut p.dirty)
                }
            };
            if loading {
                *dirty = true;
            }
            if mine.iter().any(|c| matches!(c, watch::Change::Rescan(_))) {
                rescan.push(pane);
                continue;
            }
            let mut v = entries.clone();
            for c in mine {
                let watch::Change::Item { path, entry, .. } = c else { continue };
                v.retain(|e| e.path != *path);
                if let Some(e) = entry
                    && (hidden || !e.name.starts_with('.'))
                {
                    v.push(e.clone());
                }
            }
            replace_entries(entries, list, v, sort, &mut Vec::new());
        }
        // Deepest first, so dropping a gone column doesn't shift the rest.
        for pane in rescan.into_iter().rev() {
            match pane {
                Some(k) if !self.panes[k].path.is_dir() => {
                    self.panes.truncate(k);
                    self.active = self.active.min(self.panes.len());
                }
                _ => self.request(pane),
            }
        }
        if !self.pending_select.is_empty() {
            let pending = std::mem::take(&mut self.pending_select);
            self.select_paths(&pending);
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

/// What an upgraded Tracker needs to reopen its windows as they were (see
/// `App::save_state`). The desktop comes back from `--desktop`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    hidden: bool,
    folders: Vec<SavedFolder>,
    find: usize,
    #[serde(default)]
    desktop_selected: Vec<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedFolder {
    path: PathBuf,
    size: (u32, u32),
    view: ViewMode,
    sort: (SortKey, bool),
    selected: Vec<PathBuf>,
}

fn folder_options(path: &Path) -> WindowOptions {
    WindowOptions::new(fs::display_name(path), 560, 380).min_size(240, 160)
}

/// Bring an existing Tracker window for `title` forward via app_server.
fn activate_existing(title: &str) -> bool {
    let Ok(mut c) = Client::connect() else { return false };
    if let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows)
        && let Some(w) = windows.iter().find(|w| w.app_id == "olo.tracker" && w.title == title) {
            let _ = c.request(&Request::Activate { id: w.id });
            return true;
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
        self.windows.insert(id, Win::Folder(FolderWin::new(path, self.default_view, self.hidden, loader)));
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
        let id = cx.open_window(WindowOptions::alert(title, &message));
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

    fn restore(&mut self, cx: &mut Cx, saved: Saved) {
        self.hidden = saved.hidden;
        for w in self.windows.values_mut() {
            if let Win::Desktop(d) = w {
                d.select_paths(&saved.desktop_selected);
            }
        }
        for f in saved.folders {
            if !f.path.is_dir() {
                continue;
            }
            let mut options = folder_options(&f.path);
            options.size = f.size;
            let id = cx.open_window(options);
            let loader = Loader { win: id, proxy: cx.proxy() };
            let mut w = FolderWin::new(f.path, f.view, self.hidden, loader);
            w.sort = f.sort;
            w.select_paths(&f.selected);
            self.windows.insert(id, Win::Folder(w));
        }
        for _ in 0..saved.find {
            self.open_find(cx);
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
                // Another window: home, as when launched bare.
                "--new-window" => {}
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
            "quick-look" => self.toggle_quick_look(cx, win),
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
            "terminal" => fs::open_in_terminal(&dir),
            "new-window" => {
                if let Some(app) = f.selected_entries().first().and_then(fs::running_app) {
                    fs::new_window(app);
                }
            }
            "group-windows" => fs::toggle_grouping(f.selected_entries().first()),
            "terminal-selected" => {
                for e in f.selected_entries().iter().filter(|e| e.is_dir) {
                    fs::open_in_terminal(&e.path);
                }
            }
            "info" => {
                let entries = f.selected_entries();
                if entries.is_empty()
                    && let Some(e) = fs::entry(&dir) {
                        self.open_info(cx, e);
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
            "restore" => {
                let top = fs::trash_files();
                let items: Vec<PathBuf> = selected.into_iter().filter(|p| p.parent() == Some(top.as_path())).collect();
                if !items.is_empty() {
                    self.start_job(cx, ops::Kind::Restore, items, None, Some(win));
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
            .item("Open in Terminal", "terminal", None)
            .item("Quick Look", "quick-look", Some(Key::Y))
            .enabled(has_sel)
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
            file = file
                .item("Restore", "restore", None)
                .enabled(has_sel && f.current_dir() == fs::trash_files())
                .item("Empty Trash", "empty-trash", None)
                .enabled(!fs::trash_is_empty());
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
        let Some(Win::Folder(f)) = self.windows.get_mut(&win) else { return };

        // Space is Quick Look, unless it continues a type-ahead name or
        // goes into a name being edited.
        let typing = !f.typed.0.is_empty() && f.typed.1.elapsed() < Duration::from_millis(900);
        if f.editing.is_none() && !typing && take_space(ui) {
            self.toggle_quick_look(cx, win);
        }
        let Some(Win::Folder(f)) = self.windows.get_mut(&win) else { return };
        let actions = view::folder_view(ui, f);
        self.follow_selection(cx, win);
        if !actions.is_empty() {
            // Actions change state the next frame shows (an edit starting).
            ui.ctx().request_repaint();
        }
        for a in actions {
            match a {
                view::Action::Open(e) => self.open_entry(cx, &e),
                view::Action::Drag(d) => cx.start_drag(win, d.data(ui.ctx())),
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
        if let Ok(new) = &r {
            // A desktop icon keeps its place under its new name.
            for w in self.windows.values_mut() {
                if let Win::Desktop(d) = w {
                    d.renamed(p, new);
                }
            }
        }
        self.reload_all();
        match r {
            Ok(new) => Some(new),
            Err(e) => {
                self.error(cx, format!("Couldn't rename “{current}”: {e}"));
                None
            }
        }
    }

    fn info_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Info(info)) = self.windows.get(&win) else { return };
        if let Some(d) = view::info_view(ui, &info.entry, info.size) {
            cx.start_drag(win, d.data(ui.ctx()));
        }
    }

    fn find_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Find(fw)) = self.windows.get(&win) else { return };
        // Space is Quick Look on the selected result, as in a folder
        // window, unless it goes into the Find field.
        if !fw.list.selected.is_empty() && !ui.ctx().text_edit_focused() && take_space(ui) {
            self.toggle_quick_look(cx, win);
        }
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
        self.follow_selection(cx, win);
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


impl Tracker {
    /// Watch exactly the folders on screen: every folder window's folder
    /// and columns, and the desktop's.
    fn sync_watches(&mut self) {
        let Some(w) = &mut self.watcher else { return };
        let mut want = BTreeSet::new();
        for win in self.windows.values() {
            match win {
                Win::Folder(f) => want.extend(f.watched().map(Path::to_owned)),
                Win::Desktop(_) => want.extend(desktop::watched()),
                _ => {}
            }
        }
        w.set(want);
    }
}

/// Take a Space key press out of the frame's input (so type-ahead and
/// the views don't see it). Returns whether there was one.
fn take_space(ui: &egui::Ui) -> bool {
    ui.input_mut(|i| {
        if i.modifiers.any() && !i.modifiers.shift_only() {
            return false;
        }
        let hit = i.consume_key(egui::Modifiers::NONE, Key::Space);
        if hit {
            i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == " "));
        }
        hit
    })
}

impl Tracker {
    /// The single selected item of a folder window or the desktop.
    fn selected_one(&self, win: WindowId) -> Option<Entry> {
        let sel = match self.windows.get(&win)? {
            Win::Folder(f) => f.selected_entries(),
            Win::Desktop(d) => d.selected_entries(),
            Win::Find(fw) => return fw.list.single().and_then(|i| fw.results.get(i)).map(|h| h.entry.clone()),
            _ => return None,
        };
        sel.into_iter().next()
    }

    /// Space / ⌘Y: show the selection in Quick Look, or close it.
    fn toggle_quick_look(&mut self, cx: &mut Cx, source: WindowId) {
        if let Some(q) = &self.quicklook
            && q.source == source
        {
            return self.close_quick_look(cx);
        }
        let Some(entry) = self.selected_one(source) else { return };
        self.show_quick_look(cx, source, entry);
    }

    fn show_quick_look(&mut self, cx: &mut Cx, source: WindowId, entry: Entry) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let proxy = cx.proxy();
        let send = move |g, u| proxy.send(Msg::Preview(g, u));
        match &mut self.quicklook {
            Some(q) => {
                q.source = source;
                if let Some(id) = q.win {
                    cx.set_title(id, entry.name.clone());
                }
                q.retarget(entry, generation, send);
            }
            None => {
                self.quicklook = Some(quicklook::QuickLook::new(source, entry, generation, send));
                // Open the window when the preview is ready, or after a
                // moment with a spinner if it is slow.
                let proxy = cx.proxy();
                std::thread::spawn(move || {
                    std::thread::sleep(quicklook::SLOW);
                    proxy.send(Msg::PreviewSlow(generation));
                });
            }
        }
    }

    fn close_quick_look(&mut self, cx: &mut Cx) {
        if let Some(mut q) = self.quicklook.take() {
            q.stop();
            if let Some(id) = q.win {
                self.windows.remove(&id);
                cx.close_window(id);
            }
            cx.activate(q.source);
        }
    }

    /// While Quick Look shows a window's selection, follow it as it moves.
    fn follow_selection(&mut self, cx: &mut Cx, win: WindowId) {
        let Some(q) = &self.quicklook else { return };
        if q.source != win {
            return;
        }
        match self.selected_one(win) {
            Some(e) if e.path != q.entry.path => self.show_quick_look(cx, win, e),
            _ => {}
        }
    }

    fn preview_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(q) = &mut self.quicklook else { return };
        let source = q.source;
        let action = q.ui(ui);
        if q.playing_video() != q.awake {
            q.awake = !q.awake;
            cx.keep_awake(win, q.awake);
        }
        match action {
            Some(quicklook::Action::Close) => self.close_quick_look(cx),
            Some(quicklook::Action::Open) => {
                let e = q.entry.clone();
                self.close_quick_look(cx);
                self.open_entry(cx, &e);
            }
            Some(quicklook::Action::PopOut) => {
                let e = q.entry.clone();
                self.close_quick_look(cx);
                fs::open_in_viewer(&e.path);
            }
            Some(quicklook::Action::Fullscreen) => cx.set_fullscreen(win, !cx.is_fullscreen(win)),
            Some(quicklook::Action::Drag(d)) => cx.start_drag(win, d.data(ui.ctx())),
            Some(quicklook::Action::Rotate(right)) => {
                let (proxy, path) = (cx.proxy(), q.entry.path.clone());
                q.rotate(right, move |ev| proxy.send(Msg::Edited(path.clone(), ev)));
                cx.redraw(win);
            }
            Some(quicklook::Action::Step(d)) => {
                let next = match self.windows.get_mut(&source) {
                    Some(Win::Folder(f)) => f.step_selection(d),
                    Some(Win::Find(fw)) => fw.step_selection(d),
                    _ => None,
                };
                if let Some(e) = next {
                    cx.redraw(source);
                    self.show_quick_look(cx, source, e);
                }
            }
            None => {}
        }
    }
}

impl Tracker {
    /// One window's contents, by kind.
    fn window_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get(&window) {
            Some(Win::Folder(_)) => self.folder_ui(cx, window, ui),
            Some(Win::Desktop(_)) => {
                let Some(Win::Desktop(d)) = self.windows.get_mut(&window) else { return };
                if !d.is_editing() && take_space(ui) {
                    self.toggle_quick_look(cx, window);
                }
                let Some(Win::Desktop(d)) = self.windows.get_mut(&window) else { return };
                let actions = d.ui(ui);
                self.follow_selection(cx, window);
                let Some(Win::Desktop(d)) = self.windows.get_mut(&window) else { return };
                let _ = d;
                if !actions.is_empty() {
                    ui.ctx().request_repaint();
                }
                for a in actions {
                    match a {
                        view::Action::Open(e) => self.open_entry(cx, &e),
                        view::Action::Drag(d) => cx.start_drag(window, d.data(ui.ctx())),
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
                            "terminal" => fs::open_in_terminal(&fs::home().join("Desktop")),
                            "backgrounds" => {
                                let _ = olo_kit::spawn_detached("olo-preferences backgrounds");
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
            Some(Win::Info(_)) => self.info_ui(cx, window, ui),
            Some(Win::Find(_)) => self.find_ui(cx, window, ui),
            Some(Win::Dialog(_)) => self.dialog_ui(cx, window, ui),
            Some(Win::Status) => self.status_ui(cx, window, ui),
            Some(Win::Preview) => self.preview_ui(cx, window, ui),
            None => {}
        }
    }
}

impl App for Tracker {
    fn save_state(&mut self, cx: &Cx) -> Option<String> {
        let mut ids: Vec<&WindowId> = self.windows.keys().collect();
        ids.sort(); // opening order, so they stack as before
        let mut saved = Saved { hidden: self.hidden, folders: Vec::new(), find: 0, desktop_selected: Vec::new() };
        for id in ids {
            match &self.windows[id] {
                Win::Folder(f) => saved.folders.push(SavedFolder {
                    path: f.path.clone(),
                    size: cx.window_size(*id).unwrap_or_else(|| folder_options(&f.path).size),
                    view: f.view,
                    sort: f.sort,
                    selected: f.selected_paths(),
                }),
                Win::Find(_) => saved.find += 1,
                Win::Desktop(d) => saved.desktop_selected = d.selected_paths(),
                _ => {}
            }
        }
        serde_json::to_string(&saved).ok()
    }

    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        self.sync_watches();
        self.asked_drop_ui(cx, window, ui);
        self.window_ui(cx, window, ui);
        // A drag rested on a folder: open it, so the drag can go on in.
        if let Some(dir) = view::take_spring(ui.ctx()) {
            self.open_folder(cx, &dir);
        }
    }

    fn dropped(&mut self, cx: &mut Cx, window: WindowId, drop: olo_kit::dnd::Drop) {
        self.drop_on(cx, window, drop);
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
            Some(Win::Preview) => {
                if let Some(mut q) = self.quicklook.take() {
                    q.stop();
                }
            }
            Some(Win::Find(fw)) => fw.cancel.store(true, Ordering::Relaxed),
            Some(Win::Status) => self.status_win = None,
            _ => {}
        }
        cx.close_window(window);
        self.sync_watches();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<olo_kit::instance::Launch>() {
            Ok(launch) => return self.handle_args(cx, &launch.args),
            Err(m) => m,
        };
        let message = match message.downcast::<olo_kit::notify::NoteEvent>() {
            Ok(e) => {
                match *e {
                    olo_kit::notify::NoteEvent::Action { id, token, .. } => {
                        if let Some(dir) = self.note_dirs.remove(&id) {
                            let path = dir.canonicalize().unwrap_or(dir);
                            match self.windows.iter().find(|(_, w)| matches!(w, Win::Folder(f) if f.path == path)).map(|(id, _)| *id) {
                                Some(win) => cx.activate_with(win, token),
                                None => self.open_folder(cx, &path),
                            }
                        }
                    }
                    olo_kit::notify::NoteEvent::Closed { id } => {
                        self.note_dirs.remove(&id);
                    }
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Settings(s) => self.default_view = s.default_view,
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
            Msg::Changed(changes) => {
                for (&id, w) in self.windows.iter_mut() {
                    let touched = |dirs: &mut dyn Iterator<Item = &Path>| {
                        let dirs: Vec<&Path> = dirs.collect();
                        changes.iter().any(|c| match c {
                            watch::Change::Item { dir, .. } | watch::Change::Rescan(dir) => dirs.contains(&dir.as_path()),
                            watch::Change::Mounts => false,
                        })
                    };
                    match w {
                        Win::Folder(f) if touched(&mut f.watched()) => {
                            f.apply(&changes);
                            cx.redraw(id);
                        }
                        Win::Desktop(d) => {
                            if d.apply(&changes) {
                                cx.redraw(id);
                            }
                        }
                        _ => {}
                    }
                }
                self.sync_watches();
            }
            Msg::Preview(generation, update) => {
                let screen = cx.screen_size();
                let Some(q) = self.quicklook.as_mut().filter(|q| q.generation() == generation) else { return };
                let fresh = q.update(update);
                let size = q.ideal_size(screen);
                match q.win {
                    None if fresh => {
                        let title = q.entry.name.clone();
                        let id = cx.open_window(WindowOptions::new(title, size.0, size.1).min_size(320, 200));
                        q.win = Some(id);
                        self.windows.insert(id, Win::Preview);
                    }
                    Some(id) => {
                        if fresh {
                            cx.resize(id, size);
                        }
                        cx.redraw(id);
                    }
                    None => {}
                }
            }
            Msg::Edited(path, event) => {
                let screen = cx.screen_size();
                let Some(q) = self.quicklook.as_mut() else { return };
                let shown = matches!(event, quicklook::EditEvent::Shown { .. });
                if q.edited(&path, event)
                    && let Some(id) = q.win
                {
                    if shown && !cx.is_fullscreen(id) {
                        cx.resize(id, q.ideal_size(screen));
                    }
                    cx.redraw(id);
                }
            }
            Msg::PreviewSlow(generation) => {
                let screen = cx.screen_size();
                if let Some(q) = self.quicklook.as_mut().filter(|q| q.generation() == generation && q.win.is_none()) {
                    let size = q.ideal_size(screen);
                    let id = cx.open_window(WindowOptions::new(q.entry.name.clone(), size.0, size.1).min_size(320, 200));
                    q.win = Some(id);
                    self.windows.insert(id, Win::Preview);
                }
            }
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
    /// Files or text dropped on a folder, the desktop or the Trash.
    fn drop_on(&mut self, cx: &mut Cx, window: WindowId, drop: olo_kit::dnd::Drop) {
        use olo_kit::dnd::Action as A;
        let Some(target) = view::DropTarget::parse(&drop.target) else { return };
        let paths = drop.paths();
        let dest = match target {
            view::DropTarget::Trash => {
                let trash = fs::trash_files();
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !p.starts_with(&trash)).collect();
                if !paths.is_empty() {
                    self.start_job(cx, ops::Kind::Trash, paths, None, Some(window));
                }
                return;
            }
            view::DropTarget::Place => {
                if let Some(Win::Desktop(d)) = self.windows.get_mut(&window) {
                    d.place_dragged(drop.pos);
                    cx.redraw(window);
                }
                return;
            }
            view::DropTarget::Folder(dest) => dest,
        };
        // An application dragged out of Deskbar: a link to it, named as
        // the application is.
        if drop.wants_link() && drop.mime == "text/uri-list" && !matches!(drop.action, A::Ask) {
            let mut names = Vec::new();
            for p in &paths {
                match fs::app_link_into(p, &dest) {
                    Ok(to) => names.extend(to.file_name().map(|n| n.to_string_lossy().into_owned())),
                    Err(e) => {
                        self.error(cx, format!("Couldn't create a link: {e}"));
                        break;
                    }
                }
            }
            if let Some(Win::Desktop(d)) = self.windows.get_mut(&window)
                && dest == fs::home().join("Desktop")
            {
                d.place_new(&names, drop.pos);
            }
            self.reload_all();
            return;
        }
        // Dropped on the desktop: they go where they were let go.
        if let Some(Win::Desktop(d)) = self.windows.get_mut(&window)
            && dest == fs::home().join("Desktop")
            && !matches!(drop.action, A::Ask)
        {
            let names: Vec<String> = paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            d.place_new(&names, drop.pos);
        }
        if drop.mime != "text/uri-list" || paths.is_empty() {
            // Text (or web links): a clipping file, as BeOS made.
            let text = drop.text();
            if !text.trim().is_empty() {
                match fs::clipping(&dest, &text) {
                    Ok(_) => self.reload_all(),
                    Err(e) => self.error(cx, format!("Couldn't save the clipping: {e}")),
                }
            }
            return;
        }
        // Nothing goes into itself.
        if let Some(p) = paths.iter().find(|p| p.is_dir() && dest.starts_with(p)) {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.error(cx, format!("“{name}” can't go inside itself."));
            return;
        }
        match drop.action {
            A::Ask => {
                self.asked_drop = Some(AskedDrop { window, paths, dest, pos: drop.pos, shown: false });
                cx.redraw(window);
            }
            A::Move => {
                // Already there: nothing to move.
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.parent() != Some(dest.as_path())).collect();
                if !paths.is_empty() {
                    self.start_job(cx, ops::Kind::Move, paths, Some(dest), Some(window));
                }
            }
            A::Copy => self.start_job(cx, ops::Kind::Copy, paths, Some(dest), Some(window)),
        }
    }

    /// The menu for a secondary-button drop, and what is chosen from it.
    fn asked_drop_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &egui::Ui) {
        let Some(a) = self.asked_drop.as_mut().filter(|a| a.window == window) else { return };
        if !a.shown {
            a.shown = true;
            let same = a.paths.iter().all(|p| p.parent() == Some(a.dest.as_path()));
            let menu = Menu::new("")
                .item("Move Here", "move", None)
                .enabled(!same)
                .item("Copy Here", "copy", None)
                .item("Create Link Here", "link", None)
                .separator()
                .item("Cancel", "cancel", None);
            widgets::popup_menu_at(ui.ctx(), ask_drop_id(), a.pos, menu);
            return;
        }
        let Some(cmd) = widgets::popup_result(ui.ctx(), ask_drop_id()) else { return };
        let Some(a) = self.asked_drop.take() else { return };
        match cmd.as_str() {
            "move" => self.start_job(cx, ops::Kind::Move, a.paths, Some(a.dest), Some(window)),
            "copy" => self.start_job(cx, ops::Kind::Copy, a.paths, Some(a.dest), Some(window)),
            "link" => {
                for p in &a.paths {
                    if let Err(e) = fs::link_into(p, &a.dest) {
                        self.error(cx, format!("Couldn't create a link: {e}"));
                        break;
                    }
                }
                self.reload_all();
            }
            _ => {}
        }
    }

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
        // A long job may have been left to run in the background: say it's
        // done.
        if outcome.error.is_none() && !outcome.cancelled && job.started.elapsed() >= notify_after() {
            let note = olo_kit::notify::Note::new(&job.finished);
            let note = if job.dest.is_some() { note.action("default", "Show") } else { note };
            let n = self.notes.show(None, note);
            if let Some(d) = job.dest.clone() {
                self.note_dirs.insert(n, d);
            }
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

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if olo_kit::instance::forward("olo.tracker", &args) {
        return;
    }
    olo_preview::audio::set_identity("Tracker", "olo-tracker");
    olo_kit::run("olo.tracker", |cx| {
        olo_kit::instance::listen("olo.tracker", cx.proxy());
        let proxy = cx.proxy();
        let watcher = watch::Watcher::new(move |changes| proxy.send(Msg::Changed(changes)));
        let proxy = cx.proxy();
        olo_kit::settings::watch(move |s| {
            if let Ok(mut u) = fs::UNGROUPED.lock() {
                *u = s.tiling.ungrouped_apps.clone();
            }
            proxy.send(Msg::Settings(s.tracker))
        });
        let mut t = Tracker {
            windows: HashMap::new(),
            clipboard: None,
            hidden: false,
            // Folders opened at startup can't wait for the watcher's first read.
            default_view: olo_kit::settings::current().map(|s| s.tracker.default_view).unwrap_or_default(),
            jobs: Vec::new(),
            next_job: 1,
            status_win: None,
            notes: olo_kit::notify::Notifier::new(cx.proxy(), "Tracker", "olo-tracker"),
            note_dirs: HashMap::new(),
            status_size: 0,
            watcher,
            quicklook: None,
            preview_generation: 0,
            asked_drop: None,
        };
        if args.iter().any(|a| a == "--desktop") {
            let id = cx.open_window(desktop::options());
            t.windows.insert(id, Win::Desktop(desktop::Desktop::new()));
        }
        match cx.restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) {
            Some(saved) => t.restore(cx, saved),
            None => t.handle_args(cx, &args),
        }
        t
    });
}
