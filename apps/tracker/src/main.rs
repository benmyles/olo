//! Tracker: the desktop (icons for volumes, Home, Trash and ~/Desktop) and
//! spatial folder windows, one per folder on each workspace, as on BeOS.
//! On olo-kit: every window is its own view on a looper of its own, and
//! does what it can itself (lists its folder, selects, drags, takes
//! drops, edits a name). The app's looper is a hub ([`Hub`]): the windows
//! open, the file operations running and their history (Undo), the
//! folders followed on disk, Quick Look. A window asks things of it
//! through the proxy ([`ToHub`]) and is told what happened by a message;
//! what the windows all read (Undo's names, the places, the jobs running,
//! what each window shows) is the app's, behind locks.

mod appimages;
mod backups;
mod columns;
mod desktop;
mod filemanager;
mod find;
mod folder;
mod fs;
mod goto;
mod items;
mod model;
mod ops;
mod panels;
mod quicklook;
mod sorts;
mod thumbs;
mod undo;
mod watch;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    ClipboardData, ClipboardEntry, ClipboardItem, DragAction, Dropped, Menu, WindowCx, WindowHandle, WindowOptions,
    instance::{self, Launch},
    menu,
    notify::{Note, NoteEvent, NoteId, Notifier},
    places,
    widgets::{open_alert, open_app_alert},
    workspaces,
};
use olo_sync::Mutex;

pub use fs::Entry;
/// How a window shows its folder; new windows follow Preferences ▸ Tracker.
pub use olo_protocol::FolderView as ViewMode;

use crate::{
    desktop::{Desktop, DesktopMsg},
    folder::{FolderMsg, FolderWin},
    model::SortKey,
    panels::{FindMsg, FindWin, InfoMsg, InfoWin, StatusWin},
    quicklook::{QuickLook, QuickMsg},
};

const APP_ID: &str = "olo.tracker";

/// What a window (or a worker of the app's) asks of the app, on its
/// looper.
pub enum ToHub {
    // ---- from workers ----
    JobDone(u64, ops::Outcome),
    /// A job has been running a while: time to show the status window.
    JobSlow(u64),
    /// Files changed in folders being shown.
    Changed(Vec<watch::Change>),
    /// Preferences ▸ Tracker changed (or was first read).
    Settings(olo_protocol::TrackerSettings),
    /// A picture is ready: a thumbnail, an AppImage's icon.
    Thumb,
    /// DiskBackup's service said which of a folder's files are backed up
    /// as they are now (None: it couldn't).
    Marks(PathBuf, Option<HashSet<String>>),
    /// DiskBackup went on or off, or finished a backup (on, its last).
    Backup(bool, Option<i64>),
    /// Something every window shows changed (the places): shown again.
    Refresh,
    // ---- from windows ----
    /// A window said what it shows now ([`publish`]).
    Told(u64),
    Closed(u64),
    /// A folder, in its window (brought forward, or opened); `from`: the
    /// folder of the window it was opened in, where its Back goes.
    Open {
        path: PathBuf,
        from: Option<PathBuf>,
    },
    /// Another window, always: on a folder, shown this way (None: as
    /// Preferences has it).
    NewWindow(PathBuf, Option<ViewMode>),
    /// Show a path: a folder opens (`.1`: its parent does, with it
    /// selected); anything else is selected in its folder's window.
    Reveal(PathBuf, bool),
    GetInfo(Vec<Entry>),
    Find,
    /// Go to File or Folder, asked in a window on this folder.
    GoTo(PathBuf),
    /// A name edited in place, taken (`win`: the folder window it was
    /// edited in, which selects it under its new name).
    Rename {
        win: Option<u64>,
        path: PathBuf,
        name: String,
    },
    /// A file operation, run in the background.
    Job {
        kind: ops::Kind,
        sources: Vec<PathBuf>,
        dest: Option<PathBuf>,
        origin: Option<u64>,
    },
    /// A step done there and then (a new folder, a paste, icons placed).
    Done(String, undo::Change),
    /// Edit ▸ Undo (false) or Redo, asked in a window.
    UndoRedo(bool, Option<u64>),
    /// Show Hidden Files, on or off (every window).
    Hidden,
    ReloadAll,
    /// Space / ⌘Y in a window: its selection in Quick Look, or Quick
    /// Look closed.
    QuickLook(u64),
    /// This item of a window's, in Quick Look (it stepped there).
    Look(u64, Entry),
    /// Quick Look's arrows: its window's selection a step on.
    LookStep(isize),
    LookClosed,
    StopJob(u64),
    /// A window to another display has its copy there: it goes here.
    Gone(u64),
    Quit,
}

pub fn to_hub(message: ToHub) {
    olo_kit::proxy().send(message);
}

/// What Paste takes from the clipboard, best first: files (as file
/// managers copy them), a picture, text.
pub const PASTEABLE: &[&str] =
    &[olo_kit_base::clipboard::FILES[0], olo_kit_base::clipboard::FILES[1], "image/png", "image/jpeg", "image/webp", "image/gif", "image/", "text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// Jobs that finish sooner than this never show the status window.
const STATUS_DELAY: Duration = Duration::from_millis(400);
/// Jobs that ran this long notify when they finish.
const NOTIFY_AFTER: Duration = Duration::from_secs(10);
/// A Find window's results are followed in at most this many folders.
pub const FIND_FOLLOWS: usize = 256;

/// [`NOTIFY_AFTER`]; debug builds take `OLO_TRACKER_NOTIFY_AFTER_MS`
/// (tests have no ten-second copies).
fn notify_after() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("OLO_TRACKER_NOTIFY_AFTER_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    NOTIFY_AFTER
}

/// Whether Paste would find something to take.
pub fn can_paste() -> bool {
    let types = olo_kit::clipboard::types();
    PASTEABLE.iter().any(|w| types.iter().any(|t| if w.ends_with('/') { t.starts_with(w) } else { t == w }))
}

// ---- what the windows and the app share -------------------------------------

/// What a window shows, as the app knows it (the window says, when it
/// changes: [`publish`]): its folders (watched), the one item selected
/// (Quick Look follows it), what an upgrade keeps of it.
#[derive(Clone)]
pub struct WinInfo {
    pub path: PathBuf,
    /// The window's surface (which workspace it's on).
    pub surface: Option<u32>,
    pub watched: Vec<PathBuf>,
    pub selected: Option<Entry>,
    pub folder: Option<SavedFolder>,
    pub find: bool,
}

impl WinInfo {
    /// Whether `other` says the same of everything the app acts on: all
    /// but the window's size.
    pub fn same_but_for_size(&self, other: &WinInfo) -> bool {
        let sized = |f: &Option<SavedFolder>, size| f.clone().map(|f| SavedFolder { size, ..f });
        let size = self.folder.as_ref().map_or((0, 0), |f| f.size);
        self.path == other.path
            && self.surface == other.surface
            && self.watched == other.watched
            && self.selected.as_ref().map(|e| &e.path) == other.selected.as_ref().map(|e| &e.path)
            && sized(&self.folder, size) == sized(&other.folder, size)
            && self.find == other.find
    }

    /// Whether `other` says the same.
    pub fn same(&self, other: &WinInfo) -> bool {
        self.path == other.path
            && self.surface == other.surface
            && self.watched == other.watched
            && self.selected.as_ref().map(|e| &e.path) == other.selected.as_ref().map(|e| &e.path)
            && self.folder == other.folder
            && self.find == other.find
    }
}

/// The standard folders, favourites and recent folders (the Go menu), as
/// last read: read again on a worker when a few seconds old, so a menu
/// never waits for the disk.
struct PlacesCache {
    places: Arc<places::Places>,
    read: Option<Instant>,
    reading: bool,
}

struct Shared {
    /// Hidden files are listed (every window's).
    hidden: AtomicBool,
    /// Edit ▸ Undo's and Redo's names, and whether there's a step for
    /// each.
    undo: Mutex<[(String, bool); 2]>,
    places: Mutex<PlacesCache>,
    /// The file operations running (Tracker Status shows them; an
    /// upgrade waits for them).
    jobs: Mutex<Vec<Arc<ops::Job>>>,
    infos: Mutex<HashMap<u64, WinInfo>>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        hidden: AtomicBool::new(false),
        undo: Mutex::new([("Undo".into(), false), ("Redo".into(), false)]),
        places: Mutex::new(PlacesCache { places: Arc::new(places::Places::default()), read: None, reading: false }),
        jobs: Mutex::new(Vec::new()),
        infos: Mutex::new(HashMap::new()),
    })
}

/// A window says what it shows now.
pub fn publish(id: u64, info: WinInfo) {
    shared().infos.lock().unwrap_or_else(|e| e.into_inner()).insert(id, info);
    to_hub(ToHub::Told(id));
}

/// [`publish`], of something the app only keeps (the window's size, for
/// an upgrade to bring back): kept, and the app's looper not woken for
/// it. A window resized live said its size sixty times a second, and
/// each had the app work out every window's watches again.
pub fn keep(id: u64, info: WinInfo) {
    shared().infos.lock().unwrap_or_else(|e| e.into_inner()).insert(id, info);
}

fn info_of(id: u64) -> Option<WinInfo> {
    shared().infos.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
}

/// Edit ▸ Undo's and Redo's names, and whether there's a step for each.
pub fn undo_names() -> [(String, bool); 2] {
    shared().undo.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The file operations running.
pub fn jobs() -> Vec<Arc<ops::Job>> {
    shared().jobs.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn places() -> Arc<places::Places> {
    let mut c = shared().places.lock().unwrap_or_else(|e| e.into_inner());
    let stale = c.read.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) && !c.reading;
    if stale {
        c.reading = true;
        std::thread::spawn(|| {
            let mut p = places::load();
            // Only folders that are there.
            p.favorites.retain(|d| d.is_dir());
            p.recent.retain(|d| d.is_dir());
            let mut c = shared().places.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *c.places != p;
            *c = PlacesCache { places: Arc::new(p), read: Some(Instant::now()), reading: false };
            drop(c);
            if changed {
                to_hub(ToHub::Refresh);
            }
        });
    }
    c.places.clone()
}

/// The places were changed here (a favourite added): read again.
pub fn places_changed() {
    shared().places.lock().unwrap_or_else(|e| e.into_inner()).read = None;
    to_hub(ToHub::Refresh);
}

/// Copies `clip` (each form's type and data) to the clipboard.
pub fn copy_clip(clip: olo_kit_base::clipboard::Clip) {
    let entries = clip.into_iter().map(|(mime, bytes)| ClipboardEntry::Data(ClipboardData { mime, bytes })).collect();
    olo_kit::clipboard::write(ClipboardItem::from_entries(entries));
}

/// Open With ▸ an app (`open-with:ID`; Always: and make it the kind's,
/// `always-open-with:ID`), or Other… (`open-with-other`: a window to
/// choose any app).
pub fn open_with(cmd: &str, entries: &[Entry]) {
    let Some(kind) = folder::open_with_kind(entries) else { return };
    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
    if cmd == olo_kit::open_with::OTHER {
        return olo_kit::open_with::other(kind, paths);
    }
    olo_kit::open_with::run(cmd, kind, paths);
}

/// Empty Trash, asked over the window it was chosen in.
pub fn confirm_empty_trash(cx: &mut WindowCx) {
    let _ = open_alert(cx, "Empty Trash", "trash-full", "Are you sure you want to permanently remove all items from the Trash?", &["Cancel", "Empty Trash"], |choice, _| {
        if *choice == Some(1) {
            to_hub(ToHub::Job { kind: ops::Kind::EmptyTrash, sources: Vec::new(), dest: None, origin: None });
        }
    });
}

/// An item's new state on disk (Some(None): gone) by its folder's
/// changes; None if they don't touch it.
pub fn now_on_disk(changes: &[watch::Change], path: &Path) -> Option<Option<Entry>> {
    changes.iter().find_map(|c| match c {
        watch::Change::Item { path: p, entry, .. } if p == path => Some(entry.clone()),
        watch::Change::Rescan(d) if path.parent() == Some(d.as_path()) => Some(fs::entry(path)),
        _ => None,
    })
}

/// What was dropped on folder `dest` (in window `origin`, whose `cx`
/// this is) goes into it: files moved or copied (a job), or asked about
/// (a menu where they fell); an application dragged out of Deskbar, a
/// link to it; a picture, a link or text, a file of it. The links made
/// (their names), or why it couldn't be done.
pub fn dropped_into(drop: &Dropped, paths: Vec<PathBuf>, dest: PathBuf, origin: Option<u64>, cx: &mut WindowCx) -> Result<Vec<String>, String> {
    let job = move |kind: ops::Kind, sources: Vec<PathBuf>, dest: PathBuf| to_hub(ToHub::Job { kind, sources, dest: Some(dest), origin });
    let wants_link = drop.types.iter().any(|t| t == olo_kit::dnd::LINK_TYPE);
    // An application dragged out of Deskbar: a link to it, named as
    // the application is.
    if wants_link && drop.mime == "text/uri-list" && drop.action != DragAction::Ask {
        let mut names = Vec::new();
        let mut failed = None;
        for p in &paths {
            match fs::app_link_into(p, &dest) {
                Ok(to) => names.extend(to.file_name().map(|n| n.to_string_lossy().into_owned())),
                Err(e) => {
                    failed = Some(format!("Couldn't create a link: {e}"));
                    break;
                }
            }
        }
        to_hub(ToHub::ReloadAll);
        return match failed {
            Some(e) => Err(e),
            None => Ok(names),
        };
    }
    if drop.mime != "text/uri-list" || paths.is_empty() {
        // A file offered by its contents (a picture from a web page),
        // else text or a link: a clipping file, as BeOS made.
        let saved = if let Some(name) = olo_kit_base::files::named_file(&drop.mime) {
            Some(fs::save_new(&dest, &name, &drop.data))
        } else if let Some((url, title)) = (drop.mime == "text/x-moz-url").then(|| olo_kit_base::files::moz_url(&drop.data)).flatten() {
            Some(fs::link_clipping(&dest, &url, &title))
        } else {
            let text = String::from_utf8_lossy(&drop.data).into_owned();
            (!text.trim().is_empty()).then(|| fs::clipping(&dest, &text))
        };
        return match saved {
            Some(Ok(p)) => {
                to_hub(ToHub::Done("Drop".into(), undo::Change::Made(vec![p])));
                to_hub(ToHub::ReloadAll);
                Ok(Vec::new())
            }
            Some(Err(e)) => Err(format!("Couldn't save what was dropped: {e}")),
            None => Ok(Vec::new()),
        };
    }
    // Nothing goes into itself.
    if let Some(p) = paths.iter().find(|p| p.is_dir() && dest.starts_with(p)) {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        return Err(format!("“{name}” can't go inside itself."));
    }
    match drop.action {
        DragAction::Ask => {
            // The menu for a secondary-button drop, where it fell.
            let same = paths.iter().all(|p| p.parent() == Some(dest.as_path()));
            let menu = Menu::new("").item("Move Here", "move").enabled(!same).item("Copy Here", "copy").item("Create Link Here", "link").separator().item("Cancel", "cancel");
            menu::context_menu(cx, menu, drop.position, move |cmd, _| {
                let (paths, dest) = (paths.clone(), dest.clone());
                match cmd {
                    "move" => job(ops::Kind::Move, paths, dest),
                    "copy" => job(ops::Kind::Copy, paths, dest),
                    "link" => {
                        let mut made = Vec::new();
                        for p in &paths {
                            match fs::link_into(p, &dest) {
                                Ok(l) => made.push(l),
                                Err(e) => {
                                    let _ = open_app_alert("Tracker", "about", &format!("Couldn't create a link: {e}"), &["OK"], |_| {});
                                    break;
                                }
                            }
                        }
                        to_hub(ToHub::Done(if made.len() == 1 { "Create Link" } else { "Create Links" }.into(), undo::Change::Made(made)));
                        to_hub(ToHub::ReloadAll);
                    }
                    _ => {}
                }
            });
        }
        DragAction::Move => {
            // Already there: nothing to move.
            let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.parent() != Some(dest.as_path())).collect();
            if !paths.is_empty() {
                job(ops::Kind::Move, paths, dest);
            }
        }
        DragAction::Copy => job(ops::Kind::Copy, paths, dest),
    }
    Ok(Vec::new())
}

// ---- the app's looper ---------------------------------------------------------

enum Win {
    Folder(WindowHandle<FolderWin>),
    Desktop(WindowHandle<Desktop>),
    Info(WindowHandle<InfoWin>),
    Find(WindowHandle<FindWin>),
}

/// One of Tracker's windows: the app's number for it, its window, and
/// (a folder's) the folder it was opened on, until it says what it shows.
struct Shown {
    id: u64,
    win: Win,
    path: Option<PathBuf>,
}

impl Shown {
    fn is_open(&self) -> bool {
        match &self.win {
            Win::Folder(w) => w.is_open(),
            Win::Desktop(w) => w.is_open(),
            Win::Info(w) => w.is_open(),
            Win::Find(w) => w.is_open(),
        }
    }

    fn raise(&self) {
        match &self.win {
            Win::Folder(w) => olo_kit::raise(Some(w)),
            Win::Desktop(w) => olo_kit::raise(Some(w)),
            Win::Info(w) => olo_kit::raise(Some(w)),
            Win::Find(w) => olo_kit::raise(Some(w)),
        };
    }
}

/// Quick Look, open: the window whose selection it shows, the item, and
/// its own window.
struct Looking {
    source: u64,
    path: PathBuf,
    window: WindowHandle<QuickLook>,
}

struct Hub {
    /// In opening order.
    windows: Vec<Shown>,
    next: u64,
    /// How new folder windows show their folder (Preferences ▸ Tracker).
    default_view: ViewMode,
    /// Running file operations, each with the window that started it.
    jobs: Vec<(Arc<ops::Job>, Option<u64>)>,
    next_job: u64,
    /// Edit ▸ Undo and Redo, for every window; and the jobs running whose
    /// outcome becomes a step (what to call it, and why it runs).
    history: undo::History<undo::Change>,
    recording: HashMap<u64, (String, undo::Purpose)>,
    status: Option<WindowHandle<StatusWin>>,
    go_to: Option<WindowHandle<goto::GoToWin>>,
    /// Long jobs say when they're done; Show opens where the items went.
    notes: Notifier,
    note_dirs: HashMap<NoteId, PathBuf>,
    /// Live updates for every folder shown (None if inotify is missing).
    watcher: Option<watch::Watcher>,
    /// What the watcher was last given.
    watched: BTreeSet<PathBuf>,
    look: Option<Looking>,
    preview_generation: u64,
}

/// What an upgraded Tracker needs to reopen its windows as they were.
/// The desktop comes back from `--desktop`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    hidden: bool,
    folders: Vec<SavedFolder>,
    find: usize,
    #[serde(default)]
    desktop_selected: Vec<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct SavedFolder {
    pub path: PathBuf,
    pub size: (u32, u32),
    pub view: ViewMode,
    pub sort: (SortKey, bool),
    /// `sort` was chosen (else Olo's own order: as before it was kept).
    #[serde(default)]
    pub own_sort: bool,
    pub selected: Vec<PathBuf>,
}

/// What one window keeps for an upgrade (`View::save_state`).
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SavedWindow {
    Folder(SavedFolder),
    Find,
    /// The desktop: what's selected on it.
    Desktop(Vec<PathBuf>),
}

fn hidden() -> bool {
    shared().hidden.load(Ordering::Relaxed)
}

fn error(message: String) {
    let _ = open_app_alert("Tracker", "about", &message, &["OK"], |_| {});
}

impl Hub {
    fn shown(&self, id: u64) -> Option<&Shown> {
        self.windows.iter().find(|s| s.id == id)
    }

    fn folder(&self, id: u64) -> Option<&WindowHandle<FolderWin>> {
        match self.shown(id).map(|s| &s.win) {
            Some(Win::Folder(f)) => Some(f),
            _ => None,
        }
    }

    fn desktop(&self) -> Option<&WindowHandle<Desktop>> {
        self.windows.iter().find_map(|s| match &s.win {
            Win::Desktop(d) => Some(d),
            _ => None,
        })
    }

    fn take_id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// The desktop (its window), unless this Tracker has it already: as it
    /// starts with `--desktop`, or told so by the session's Tracker when
    /// another, started by D-Bus as the file manager (`--file-manager`),
    /// was running first.
    fn open_desktop(&mut self, selected: Vec<PathBuf>) {
        if self.desktop().is_some() {
            return;
        }
        olo_kit::stay_resident();
        let id = self.take_id();
        let window = olo_kit::open_window(desktop::options(), move |_| Desktop::new(id, selected));
        self.windows.push(Shown { id, win: Win::Desktop(window), path: None });
        self.sync_watches();
    }

    /// The folder's window, brought forward, if it has one on this
    /// workspace; else a new one here (each workspace has its own).
    fn open_folder(&mut self, path: &Path) -> u64 {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        // A recent folder now, for the Go menu and file panels (off the
        // looper).
        let p = path.clone();
        std::thread::spawn(move || places::visited(&p));
        if let Some(id) = self.folder_window(&path) {
            if let Some(s) = self.shown(id) {
                s.raise();
            }
            return id;
        }
        self.new_folder_window(path, self.default_view, (560, 380), |_| {})
    }

    fn new_folder_window(&mut self, path: PathBuf, view: ViewMode, (w, h): (u32, u32), setup: impl FnOnce(&mut FolderWin) + Send + 'static) -> u64 {
        let id = self.take_id();
        let (hidden, opened) = (hidden(), path.clone());
        let window = olo_kit::open_window(WindowOptions::new(fs::display_name(&path), w as f32, h as f32).min_size(240.0, 160.0), move |cx| {
            let mut f = FolderWin::new(id, opened, view, hidden, cx);
            // (Its size as it's opened: what it keeps for an upgrade
            // before it has drawn and found out.)
            f.size = (w, h);
            setup(&mut f);
            f
        });
        self.windows.push(Shown { id, win: Win::Folder(window), path: Some(path) });
        self.sync_watches();
        id
    }

    /// `path`'s window on this workspace, if there is one.
    fn folder_window(&mut self, path: &Path) -> Option<u64> {
        self.windows.retain(|s| s.is_open());
        self.windows
            .iter()
            .find(|s| {
                matches!(s.win, Win::Folder(_)) && {
                    let info = info_of(s.id);
                    info.as_ref().map(|i| i.path.as_path()).or(s.path.as_deref()) == Some(path) && info.and_then(|i| i.surface).and_then(workspaces::here_surface).unwrap_or(true)
                }
            })
            .map(|s| s.id)
    }

    /// Show `path`: a folder opens; anything else is selected in its
    /// folder's window.
    fn reveal(&mut self, path: &Path) {
        if path.is_dir() {
            self.open_folder(path);
        } else {
            self.show_in_parent(path);
        }
    }

    /// Open `path`'s folder with it selected.
    fn show_in_parent(&mut self, path: &Path) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
        let id = self.open_folder(&dir);
        if let Some(f) = self.folder(id) {
            f.messenger().send(FolderMsg::Select(vec![dir.join(name)]));
        }
    }

    fn open_info(&mut self, e: Entry) {
        let h = panels::info_height(panels::info_rows(&e)).ceil();
        let title = format!("{} info", e.name);
        let id = self.take_id();
        let window = olo_kit::open_window(WindowOptions::new(title, 360.0, h).min_size(300.0, h).floating(), move |cx| InfoWin::new(id, e, cx));
        self.windows.push(Shown { id, win: Win::Info(window), path: None });
    }

    fn open_find(&mut self) {
        let id = self.take_id();
        let window = olo_kit::open_window(WindowOptions::new("Find", 680.0, 420.0).min_size(440.0, 260.0), move |cx| FindWin::new(id, cx));
        self.windows.push(Shown { id, win: Win::Find(window), path: None });
    }

    /// Go to File or Folder, from the window whose folder is `base` (a
    /// relative path is from there): the one open brought forward, else
    /// a new one.
    fn open_go_to(&mut self, base: PathBuf) {
        if let Some(w) = self.go_to.as_ref().filter(|w| w.is_open()) {
            olo_kit::raise(Some(w));
            return;
        }
        // (A dialog: over the folder's window, never a tab in its group.)
        let options = WindowOptions::new("Go to File or Folder", goto::WIDTH, goto::HEIGHT).min_size(360.0, 200.0).dialog().child();
        self.go_to = Some(olo_kit::open_window(options, move |cx| goto::GoToWin::new(base, cx)));
    }

    fn reload_all(&mut self) {
        let hidden = hidden();
        for s in &self.windows {
            let _ = match &s.win {
                Win::Folder(f) => f.messenger().send(FolderMsg::Reload(hidden)),
                Win::Desktop(d) => d.messenger().send(DesktopMsg::Reload),
                _ => false,
            };
        }
    }

    /// The windows that show folders say what they show again (Undo's
    /// name, the places, marks, a thumbnail).
    fn refresh(&self) {
        for s in &self.windows {
            let _ = match &s.win {
                Win::Folder(f) => f.messenger().send(FolderMsg::Refresh),
                Win::Desktop(d) => d.messenger().send(DesktopMsg::Refresh),
                _ => false,
            };
        }
    }

    fn restore(&mut self, saved: Saved) {
        shared().hidden.store(saved.hidden, Ordering::Relaxed);
        if let Some(d) = self.desktop() {
            d.messenger().send(DesktopMsg::Select(saved.desktop_selected));
        }
        for f in saved.folders {
            if !f.path.is_dir() {
                continue;
            }
            let (sort, selected) = (f.own_sort.then_some(f.sort), f.selected);
            self.new_folder_window(f.path, f.view, f.size, move |w| {
                if sort.is_some() {
                    w.sort = sort;
                }
                w.select_paths(&selected);
            });
        }
        for _ in 0..saved.find {
            self.open_find();
        }
    }

    fn handle_args(&mut self, args: &[String]) {
        let mut opened = false;
        let mut left = args.iter();
        while let Some(a) = left.next() {
            match a.as_str() {
                "--find" => {
                    self.open_find();
                    opened = true;
                }
                // Any app's Open in Tracker (`olo_kit_base::tracker`).
                "--reveal" => {
                    if let Some(p) = left.next() {
                        self.reveal(Path::new(p));
                    }
                    opened = true;
                }
                // (A Tracker D-Bus started as the file manager, then
                // the session's: the desktop is this one's to show.)
                "--desktop" => self.open_desktop(Vec::new()),
                FILE_MANAGER_ARG => {}
                // Another window, always (not one brought forward): home.
                "--new-window" => {
                    self.new_folder_window(fs::home(), self.default_view, (560, 380), |_| {});
                    opened = true;
                }
                p => {
                    let path = PathBuf::from(p);
                    if path.is_dir() {
                        self.open_folder(&path);
                    } else if let Some(e) = fs::entry(&path) {
                        fs::open(&e);
                    }
                    opened = true;
                }
            }
        }
        if !opened && !args.iter().any(|a| a == "--desktop" || a == FILE_MANAGER_ARG) {
            self.open_folder(&fs::home());
        }
    }

    /// Rename `p` to `name`; reports failures and returns the new path.
    fn rename(&mut self, p: &Path, name: &str) -> Option<PathBuf> {
        let current = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = name.trim();
        if name.is_empty() || name == current {
            return None;
        }
        let r = fs::rename(p, name);
        if let Ok(new) = &r {
            if let Some(d) = self.desktop() {
                // A desktop icon keeps its place under its new name.
                d.messenger().send(DesktopMsg::Renamed(vec![(p.to_owned(), new.clone())]));
            }
            self.done("Rename", undo::Change::Moved(vec![(p.to_owned(), new.clone())]));
        }
        self.reload_all();
        match r {
            Ok(new) => Some(new),
            Err(e) => {
                error(format!("Couldn't rename “{current}”: {e}"));
                None
            }
        }
    }

    /// Watch exactly the folders on screen: every folder window's folder
    /// and columns, the desktop's, items' Info and Find's results.
    fn sync_watches(&mut self) {
        self.windows.retain(|s| s.is_open());
        let mut want = BTreeSet::new();
        for s in &self.windows {
            match &s.win {
                Win::Desktop(_) => want.extend(desktop::watched()),
                // (A folder's window before it has said what it shows:
                // its folder.)
                _ => match info_of(s.id) {
                    Some(info) => want.extend(info.watched),
                    None => want.extend(s.path.clone()),
                },
            }
        }
        if want != self.watched {
            self.watched = want.clone();
            if let Some(w) = &mut self.watcher {
                w.set(want);
            }
            // (DiskBackup's marks for the folders shown now.)
            backups::shown(&self.watched, &olo_kit::proxy());
        }
    }

    // ---- Quick Look -------------------------------------------------------

    /// Space / ⌘Y: show the selection in Quick Look, or close it.
    fn toggle_quick_look(&mut self, source: u64) {
        if let Some(l) = self.look.as_ref().filter(|l| l.window.is_open())
            && l.source == source
        {
            l.window.messenger().send(QuickMsg::Close);
            return;
        }
        let Some(entry) = info_of(source).and_then(|i| i.selected) else { return };
        self.show_quick_look(source, entry);
    }

    fn show_quick_look(&mut self, source: u64, entry: Entry) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let path = entry.path.clone();
        if let Some(l) = self.look.as_mut().filter(|l| l.window.is_open()) {
            l.source = source;
            l.path = path;
            l.window.messenger().send(QuickMsg::Show(source, entry, generation));
            return;
        }
        let window = olo_kit::open_window(quicklook::window_options(entry.name.clone()), move |cx| QuickLook::new(source, entry, generation, cx));
        self.look = Some(Looking { source, path, window });
    }

    /// While Quick Look shows a window's selection, follow it as it moves.
    fn follow_selection(&mut self, id: u64) {
        let Some(l) = self.look.as_ref().filter(|l| l.window.is_open() && l.source == id) else { return };
        match info_of(id).and_then(|i| i.selected) {
            Some(e) if e.path != l.path => self.show_quick_look(id, e),
            _ => {}
        }
    }

    // ---- jobs ---------------------------------------------------------------

    /// Run a file operation in the background. Short ones finish without a
    /// trace; longer ones show in the Tracker Status window.
    fn start_job(&mut self, kind: ops::Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, origin: Option<u64>) {
        // (What it does is a step Undo takes back; emptying the Trash isn't.)
        let record = (kind != ops::Kind::EmptyTrash).then(|| (undo::name_of(kind).to_owned(), undo::Purpose::Do));
        self.run_job(kind, sources, dest, Vec::new(), origin, record);
    }

    /// Edit ▸ Undo (or Redo): the step's inverse run as a job, what it
    /// does then the step for the other way (`undo.rs`).
    fn undo_redo(&mut self, redo: bool, origin: Option<u64>) {
        let step = if redo { self.history.take_redo() } else { self.history.take_undo() };
        let Some(step) = step else { return };
        let purpose = if redo { undo::Purpose::Redo } else { undo::Purpose::Undo };
        match step.change.inverse() {
            Some((kind, sources, targets)) => self.run_job(kind, sources, None, targets, origin, Some((step.name, purpose))),
            // Desktop icons: back where they were, at once.
            None => {
                if let undo::Change::Placed(moved) = &step.change {
                    if let Some(d) = self.desktop() {
                        d.messenger().send(DesktopMsg::PutPlaces(moved.clone()));
                    }
                    self.history.record(purpose, step.name, undo::Change::swapped(moved));
                }
            }
        }
        self.refresh_menus();
    }

    /// A step done here and now (a rename, a new folder, icons placed: no
    /// job).
    fn done(&mut self, name: &str, change: undo::Change) {
        if change.is_empty() {
            return;
        }
        self.history.done(name, change);
        self.refresh_menus();
    }

    /// The windows' menus again (Undo's name changed).
    fn refresh_menus(&mut self) {
        let h = &self.history;
        *shared().undo.lock().unwrap_or_else(|e| e.into_inner()) = [(h.undo_label(), h.can_undo()), (h.redo_label(), h.can_redo())];
        self.refresh();
    }

    fn run_job(&mut self, kind: ops::Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, targets: Vec<PathBuf>, origin: Option<u64>, record: Option<(String, undo::Purpose)>) {
        let id = self.next_job;
        self.next_job += 1;
        if let Some(r) = record {
            self.recording.insert(id, r);
        }
        let job = Arc::new(ops::start(id, kind, sources, dest, targets, move |outcome| to_hub(ToHub::JobDone(id, outcome))));
        std::thread::spawn(move || {
            std::thread::sleep(STATUS_DELAY);
            to_hub(ToHub::JobSlow(id));
        });
        self.jobs.push((job, origin));
        self.share_jobs();
    }

    fn share_jobs(&self) {
        *shared().jobs.lock().unwrap_or_else(|e| e.into_inner()) = self.jobs.iter().map(|(j, _)| j.clone()).collect();
    }

    fn job_done(&mut self, id: u64, outcome: ops::Outcome) {
        let Some(i) = self.jobs.iter().position(|(j, _)| j.id == id) else { return };
        let (job, origin) = self.jobs.remove(i);
        self.share_jobs();
        // What it did, for Undo (or Redo): some of it, if it stopped part
        // way. A desktop icon moved back by an undone rename keeps its place.
        if let Some((name, purpose)) = self.recording.remove(&id)
            && let Some(change) = undo::Change::of(job.kind, &outcome)
        {
            if let (undo::Change::Moved(pairs), Some(d)) = (&change, self.desktop()) {
                d.messenger().send(DesktopMsg::Renamed(pairs.clone()));
            }
            self.history.record(purpose, name, change);
        }
        self.refresh_menus();
        self.reload_all();
        // New items come up selected, as after a drop.
        if let Some(f) = origin.and_then(|k| self.folder(k))
            && !outcome.created.is_empty()
        {
            f.messenger().send(FolderMsg::Select(outcome.created.clone()));
        }
        // A long job may have been left to run in the background: say it's
        // done.
        if outcome.error.is_none() && !outcome.cancelled && job.started.elapsed() >= notify_after() {
            let note = Note::new(&job.finished);
            let note = if job.dest.is_some() { note.action("default", "Show") } else { note };
            let n = self.notes.show(None, note);
            if let Some(d) = job.dest.clone() {
                self.note_dirs.insert(n, d);
            }
        }
        if let Some(e) = outcome.error {
            let what = job.title.chars().next().map(|c| c.to_lowercase().chain(job.title.chars().skip(1)).collect::<String>());
            error(format!("An error occurred while {}: {e}", what.unwrap_or_default()));
        }
    }

    // ---- messages ---------------------------------------------------------

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args),
            Err(m) => m,
        };
        let message = match message.downcast::<NoteEvent>() {
            Ok(e) => {
                match *e {
                    NoteEvent::Action { id, token, .. } => {
                        if let Some(dir) = self.note_dirs.remove(&id) {
                            let path = dir.canonicalize().unwrap_or(dir);
                            match self.folder_window(&path).and_then(|k| self.folder(k)) {
                                Some(f) => {
                                    f.messenger().send(FolderMsg::Activate(token));
                                }
                                None => {
                                    self.open_folder(&path);
                                }
                            }
                        }
                    }
                    NoteEvent::Closed { id } => {
                        self.note_dirs.remove(&id);
                    }
                    NoteEvent::Replied { .. } => {}
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<ToHub>() else { return };
        match *msg {
            ToHub::Settings(s) => self.default_view = s.default_view,
            ToHub::Thumb | ToHub::Refresh => self.refresh(),
            ToHub::JobSlow(id) => {
                if self.jobs.iter().any(|(j, _)| j.id == id) && self.status.as_ref().is_none_or(|w| !w.is_open()) {
                    let h = panels::status_height(self.jobs.len());
                    self.status = Some(olo_kit::open_window(WindowOptions::new("Tracker Status", panels::STATUS_WIDTH, h).min_size(300.0, 60.0).floating(), StatusWin::new));
                }
            }
            ToHub::JobDone(id, outcome) => self.job_done(id, outcome),
            ToHub::Changed(changes) => self.changed(changes),
            ToHub::Marks(dir, names) => {
                if backups::answered(dir, names, &olo_kit::proxy()) {
                    self.refresh();
                }
            }
            ToHub::Backup(on, last_ok) => {
                if backups::followed(&self.watched, &olo_kit::proxy(), on, last_ok) {
                    self.refresh();
                }
            }
            ToHub::Told(id) => {
                self.sync_watches();
                self.follow_selection(id);
            }
            ToHub::Closed(id) => {
                self.windows.retain(|s| s.id != id);
                shared().infos.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                self.sync_watches();
            }
            ToHub::Open { path, from } => {
                let id = self.open_folder(&path);
                if let (Some(from), Some(f)) = (from, self.folder(id)) {
                    f.messenger().send(FolderMsg::CameFrom(from));
                }
            }
            ToHub::NewWindow(path, view) => {
                self.new_folder_window(path, view.unwrap_or(self.default_view), (560, 380), |_| {});
            }
            ToHub::Reveal(path, true) => self.show_in_parent(&path),
            ToHub::Reveal(path, false) => self.reveal(&path),
            ToHub::GetInfo(entries) => {
                for e in entries {
                    self.open_info(e);
                }
            }
            ToHub::Find => self.open_find(),
            ToHub::GoTo(base) => self.open_go_to(base),
            ToHub::Rename { win, path, name } => {
                if let Some(new) = self.rename(&path, &name)
                    && let Some(f) = win.and_then(|w| self.folder(w))
                {
                    f.messenger().send(FolderMsg::Select(vec![new]));
                }
            }
            ToHub::Job { kind, sources, dest, origin } => self.start_job(kind, sources, dest, origin),
            ToHub::Done(name, change) => self.done(&name, change),
            ToHub::UndoRedo(redo, origin) => self.undo_redo(redo, origin),
            ToHub::Hidden => {
                shared().hidden.fetch_xor(true, Ordering::Relaxed);
                self.reload_all();
            }
            ToHub::ReloadAll => self.reload_all(),
            ToHub::QuickLook(source) => self.toggle_quick_look(source),
            ToHub::Look(source, entry) => self.show_quick_look(source, entry),
            ToHub::LookStep(d) => {
                let Some(source) = self.look.as_ref().map(|l| l.source) else { return };
                let _ = match self.shown(source).map(|s| &s.win) {
                    Some(Win::Folder(f)) => f.messenger().send(FolderMsg::Step(d)),
                    Some(Win::Find(f)) => f.messenger().send(FindMsg::Step(d)),
                    _ => false,
                };
            }
            ToHub::LookClosed => {
                // The window it showed the selection of has the keyboard
                // again.
                if let Some(l) = self.look.take()
                    && let Some(s) = self.shown(l.source)
                {
                    s.raise();
                }
            }
            ToHub::StopJob(id) => {
                if let Some((job, _)) = self.jobs.iter().find(|(j, _)| j.id == id) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            ToHub::Gone(id) => {
                let _ = match self.shown(id).map(|s| &s.win) {
                    Some(Win::Folder(f)) => f.messenger().send(FolderMsg::Close),
                    Some(Win::Find(f)) => f.messenger().send(FindMsg::Close),
                    _ => false,
                };
            }
            ToHub::Quit => olo_kit::quit(),
        }
    }

    /// Changes seen on disk, to the windows showing them.
    fn changed(&mut self, changes: Vec<watch::Change>) {
        // (DiskBackup's marks of a folder changed: asked again.)
        let dirs: BTreeSet<&Path> = changes
            .iter()
            .filter_map(|c| match c {
                watch::Change::Item { dir, .. } | watch::Change::Rescan(dir) => Some(dir.as_path()),
                watch::Change::Mounts => None,
            })
            .collect();
        let proxy = olo_kit::proxy();
        for dir in dirs {
            backups::changed(dir, &proxy);
        }
        let changes = Arc::new(changes);
        for s in &self.windows {
            match &s.win {
                Win::Folder(f) => f.messenger().send(FolderMsg::Changed(changes.clone())),
                Win::Desktop(d) => d.messenger().send(DesktopMsg::Changed(changes.clone())),
                Win::Info(i) => i.messenger().send(InfoMsg::Changed(changes.clone())),
                Win::Find(f) => f.messenger().send(FindMsg::Changed(changes.clone())),
            };
        }
    }
}

/// One window to another display (an agent screen's, or from one to the
/// person's): a folder or Find window goes as a Tracker there opens it,
/// as it was; the desktop and the rest stay.
fn move_window(surface: u32) -> Result<olo_kit::WindowMove, String> {
    let (id, info) = shared().infos.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(_, i)| i.surface == Some(surface)).map(|(id, i)| (*id, i.clone())).ok_or("it has no such window")?;
    let mut saved = Saved { hidden: hidden(), folders: Vec::new(), find: 0, desktop_selected: Vec::new() };
    match (info.folder, info.find) {
        (Some(f), _) => saved.folders.push(f),
        (None, true) => saved.find = 1,
        (None, false) if info.path == fs::home().join("Desktop") => return Err("the desktop stays with its display".into()),
        _ => return Err("only folder and Find windows move".into()),
    }
    Ok(olo_kit::WindowMove { args: Vec::new(), state: serde_json::to_string(&saved).ok(), gone: Box::new(move || to_hub(ToHub::Gone(id))) })
}

/// Started by D-Bus to be the file manager other programs ask
/// (`filemanager`): serving `org.freedesktop.FileManager1` without the
/// desktop and without opening a window of its own (Olo Doctor's "Always
/// Use Tracker" names it in the user's activation file for the name, so
/// D-Bus starts Tracker, never another file manager, when no Tracker has
/// it: as Olo restarts after an upgrade, or outside an Olo session).
/// Passed to a Tracker running already, it changes nothing.
pub const FILE_MANAGER_ARG: &str = "--file-manager";

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        // The session's desktop, handed to a Tracker D-Bus started first
        // (as the file manager, `FILE_MANAGER_ARG`), which shows it now.
        // This process is the one the session looks after: it stays
        // while that one runs, so when that one goes the session starts
        // the desktop's Tracker again (gone at once, it was started
        // again and again, then left stopped).
        if args.iter().any(|a| a == "--desktop")
            && let Some(pid) = instance::running_pid(APP_ID)
        {
            instance::wait_for_exit(pid);
        }
        return;
    }
    olo_preview::audio::set_identity("Tracker", "olo-tracker");
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        workspaces::follow();
        thumbs::init(olo_kit::proxy());
        let watcher = watch::Watcher::new(|changes| to_hub(ToHub::Changed(changes)));
        olo_kit_base::settings::watch(|s| {
            if let Ok(mut u) = fs::UNGROUPED.lock() {
                *u = s.tiling.ungrouped_apps.clone();
            }
            to_hub(ToHub::Settings(s.tracker))
        });
        let desktop = args.iter().any(|a| a == "--desktop");
        let file_manager = desktop || args.iter().any(|a| a == FILE_MANAGER_ARG);
        let mut hub = Hub {
            windows: Vec::new(),
            next: 0,
            // Folders opened at startup can't wait for the watcher's first
            // read.
            default_view: olo_kit_base::settings::current().map(|s| s.tracker.default_view).unwrap_or_default(),
            jobs: Vec::new(),
            next_job: 1,
            history: undo::History::default(),
            recording: HashMap::new(),
            status: None,
            go_to: None,
            notes: Notifier::new(olo_kit::proxy(), "Tracker", "olo-tracker"),
            note_dirs: HashMap::new(),
            watcher,
            watched: BTreeSet::new(),
            look: None,
            preview_generation: 0,
        };
        // DiskBackup, followed: its ticks (`backups`). Its state file is
        // written every 2 s while it backs up: looked at on the watch's
        // thread, the app told only when it went on or off or finished one.
        let was = std::sync::Mutex::new(backups::state());
        let (on, last_ok) = backups::state();
        let backup_watch = olo_kit::watch::watch([olo_protocol::diskbackup::State::path(), olo_protocol::diskbackup::Settings::path()], move |_| {
            let now = backups::state();
            if let Ok(mut was) = was.lock()
                && *was != now
            {
                *was = now;
                to_hub(ToHub::Backup(now.0, now.1));
            }
        });
        // (Tracker's for as long as it runs.)
        std::mem::forget(backup_watch);
        to_hub(ToHub::Backup(on, last_ok));
        // What each window keeps, as one state: its folder windows as they
        // are, how many Find windows, what's selected on the desktop.
        olo_kit::on_upgrade(|windows| {
            let mut saved = Saved { hidden: hidden(), folders: Vec::new(), find: 0, desktop_selected: Vec::new() };
            for w in windows.iter().filter_map(|s| serde_json::from_str::<SavedWindow>(s).ok()) {
                match w {
                    SavedWindow::Folder(f) => saved.folders.push(f),
                    SavedWindow::Find => saved.find += 1,
                    SavedWindow::Desktop(selected) => saved.desktop_selected = selected,
                }
            }
            serde_json::to_string(&saved).ok()
        });
        olo_kit::on_move_window(move_window);
        // A file operation under way: the new version waits for it.
        olo_kit::upgrade_waits_while(|| !shared().jobs.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        let restored = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        if desktop {
            hub.open_desktop(restored.as_ref().map(|s| s.desktop_selected.clone()).unwrap_or_default());
        }
        if file_manager {
            // (Started by D-Bus for the name alone: it stays to answer.)
            olo_kit::stay_resident();
            // Other programs' "show in folder", while the desktop's Tracker
            // runs, or one D-Bus started for it (`filemanager`).
            let proxy = olo_kit::proxy();
            let _ = std::thread::Builder::new().name("tracker-filemanager".into()).spawn(move || {
                let reveal = move |paths: Vec<PathBuf>| {
                    let args = paths.into_iter().flat_map(|p| ["--reveal".to_owned(), p.display().to_string()]).collect();
                    proxy.send(Launch { args, cwd: None, taken: None });
                };
                match filemanager::serve(reveal) {
                    Ok((_conn, ours)) => {
                        if !ours {
                            tracing::info!("tracker: {} is another program's for now: waiting for it", filemanager::NAME);
                        }
                        loop {
                            std::thread::park();
                        }
                    }
                    Err(e) => tracing::info!("tracker: not serving {}: {e}", filemanager::NAME),
                }
            });
        }
        match restored {
            Some(saved) => hub.restore(saved),
            None => hub.handle_args(&args),
        }
        olo_kit::message_handler(app, move |message, _| hub.message(message));
    });
}
