//! Tracker: the desktop (icons for volumes, Home, Trash and ~/Desktop) and
//! spatial folder windows, one per folder on each workspace, as on BeOS.
//! On Olo's GPUI: the [`Tracker`] keeps the windows and does what opens
//! windows or touches files; each window's view does the rest.

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
mod watch;

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use olo_ui::{
    Menu, WindowOptions,
    gpui::{self, AnyWindowHandle, App, AppContext as _, Context, DragAction, Dropped, Entity, EntityId, Global, Window},
    instance::{self, Launch},
    menu,
    notify::{Note, NoteEvent, NoteId, Notifier},
    places,
    widgets::open_alert,
    workspaces,
};

pub use fs::Entry;
/// How a window shows its folder; new windows follow Preferences ▸ Tracker.
pub use olo_protocol::FolderView as ViewMode;

use crate::{
    desktop::Desktop,
    folder::{Action, FolderWin},
    items::DropTarget,
    model::SortKey,
    panels::{FindWin, InfoWin, StatusWin},
    quicklook::QuickLook,
};

const APP_ID: &str = "olo.tracker";

pub enum Msg {
    DirSize(EntityId, u64, u64),
    Found(EntityId, u64, Vec<find::Hit>, bool),
    JobDone(u64, ops::Outcome),
    Listed {
        win: EntityId,
        path: PathBuf,
        generation: u64,
        result: Result<Vec<Entry>, String>,
    },
    /// A job has been running a while: time to show the status window.
    JobSlow(u64),
    /// Files changed in folders being shown.
    Changed(Vec<watch::Change>),
    /// Quick Look rotated (or saved) the picture at a path.
    Edited(PathBuf, quicklook::EditEvent),
    /// Preferences ▸ Tracker changed (or was first read).
    Settings(olo_protocol::TrackerSettings),
    /// A thumbnail is ready.
    Thumb,
}

/// What Paste takes from the clipboard, best first: files (as file
/// managers copy them), a picture, text.
const PASTEABLE: &[&str] =
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
pub fn can_paste(cx: &App) -> bool {
    let types = cx.clipboard_types();
    PASTEABLE.iter().any(|w| types.iter().any(|t| if w.ends_with('/') { t.starts_with(w) } else { t == w }))
}

/// The standard folders, favourites and recent folders (the Go menu), as
/// last read: read again on a worker when a few seconds old, so a menu
/// never waits for the disk.
struct PlacesCache {
    places: Arc<places::Places>,
    read: Option<Instant>,
    reading: bool,
}

impl Global for PlacesCache {}

pub fn places(cx: &mut App) -> Arc<places::Places> {
    if !cx.has_global::<PlacesCache>() {
        cx.set_global(PlacesCache { places: Arc::new(places::Places::default()), read: None, reading: false });
    }
    let c = cx.global::<PlacesCache>();
    let (stale, out) = (c.read.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) && !c.reading, c.places.clone());
    if stale {
        cx.global_mut::<PlacesCache>().reading = true;
        cx.spawn(async move |cx| {
            let p = cx
                .background_spawn(async move {
                    let mut p = places::load();
                    // Only folders that are there.
                    p.favorites.retain(|d| d.is_dir());
                    p.recent.retain(|d| d.is_dir());
                    p
                })
                .await;
            let _ = cx.update(|cx| {
                let changed = *cx.global::<PlacesCache>().places != p;
                cx.set_global(PlacesCache { places: Arc::new(p), read: Some(Instant::now()), reading: false });
                if changed {
                    cx.refresh_windows();
                }
            });
        })
        .detach();
    }
    out
}

enum Win {
    Folder(Entity<FolderWin>),
    Desktop(Entity<Desktop>),
    Info(Entity<InfoWin>),
    Find(Entity<FindWin>),
    /// Go to File or Folder (`goto`).
    GoTo,
    /// Open With ▸ Other….
    OpenWith,
    /// "Tracker Status": progress of running file operations (its view
    /// reads the jobs from here).
    Status,
    /// The Quick Look window (its state is `Tracker::quicklook`).
    Preview,
}

/// One of Tracker's windows: its view's id (the key it's known by), its
/// window and what it is.
struct Shown {
    key: EntityId,
    handle: AnyWindowHandle,
    win: Win,
}

pub struct Tracker {
    /// In opening order.
    windows: Vec<Shown>,
    hidden: bool,
    /// How new folder windows show their folder (Preferences ▸ Tracker).
    default_view: ViewMode,
    /// Running file operations, each with the window that started it.
    pub jobs: Vec<(ops::Job, Option<EntityId>)>,
    next_job: u64,
    status: Option<EntityId>,
    /// Long jobs say when they're done; Show opens where the items went.
    notes: Notifier,
    note_dirs: HashMap<NoteId, PathBuf>,
    /// Live updates for every folder shown (None if inotify is missing).
    watcher: Option<watch::Watcher>,
    /// What the watcher was last given.
    watched: BTreeSet<PathBuf>,
    quicklook: Option<Entity<QuickLook>>,
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

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedFolder {
    path: PathBuf,
    size: (u32, u32),
    view: ViewMode,
    sort: (SortKey, bool),
    /// `sort` was chosen (else Olo's own order: as before it was kept).
    #[serde(default)]
    own_sort: bool,
    selected: Vec<PathBuf>,
}

impl Tracker {
    fn shown(&self, key: EntityId) -> Option<&Shown> {
        self.windows.iter().find(|s| s.key == key)
    }

    fn folder(&self, key: EntityId) -> Option<Entity<FolderWin>> {
        match self.shown(key).map(|s| &s.win) {
            Some(Win::Folder(f)) => Some(f.clone()),
            _ => None,
        }
    }

    fn desktop(&self) -> Option<Entity<Desktop>> {
        self.windows.iter().find_map(|s| match &s.win {
            Win::Desktop(d) => Some(d.clone()),
            _ => None,
        })
    }

    /// Takes a window just opened: closing it, and drops on it, come here.
    fn register(&mut self, key: EntityId, handle: AnyWindowHandle, win: Win, cx: &mut Context<Self>) {
        let (closing, dropping) = (cx.weak_entity(), cx.weak_entity());
        let _ = handle.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, move |_, cx| closing.upgrade().is_none_or(|t| t.update(cx, |t, cx| t.close_requested(key, cx))));
            window.on_dropped(move |d, window, cx| {
                if let Some(t) = dropping.upgrade() {
                    t.update(cx, |t, cx| t.drop_on(key, d, window, cx));
                }
            });
        });
        self.windows.push(Shown { key, handle, win });
        self.sync_watches(cx);
    }

    fn activate(&self, key: EntityId, cx: &mut App) {
        if let Some(s) = self.shown(key) {
            olo_ui::raise(Some(s.handle), cx);
        }
    }

    /// The folder's window, brought forward, if it has one on this
    /// workspace; else a new one here (each workspace has its own).
    fn open_folder(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<EntityId> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        // A recent folder now, for the Go menu and file panels (off the
        // UI thread).
        let p = path.clone();
        std::thread::spawn(move || places::visited(&p));
        if let Some(key) = self.folder_window(&path, cx) {
            self.activate(key, cx);
            return Some(key);
        }
        self.new_folder_window(path, self.default_view, (560, 380), |_| {}, cx)
    }

    fn new_folder_window(&mut self, path: PathBuf, view: ViewMode, (w, h): (u32, u32), setup: impl FnOnce(&mut FolderWin), cx: &mut Context<Self>) -> Option<EntityId> {
        let (app, hidden) = (cx.weak_entity(), self.hidden);
        let folder = cx.new(|cx| {
            let mut f = FolderWin::new(path.clone(), view, hidden, app, cx);
            setup(&mut f);
            f
        });
        let key = folder.entity_id();
        let content = folder.clone();
        let handle = olo_ui::open_window(cx, WindowOptions::new(fs::display_name(&path), w as f32, h as f32).min_size(240.0, 160.0), move |_, _| content)?;
        // What it shows (its columns) and what it has selected (for Quick
        // Look) are followed as they change.
        cx.observe(&folder, |t, f, cx| {
            t.sync_watches(cx);
            t.follow_selection(f.entity_id(), cx);
        })
        .detach();
        self.register(key, handle.into(), Win::Folder(folder), cx);
        Some(key)
    }

    /// `path`'s window on this workspace, if there is one.
    fn folder_window(&self, path: &Path, cx: &App) -> Option<EntityId> {
        self.windows
            .iter()
            .find(|s| match &s.win {
                Win::Folder(f) => {
                    let f = f.read(cx);
                    f.path == path && f.surface.and_then(workspaces::here_surface).unwrap_or(true)
                }
                _ => false,
            })
            .map(|s| s.key)
    }

    /// Show `path`: a folder opens; anything else is selected in its
    /// folder's window.
    fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.open_folder(path, cx);
        } else {
            self.show_in_parent(path, cx);
        }
    }

    /// Open `path`'s folder with it selected.
    fn show_in_parent(&mut self, path: &Path, cx: &mut Context<Self>) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
        let Some(key) = self.open_folder(&dir, cx) else { return };
        if let Some(f) = self.folder(key) {
            f.update(cx, |f, cx| {
                f.select_paths(&[dir.join(name)]);
                cx.notify();
            });
        }
    }

    fn open_entry(&mut self, e: &Entry, cx: &mut Context<Self>) {
        if e.is_dir {
            self.open_folder(&e.path, cx);
        } else {
            fs::open(e);
        }
    }

    /// Open With ▸ an app (`open-with:ID`; Always: and make it the kind's,
    /// `always-open-with:ID`), or Other… (`open-with-other`: a window to
    /// choose any app).
    fn open_with(&mut self, cmd: &str, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let Some(kind) = folder::open_with_kind(&entries) else { return };
        let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        if cmd == olo_ui::open_with::OTHER {
            return self.open_with_other(kind, paths, cx);
        }
        olo_ui::open_with::run(cmd, kind, paths, cx);
    }

    fn open_info(&mut self, e: Entry, cx: &mut Context<Self>) {
        let h = panels::info_height(panels::info_rows(&e)).ceil();
        let title = format!("{} info", e.name);
        let dir = e.is_dir.then(|| e.path.clone());
        let info = cx.new(|_| InfoWin::new(e));
        let key = info.entity_id();
        let content = info.clone();
        let Some(handle) = olo_ui::open_window(cx, WindowOptions::new(title, 360.0, h).min_size(300.0, h).floating(), move |_, _| content) else { return };
        if let Some(path) = dir {
            let proxy = olo_ui::proxy(cx);
            std::thread::spawn(move || {
                let mut limit = 200_000;
                let (b, f) = fs::dir_size(&path, &mut limit);
                proxy.send(Msg::DirSize(key, b, f));
            });
        }
        self.register(key, handle.into(), Win::Info(info), cx);
    }

    fn open_find(&mut self, cx: &mut Context<Self>) {
        let app = cx.weak_entity();
        let made: std::rc::Rc<std::cell::RefCell<Option<Entity<FindWin>>>> = Default::default();
        let keep = made.clone();
        let Some(handle) = olo_ui::open_window(cx, WindowOptions::new("Find", 680.0, 420.0).min_size(440.0, 260.0), move |window, cx| {
            let v = cx.new(|cx| FindWin::new(app, window, cx));
            *keep.borrow_mut() = Some(v.clone());
            v
        }) else {
            return;
        };
        let Some(find) = made.borrow_mut().take() else { return };
        cx.observe(&find, |t, f, cx| {
            t.sync_watches(cx);
            t.follow_selection(f.entity_id(), cx);
        })
        .detach();
        self.register(find.entity_id(), handle.into(), Win::Find(find), cx);
    }

    /// Go to File or Folder, from the window whose folder is `base` (a
    /// relative path is from there): the one open brought forward, else
    /// a new one.
    fn open_go_to(&mut self, base: PathBuf, cx: &mut Context<Self>) {
        if let Some(key) = self.windows.iter().find(|s| matches!(s.win, Win::GoTo)).map(|s| s.key) {
            return self.activate(key, cx);
        }
        let app = cx.weak_entity();
        let made: std::rc::Rc<std::cell::RefCell<Option<Entity<goto::GoToWin>>>> = Default::default();
        let keep = made.clone();
        // (A dialog: over the folder's window, never a tab in its group.)
        let options = WindowOptions::new("Go to File or Folder", goto::WIDTH, goto::HEIGHT).min_size(360.0, 200.0).dialog().child();
        let Some(handle) = olo_ui::open_window(cx, options, move |window, cx| {
            let v = cx.new(|cx| goto::GoToWin::new(app, base, window, cx));
            *keep.borrow_mut() = Some(v.clone());
            v
        }) else {
            return;
        };
        let Some(go) = made.borrow_mut().take() else { return };
        let key = go.entity_id();
        let any: AnyWindowHandle = handle.into();
        cx.subscribe(&go, move |t, _, _: &goto::GoToEvent, cx| {
            if t.close_requested(key, cx) {
                cx.close_window(any);
            }
        })
        .detach();
        self.register(key, any, Win::GoTo, cx);
    }

    /// Open With ▸ Other…: a window to choose any app to open `paths`
    /// (of `kind`) with.
    fn open_with_other(&mut self, kind: String, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(key) = self.windows.iter().find(|s| matches!(s.win, Win::OpenWith)).map(|s| s.key) {
            return self.activate(key, cx);
        }
        let made: std::rc::Rc<std::cell::RefCell<Option<Entity<olo_ui::open_with::OpenWithWin>>>> = Default::default();
        let keep = made.clone();
        let Some(handle) = olo_ui::open_window(cx, olo_ui::open_with::window_options(), move |window, cx| {
            let v = cx.new(|cx| olo_ui::open_with::OpenWithWin::new(kind, paths, window, cx));
            *keep.borrow_mut() = Some(v.clone());
            v
        }) else {
            return;
        };
        let Some(win) = made.borrow_mut().take() else { return };
        let key = win.entity_id();
        let any: AnyWindowHandle = handle.into();
        cx.subscribe(&win, move |t, _, _: &olo_ui::open_with::OpenWithEvent, cx| {
            if t.close_requested(key, cx) {
                cx.close_window(any);
            }
        })
        .detach();
        self.register(key, any, Win::OpenWith, cx);
    }

    /// Show `path`: a folder opens; anything else is selected in its
    /// folder's window (Go to File or Folder, `go-to:`).
    pub fn go_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.reveal(path, cx);
    }

    fn error(&mut self, message: String, cx: &mut Context<Self>) {
        let _ = open_alert(cx, "Tracker", "about", &message, &["OK"], |_, _| {});
    }

    fn confirm_empty_trash(&mut self, cx: &mut Context<Self>) {
        let me = cx.weak_entity();
        let _ = open_alert(cx, "Empty Trash", "trash-full", "Are you sure you want to permanently remove all items from the Trash?", &["Cancel", "Empty Trash"], move |choice, cx| {
            if choice == Some(1)
                && let Some(t) = me.upgrade()
            {
                t.update(cx, |t, cx| t.start_job(ops::Kind::EmptyTrash, Vec::new(), None, None, cx));
            }
        });
    }

    fn reload_all(&mut self, cx: &mut Context<Self>) {
        let hidden = self.hidden;
        for s in &self.windows {
            match &s.win {
                Win::Folder(f) => f.update(cx, |f, cx| {
                    f.reload(hidden);
                    cx.notify();
                }),
                Win::Desktop(d) => d.update(cx, |d, cx| {
                    d.reload();
                    cx.notify();
                }),
                _ => {}
            }
        }
    }

    fn restore(&mut self, saved: Saved, cx: &mut Context<Self>) {
        self.hidden = saved.hidden;
        if let Some(d) = self.desktop() {
            d.update(cx, |d, cx| {
                d.select_paths(&saved.desktop_selected);
                cx.notify();
            });
        }
        for f in saved.folders {
            if !f.path.is_dir() {
                continue;
            }
            let (sort, selected) = (f.own_sort.then_some(f.sort), f.selected);
            self.new_folder_window(
                f.path,
                f.view,
                f.size,
                move |w| {
                    if sort.is_some() {
                        w.sort = sort;
                    }
                    w.select_paths(&selected);
                },
                cx,
            );
        }
        for _ in 0..saved.find {
            self.open_find(cx);
        }
    }

    fn handle_args(&mut self, args: &[String], cx: &mut Context<Self>) {
        let mut opened = false;
        let mut left = args.iter();
        while let Some(a) = left.next() {
            match a.as_str() {
                "--find" => {
                    self.open_find(cx);
                    opened = true;
                }
                // Any app's Open in Tracker (`olo_kit_base::tracker`).
                "--reveal" => {
                    if let Some(p) = left.next() {
                        self.reveal(Path::new(p), cx);
                    }
                    opened = true;
                }
                "--desktop" => {}
                // Another window, always (not one brought forward): home.
                "--new-window" => {
                    self.new_folder_window(fs::home(), self.default_view, (560, 380), |_| {}, cx);
                    opened = true;
                }
                p => {
                    let path = PathBuf::from(p);
                    if path.is_dir() {
                        self.open_folder(&path, cx);
                    } else if let Some(e) = fs::entry(&path) {
                        fs::open(&e);
                    }
                    opened = true;
                }
            }
        }
        if !opened && !args.iter().any(|a| a == "--desktop") {
            self.open_folder(&fs::home(), cx);
        }
    }

    // ---- what windows ask -------------------------------------------------

    pub fn folder_action(&mut self, key: EntityId, a: Action, cx: &mut Context<Self>) {
        match a {
            Action::Open(e) => self.open_entry(&e, cx),
            Action::Command(c) => self.folder_command(key, &c, cx),
            Action::Rename(p, name) => {
                if let Some(new) = self.rename(&p, &name, cx)
                    && let Some(f) = self.folder(key)
                {
                    f.update(cx, |f, cx| {
                        f.select_paths(&[new]);
                        cx.notify();
                    });
                }
            }
            Action::Spring(dir) => {
                self.open_folder(&dir, cx);
            }
        }
    }

    pub fn desktop_action(&mut self, a: Action, cx: &mut Context<Self>) {
        let Some(d) = self.desktop() else { return };
        let key = d.entity_id();
        match a {
            Action::Open(e) => self.open_entry(&e, cx),
            Action::Rename(p, name) => {
                self.rename(&p, &name, cx);
            }
            Action::Spring(dir) => {
                self.open_folder(&dir, cx);
            }
            Action::Command(c) => match c.as_str() {
                c if olo_ui::open_with::is_command(c) => {
                    let entries = d.read(cx).selected_entries();
                    self.open_with(c, entries, cx);
                }
                "new-folder" => {
                    let dir = fs::home().join("Desktop");
                    let _ = std::fs::create_dir_all(&dir);
                    match fs::new_folder(&dir) {
                        Ok(p) => d.update(cx, |d, cx| {
                            d.reload();
                            d.edit_name(&p, cx);
                        }),
                        Err(e) => self.error(format!("Couldn't create a folder: {e}"), cx),
                    }
                }
                "empty-trash" => self.confirm_empty_trash(cx),
                "terminal" => fs::open_in_terminal(&fs::home().join("Desktop")),
                // From the desktop: a window on Home.
                "new" => {
                    self.new_folder_window(fs::home(), self.default_view, (560, 380), |_| {}, cx);
                }
                "backgrounds" => {
                    let _ = olo_xdg::spawn_detached("olo-preferences backgrounds");
                }
                "trash" => {
                    let paths = d.read(cx).movable();
                    if !paths.is_empty() {
                        self.start_job(ops::Kind::Trash, paths, None, Some(key), cx);
                    }
                }
                "info" => {
                    for e in d.read(cx).selected_entries() {
                        self.open_info(e, cx);
                    }
                }
                _ => {}
            },
        }
    }

    pub fn folder_command(&mut self, key: EntityId, cmd: &str, cx: &mut Context<Self>) {
        let Some(fe) = self.folder(key) else { return };
        let (selected, entries, dir) = {
            let f = fe.read(cx);
            (f.selected_paths(), f.selected_entries(), f.current_dir())
        };
        let update = |cx: &mut Context<Self>, g: &mut dyn FnMut(&mut FolderWin, &mut Context<FolderWin>)| {
            fe.update(cx, |f, cx| {
                g(f, cx);
                cx.notify();
            })
        };
        match cmd {
            "quick-look" => self.toggle_quick_look(key, cx),
            "open" => {
                for e in &entries {
                    self.open_entry(e, cx);
                }
            }
            // The folder it's in, with this one selected (to go on from).
            "parent" => self.show_in_parent(&dir, cx),
            "new-folder" => match fs::new_folder(&dir) {
                Ok(p) => update(cx, &mut |f, cx| {
                    f.add_entry(&p);
                    f.edit_name(&p, cx);
                }),
                Err(e) => self.error(format!("Couldn't create a folder: {e}"), cx),
            },
            "terminal" => fs::open_in_terminal(&dir),
            // File ▸ New Window (⌘N): another window on this folder, as
            // it shows it, beside this one (always a new one).
            "new" => {
                let view = fe.read(cx).view;
                self.new_folder_window(dir.clone(), view, (560, 380), |_| {}, cx);
            }
            "new-window" => {
                if let Some(app) = entries.first().and_then(fs::running_app) {
                    fs::new_window(app);
                }
            }
            "group-windows" => fs::toggle_grouping(entries.first()),
            "terminal-selected" => {
                for e in entries.iter().filter(|e| e.is_dir) {
                    fs::open_in_terminal(&e.path);
                }
            }
            "info" => {
                if entries.is_empty()
                    && let Some(e) = fs::entry(&dir)
                {
                    self.open_info(e, cx);
                }
                for e in entries {
                    self.open_info(e, cx);
                }
            }
            "rename" => {
                if let [p] = selected.as_slice() {
                    let p = p.clone();
                    update(cx, &mut |f, cx| f.edit_name(&p, cx));
                }
            }
            "duplicate" => {
                if !selected.is_empty() {
                    self.start_job(ops::Kind::Duplicate, selected, None, Some(key), cx);
                }
            }
            "link" => {
                for p in &selected {
                    let _ = fs::make_link(p);
                }
                self.reload_all(cx);
            }
            "trash" => {
                if !selected.is_empty() {
                    self.start_job(ops::Kind::Trash, selected, None, Some(key), cx);
                }
            }
            "restore" => {
                let top = fs::trash_files();
                let items: Vec<PathBuf> = selected.into_iter().filter(|p| p.parent() == Some(top.as_path())).collect();
                if !items.is_empty() {
                    self.start_job(ops::Kind::Restore, items, None, Some(key), cx);
                }
            }
            "empty-trash" => self.confirm_empty_trash(cx),
            "cut" | "copy" => {
                // As file managers copy files, so they paste into Chrome,
                // Slack, GTK's and Qt's apps and other file managers too.
                if !selected.is_empty() {
                    copy_clip(olo_kit_base::clipboard::files(&selected, cmd == "cut"), cx);
                }
            }
            "paste" => {
                let task = cx.paste_as(PASTEABLE);
                cx.spawn(async move |t, cx| {
                    if let Some((mime, data)) = task.await {
                        let _ = t.update(cx, |t, cx| t.pasted(key, &dir, &mime, &data, cx));
                    }
                })
                .detach();
            }
            "select-all" => update(cx, &mut |f, _| {
                let (entries, list) = f.current_mut();
                list.select_all(entries.len());
            }),
            "invert" => update(cx, &mut |f, _| {
                let (entries, list) = f.current_mut();
                list.invert(entries.len());
            }),
            "view-icons" => update(cx, &mut |f, _| f.set_view(ViewMode::Icons)),
            "view-mini" => update(cx, &mut |f, _| f.set_view(ViewMode::MiniIcons)),
            "view-list" => update(cx, &mut |f, _| f.set_view(ViewMode::List)),
            "view-columns" => update(cx, &mut |f, _| f.set_view(ViewMode::Columns)),
            "hidden" => {
                self.hidden = !self.hidden;
                self.reload_all(cx);
            }
            "sort-default" => update(cx, &mut |f, cx| {
                f.sort = None;
                sorts::remember(cx, &f.path, None);
                f.resort();
            }),
            "sort-name" | "sort-size" | "sort-modified" | "sort-kind" => {
                let key = match cmd {
                    "sort-size" => SortKey::Size,
                    "sort-modified" => SortKey::Modified,
                    "sort-kind" => SortKey::Kind,
                    _ => SortKey::Name,
                };
                update(cx, &mut |f, cx| {
                    f.sort = Some(match f.sort {
                        Some((k, asc)) if k == key => (key, !asc),
                        _ => (key, true),
                    });
                    sorts::remember(cx, &f.path, f.sort);
                    f.resort();
                });
            }
            "go-home" => {
                self.open_folder(&fs::home(), cx);
            }
            "go-desktop" => {
                self.open_folder(&fs::home().join("Desktop"), cx);
            }
            "go-root" => {
                self.open_folder(Path::new("/"), cx);
            }
            "go-trash" => {
                let t = fs::trash_files();
                let _ = std::fs::create_dir_all(&t);
                self.open_folder(&t, cx);
            }
            "favorite-on" | "favorite-off" => {
                let on = cmd == "favorite-on";
                std::thread::spawn(move || places::set_favorite(&dir, on));
                if cx.has_global::<PlacesCache>() {
                    cx.global_mut::<PlacesCache>().read = None;
                }
            }
            "go-to" => self.open_go_to(dir.clone(), cx),
            // (Agents': straight there.)
            c if c.starts_with("go-to:") => {
                if let Some(p) = goto::parse(&c["go-to:".len()..], &dir) {
                    self.go_to(&p, cx);
                }
            }
            // Open With ▸ an app (Always: and make it the kind's).
            c if olo_ui::open_with::is_command(c) => self.open_with(c, entries, cx),
            c if c.starts_with("go-place:") => {
                let dir = PathBuf::from(&c["go-place:".len()..]);
                if dir.is_dir() {
                    self.open_folder(&dir, cx);
                }
            }
            "find" => self.open_find(cx),
            "close" => {
                if let Some(h) = self.shown(key).map(|s| s.handle)
                    && self.close_requested(key, cx)
                {
                    cx.close_window(h);
                }
            }
            "quit" => cx.quit(),
            _ => {}
        }
    }

    /// What Find's result menu (or its keys) chose, for its selection.
    pub fn find_command(&mut self, key: EntityId, cmd: &str, cx: &mut Context<Self>) {
        let Some(Win::Find(f)) = self.shown(key).map(|s| &s.win) else { return };
        let Some(e) = f.read(cx).selected(cx) else { return };
        match cmd {
            "open" => self.open_entry(&e, cx),
            "parent" => self.show_in_parent(&e.path, cx),
            "quick-look" => self.toggle_quick_look(key, cx),
            _ => {}
        }
    }

    /// Rename `p` to `name`; reports failures and returns the new path.
    fn rename(&mut self, p: &Path, name: &str, cx: &mut Context<Self>) -> Option<PathBuf> {
        let current = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = name.trim();
        if name.is_empty() || name == current {
            return None;
        }
        let r = fs::rename(p, name);
        if let Ok(new) = &r
            && let Some(d) = self.desktop()
        {
            // A desktop icon keeps its place under its new name.
            d.update(cx, |d, _| d.renamed(p, new));
        }
        self.reload_all(cx);
        match r {
            Ok(new) => Some(new),
            Err(e) => {
                self.error(format!("Couldn't rename “{current}”: {e}"), cx);
                None
            }
        }
    }

    /// A window's close box (or Close): the desktop stays; anything else
    /// goes, and what it was doing with it.
    fn close_requested(&mut self, key: EntityId, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.windows.iter().position(|s| s.key == key) else { return true };
        if matches!(self.windows[i].win, Win::Desktop(_)) {
            return false;
        }
        let s = self.windows.remove(i);
        match s.win {
            Win::Preview => {
                if let Some(q) = self.quicklook.take() {
                    q.update(cx, |q, cx| q.stop(cx));
                }
            }
            Win::Find(f) => f.read(cx).cancel.store(true, Ordering::Relaxed),
            Win::Status => self.status = None,
            _ => {}
        }
        self.sync_watches(cx);
        true
    }

    /// Watch exactly the folders on screen: every folder window's folder
    /// and columns, the desktop's, items' Info and Find's results.
    fn sync_watches(&mut self, cx: &mut Context<Self>) {
        let mut want = BTreeSet::new();
        for s in &self.windows {
            match &s.win {
                Win::Folder(f) => want.extend(f.read(cx).watched().map(Path::to_owned)),
                Win::Desktop(_) => want.extend(desktop::watched()),
                // An item's Info follows it; Find's results theirs (as
                // many folders as is reasonable).
                Win::Info(i) => want.extend(i.read(cx).entry.path.parent().map(Path::to_owned)),
                Win::Find(f) => want.extend(f.update(cx, |f, _| f.followed().to_vec())),
                _ => {}
            }
        }
        if want != self.watched
            && let Some(w) = &mut self.watcher
        {
            self.watched = want.clone();
            w.set(want);
        }
    }

    // ---- Quick Look -------------------------------------------------------

    /// The single selected item of a folder or Find window, or the desktop.
    fn selected_one(&self, key: EntityId, cx: &App) -> Option<Entry> {
        match &self.shown(key)?.win {
            Win::Folder(f) => f.read(cx).selected_entries().into_iter().next(),
            Win::Desktop(d) => d.read(cx).selected_entries().into_iter().next(),
            Win::Find(f) => f.read(cx).selected(cx),
            _ => None,
        }
    }

    /// Space / ⌘Y: show the selection in Quick Look, or close it.
    fn toggle_quick_look(&mut self, source: EntityId, cx: &mut Context<Self>) {
        if let Some(q) = &self.quicklook
            && q.read(cx).source == source
        {
            return self.close_quick_look(cx);
        }
        let Some(entry) = self.selected_one(source, cx) else { return };
        self.show_quick_look(source, entry, cx);
    }

    fn show_quick_look(&mut self, source: EntityId, entry: Entry, cx: &mut Context<Self>) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        if let Some(q) = &self.quicklook {
            let title = entry.name.clone();
            q.update(cx, |q, cx| q.retarget(source, entry, generation, cx));
            if let Some(win) = q.read(cx).win {
                let _ = win.update(cx, |frame, window, _| frame.set_title(title, window));
            }
            return;
        }
        let app = cx.weak_entity();
        let q = cx.new(|cx| QuickLook::new(app, source, entry, generation, cx));
        self.quicklook = Some(q.clone());
        // The window opens when the preview is ready, or after a moment
        // with a spinner if it is slow.
        cx.spawn(async move |t, cx| {
            cx.background_executor().timer(olo_preview::SLOW).await;
            let _ = t.update(cx, |t, cx| {
                if t.quicklook.as_ref().is_some_and(|x| x == &q) && q.read(cx).win.is_none() {
                    t.open_quick_look(&q, cx);
                }
            });
        })
        .detach();
    }

    fn open_quick_look(&mut self, q: &Entity<QuickLook>, cx: &mut Context<Self>) {
        let source = q.read(cx).source;
        // Its size for what it shows, on the screen of the window it
        // shows the selection of.
        let screen = self.shown(source).and_then(|s| s.handle.update(cx, |_, window, cx| quicklook::screen(window, cx)).ok().flatten());
        let screen = screen.or_else(|| cx.primary_display().map(|d| (f32::from(d.bounds().size.width) as u32, f32::from(d.bounds().size.height) as u32)));
        let size = q.read(cx).ideal_size(screen, cx);
        let title = q.read(cx).entry.name.clone();
        let content = q.clone();
        let Some(handle) = olo_ui::open_window(cx, quicklook::window_options(title, size), move |_, _| content) else { return };
        let key = q.entity_id();
        q.update(cx, |q, _| q.win = Some(handle));
        self.register(key, handle.into(), Win::Preview, cx);
    }

    /// Quick Look's preview came: its window opens, or takes its size.
    pub fn quick_look_shown(&mut self, q: &Entity<QuickLook>, cx: &mut Context<Self>) {
        if self.quicklook.as_ref() != Some(q) {
            return;
        }
        match q.read(cx).win {
            None => self.open_quick_look(q, cx),
            Some(win) => {
                let q = q.clone();
                let _ = win.update(cx, |_, window, cx| {
                    if !window.is_fullscreen() {
                        let (w, h) = q.read(cx).ideal_size(quicklook::screen(window, cx), cx);
                        window.resize(quicklook::bounds_size(w, h));
                    }
                });
            }
        }
    }

    fn close_quick_look(&mut self, cx: &mut Context<Self>) {
        let Some(q) = self.quicklook.take() else { return };
        q.update(cx, |q, cx| q.stop(cx));
        let (win, source) = (q.read(cx).win, q.read(cx).source);
        if let Some(win) = win {
            self.windows.retain(|s| s.key != q.entity_id());
            cx.close_window(win);
        }
        self.activate(source, cx);
    }

    /// While Quick Look shows a window's selection, follow it as it moves.
    fn follow_selection(&mut self, key: EntityId, cx: &mut Context<Self>) {
        let Some(q) = &self.quicklook else { return };
        if q.read(cx).source != key {
            return;
        }
        let shown = q.read(cx).entry.path.clone();
        match self.selected_one(key, cx) {
            Some(e) if e.path != shown => self.show_quick_look(key, e, cx),
            _ => {}
        }
    }

    pub fn quick_look_action(&mut self, a: quicklook::Action, cx: &mut Context<Self>) {
        let Some(q) = self.quicklook.clone() else { return };
        match a {
            quicklook::Action::Close => self.close_quick_look(cx),
            quicklook::Action::Open => {
                let e = q.read(cx).entry.clone();
                self.close_quick_look(cx);
                self.open_entry(&e, cx);
            }
            quicklook::Action::PopOut => {
                let e = q.read(cx).entry.clone();
                self.close_quick_look(cx);
                fs::open_in_viewer(&e.path);
            }
            quicklook::Action::OpenWith(cmd) => {
                let e = q.read(cx).entry.clone();
                self.close_quick_look(cx);
                self.open_with(&cmd, vec![e], cx);
            }
            quicklook::Action::Step(d) => {
                let source = q.read(cx).source;
                let next = match self.shown(source).map(|s| &s.win) {
                    Some(Win::Folder(f)) => f.update(cx, |f, cx| {
                        let e = f.step_selection(d);
                        if let Some(e) = &e {
                            let row = f.current().0.iter().position(|x| x.path == e.path);
                            f.show.extend(row.map(|r| (f.active, r)));
                        }
                        cx.notify();
                        e
                    }),
                    Some(Win::Find(f)) => f.update(cx, |f, cx| f.step_selection(d, cx)),
                    _ => None,
                };
                if let Some(e) = next {
                    self.show_quick_look(source, e, cx);
                }
            }
        }
    }

    // ---- drops, pastes and jobs -------------------------------------------

    /// Files or text dropped on a folder, the desktop or the Trash.
    fn drop_on(&mut self, key: EntityId, drop: Dropped, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = DropTarget::parse(&drop.target) else { return };
        let paths = drop.paths();
        let desk = fs::home().join("Desktop");
        let on_desktop = self.desktop().filter(|d| d.entity_id() == key);
        let dest = match target {
            DropTarget::Trash => {
                let trash = fs::trash_files();
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !p.starts_with(&trash)).collect();
                if !paths.is_empty() {
                    self.start_job(ops::Kind::Trash, paths, None, Some(key), cx);
                }
                return;
            }
            DropTarget::Place => {
                if let Some(d) = on_desktop {
                    d.update(cx, |d, cx| {
                        d.place_dragged(drop.position);
                        cx.notify();
                    });
                }
                return;
            }
            DropTarget::Folder(dest) => dest,
        };
        let wants_link = drop.types.iter().any(|t| t == olo_ui::dnd::LINK_TYPE);
        // An application dragged out of Deskbar: a link to it, named as
        // the application is.
        if wants_link && drop.mime == "text/uri-list" && drop.action != DragAction::Ask {
            let mut names = Vec::new();
            for p in &paths {
                match fs::app_link_into(p, &dest) {
                    Ok(to) => names.extend(to.file_name().map(|n| n.to_string_lossy().into_owned())),
                    Err(e) => {
                        self.error(format!("Couldn't create a link: {e}"), cx);
                        break;
                    }
                }
            }
            if let Some(d) = &on_desktop
                && dest == desk
            {
                d.update(cx, |d, _| d.place_new(&names, drop.position));
            }
            self.reload_all(cx);
            return;
        }
        // Dropped on the desktop: they go where they were let go.
        if let Some(d) = &on_desktop
            && dest == desk
            && drop.action != DragAction::Ask
        {
            let names: Vec<String> = paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            d.update(cx, |d, _| d.place_new(&names, drop.position));
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
            match saved {
                Some(Ok(_)) => self.reload_all(cx),
                Some(Err(e)) => self.error(format!("Couldn't save what was dropped: {e}"), cx),
                None => {}
            }
            return;
        }
        // Nothing goes into itself.
        if let Some(p) = paths.iter().find(|p| p.is_dir() && dest.starts_with(p)) {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.error(format!("“{name}” can't go inside itself."), cx);
            return;
        }
        match drop.action {
            DragAction::Ask => {
                // The menu for a secondary-button drop, where it fell.
                let same = paths.iter().all(|p| p.parent() == Some(dest.as_path()));
                let menu = Menu::new("").item("Move Here", "move").enabled(!same).item("Copy Here", "copy").item("Create Link Here", "link").separator().item("Cancel", "cancel");
                let me = cx.weak_entity();
                menu::context_menu(window, cx, menu, drop.position, move |cmd, _, cx| {
                    let Some(t) = me.upgrade() else { return };
                    let (paths, dest) = (paths.clone(), dest.clone());
                    t.update(cx, |t, cx| match cmd {
                        "move" => t.start_job(ops::Kind::Move, paths, Some(dest), Some(key), cx),
                        "copy" => t.start_job(ops::Kind::Copy, paths, Some(dest), Some(key), cx),
                        "link" => {
                            for p in &paths {
                                if let Err(e) = fs::link_into(p, &dest) {
                                    t.error(format!("Couldn't create a link: {e}"), cx);
                                    break;
                                }
                            }
                            t.reload_all(cx);
                        }
                        _ => {}
                    });
                });
            }
            DragAction::Move => {
                // Already there: nothing to move.
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.parent() != Some(dest.as_path())).collect();
                if !paths.is_empty() {
                    self.start_job(ops::Kind::Move, paths, Some(dest), Some(key), cx);
                }
            }
            DragAction::Copy => self.start_job(ops::Kind::Copy, paths, Some(dest), Some(key), cx),
        }
    }

    /// What a Paste found on the clipboard: files (copied, or cut to move
    /// them), a picture (a file of it), or text (a clipping, as a drop of
    /// text makes).
    fn pasted(&mut self, key: EntityId, dir: &Path, mime: &str, data: &[u8], cx: &mut Context<Self>) {
        if let Some((paths, cut)) = olo_kit_base::clipboard::copied_files(mime, data) {
            // Nothing goes into itself.
            if let Some(p) = paths.iter().find(|p| p.is_dir() && dir.starts_with(p)) {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                return self.error(format!("“{name}” can't go inside itself."), cx);
            }
            if cut {
                // Moved: the clipboard has them where they are now, copied.
                let moved: Vec<PathBuf> = paths.iter().filter_map(|p| Some(dir.join(p.file_name()?))).collect();
                copy_clip(olo_kit_base::clipboard::files(&moved, false), cx);
            }
            let kind = if cut { ops::Kind::Move } else { ops::Kind::Copy };
            return self.start_job(kind, paths, Some(dir.to_owned()), Some(key), cx);
        }
        let saved = if let Some(kind) = mime.strip_prefix("image/") {
            fs::pasted_picture(dir, kind, data)
        } else if mime.starts_with("text/") || mime == "UTF8_STRING" {
            let text = String::from_utf8_lossy(data);
            if text.trim().is_empty() {
                return;
            }
            fs::clipping(dir, &text)
        } else {
            return;
        };
        match saved {
            Ok(_) => self.reload_all(cx),
            Err(e) => self.error(format!("Couldn't paste: {e}"), cx),
        }
    }

    /// Run a file operation in the background. Short ones finish without a
    /// trace; longer ones show in the Tracker Status window.
    fn start_job(&mut self, kind: ops::Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, origin: Option<EntityId>, cx: &mut Context<Self>) {
        let id = self.next_job;
        self.next_job += 1;
        let proxy = olo_ui::proxy(cx);
        let job = ops::start(id, kind, sources, dest, move |outcome| proxy.send(Msg::JobDone(id, outcome)));
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            std::thread::sleep(STATUS_DELAY);
            proxy.send(Msg::JobSlow(id));
        });
        self.jobs.push((job, origin));
        self.fit_status(cx);
    }

    pub fn stop_job(&mut self, id: u64) {
        if let Some((job, _)) = self.jobs.iter().find(|(j, _)| j.id == id) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The status window as tall as its rows.
    fn fit_status(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.status.and_then(|k| self.shown(k)) else { return };
        let h = panels::status_height(self.jobs.len());
        let _ = s.handle.update(cx, |_, window, _| window.resize(gpui::size(gpui::px(panels::STATUS_WIDTH), gpui::px(h))));
    }

    fn job_done(&mut self, id: u64, outcome: ops::Outcome, cx: &mut Context<Self>) {
        let Some(i) = self.jobs.iter().position(|(j, _)| j.id == id) else { return };
        let (job, origin) = self.jobs.remove(i);
        self.reload_all(cx);
        // New items come up selected, as after a drop.
        if let Some(f) = origin.and_then(|k| self.folder(k))
            && !outcome.created.is_empty()
        {
            let created = outcome.created.clone();
            f.update(cx, |f, cx| {
                f.select_paths(&created);
                cx.notify();
            });
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
            self.error(format!("An error occurred while {}: {e}", what.unwrap_or_default()), cx);
        }
        if self.jobs.is_empty() {
            if let Some(k) = self.status.take()
                && let Some(i) = self.windows.iter().position(|s| s.key == k)
            {
                let s = self.windows.remove(i);
                cx.close_window(s.handle);
            }
        } else {
            self.fit_status(cx);
        }
    }

    fn show_status(&mut self, cx: &mut Context<Self>) {
        let app = cx.weak_entity();
        let status = cx.new(|cx| StatusWin::new(app, cx));
        let key = status.entity_id();
        let content = status.clone();
        let h = panels::status_height(self.jobs.len());
        let Some(handle) = olo_ui::open_window(cx, WindowOptions::new("Tracker Status", panels::STATUS_WIDTH, h).min_size(300.0, 60.0).floating(), move |_, _| content) else { return };
        self.status = Some(key);
        self.register(key, handle.into(), Win::Status, cx);
    }

    // ---- messages ---------------------------------------------------------

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args, cx),
            Err(m) => m,
        };
        let message = match message.downcast::<NoteEvent>() {
            Ok(e) => {
                match *e {
                    NoteEvent::Action { id, token, .. } => {
                        if let Some(dir) = self.note_dirs.remove(&id) {
                            let path = dir.canonicalize().unwrap_or(dir);
                            match self.folder_window(&path, cx).and_then(|k| self.shown(k)) {
                                Some(s) => {
                                    let _ = s.handle.update(cx, |_, window, _| match &token {
                                        Some(t) => window.activate_window_with_token(t),
                                        None => window.activate_window(),
                                    });
                                }
                                None => {
                                    self.open_folder(&path, cx);
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
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Settings(s) => self.default_view = s.default_view,
            Msg::Thumb => cx.refresh_windows(),
            Msg::DirSize(key, b, n) => {
                if let Some(Win::Info(i)) = self.shown(key).map(|s| &s.win) {
                    i.update(cx, |i, cx| {
                        i.size = Some((b, n));
                        cx.notify();
                    });
                }
            }
            Msg::JobSlow(id) => {
                if self.jobs.iter().any(|(j, _)| j.id == id) && self.status.is_none() {
                    self.show_status(cx);
                }
            }
            Msg::JobDone(id, outcome) => self.job_done(id, outcome, cx),
            Msg::Changed(changes) => self.changed(changes, cx),
            Msg::Edited(path, event) => {
                let Some(q) = self.quicklook.clone() else { return };
                if q.update(cx, |q, cx| q.edited(&path, event, cx)) {
                    self.quick_look_shown(&q, cx);
                }
            }
            Msg::Listed { win, path, generation, result } => {
                if let Some(f) = self.folder(win) {
                    f.update(cx, |f, cx| {
                        f.listed(&path, generation, result);
                        cx.notify();
                    });
                }
            }
            Msg::Found(win, generation, hits, done) => {
                if let Some(Win::Find(f)) = self.shown(win).map(|s| &s.win) {
                    f.update(cx, |f, cx| f.found(generation, hits, done, cx));
                }
            }
        }
    }

    /// Changes seen on disk, to the windows showing them.
    fn changed(&mut self, changes: Vec<watch::Change>, cx: &mut Context<Self>) {
        // An item's new state on disk (None: gone), by its folder's changes.
        let now = |path: &Path| {
            changes.iter().find_map(|c| match c {
                watch::Change::Item { path: p, entry, .. } if p == path => Some(entry.clone()),
                watch::Change::Rescan(d) if path.parent() == Some(d.as_path()) => Some(fs::entry(path)),
                _ => None,
            })
        };
        let mut gone = Vec::new();
        for s in &self.windows {
            match &s.win {
                Win::Info(i) => match now(&i.read(cx).entry.path) {
                    // Gone: its Info goes too, as on BeOS.
                    Some(None) => gone.push(s.key),
                    Some(Some(e)) => i.update(cx, |i, cx| {
                        i.entry = e;
                        cx.notify();
                    }),
                    None => {}
                },
                Win::Find(f) => f.update(cx, |f, cx| f.changed(now, cx)),
                _ => {}
            }
        }
        for key in gone {
            if let Some(i) = self.windows.iter().position(|s| s.key == key) {
                let s = self.windows.remove(i);
                cx.close_window(s.handle);
            }
        }
        for s in &self.windows {
            match &s.win {
                Win::Folder(f) => {
                    let touched = {
                        let dirs: Vec<PathBuf> = f.read(cx).watched().map(Path::to_owned).collect();
                        changes.iter().any(|c| match c {
                            watch::Change::Item { dir, .. } | watch::Change::Rescan(dir) => dirs.contains(dir),
                            watch::Change::Mounts => false,
                        })
                    };
                    if touched {
                        f.update(cx, |f, cx| {
                            f.apply(&changes);
                            cx.notify();
                        });
                    }
                }
                Win::Desktop(d) => d.update(cx, |d, cx| {
                    if d.apply(&changes) {
                        cx.notify();
                    }
                }),
                _ => {}
            }
        }
        self.sync_watches(cx);
    }

    fn save_state(&self, cx: &App) -> Option<String> {
        let mut saved = Saved { hidden: self.hidden, folders: Vec::new(), find: 0, desktop_selected: Vec::new() };
        for s in &self.windows {
            match &s.win {
                Win::Folder(f) => {
                    let f = f.read(cx);
                    saved.folders.push(SavedFolder {
                        path: f.path.clone(),
                        size: f.size,
                        view: f.view,
                        sort: f.sort.unwrap_or((SortKey::Name, true)),
                        own_sort: f.sort.is_some(),
                        selected: f.selected_paths(),
                    });
                }
                Win::Find(_) => saved.find += 1,
                Win::Desktop(d) => saved.desktop_selected = d.read(cx).selected_paths(),
                _ => {}
            }
        }
        serde_json::to_string(&saved).ok()
    }
}

/// Copies `clip` (each form's type and data) to the clipboard.
fn copy_clip(clip: olo_kit_base::clipboard::Clip, cx: &mut App) {
    let entries = clip.into_iter().map(|(mime, bytes)| gpui::ClipboardEntry::Data(gpui::ClipboardData { mime, bytes })).collect();
    cx.write_to_clipboard(gpui::ClipboardItem::from_entries(entries));
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_preview::audio::set_identity("Tracker", "olo-tracker");
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        workspaces::follow(cx);
        thumbs::init(olo_ui::proxy(cx));
        let proxy = olo_ui::proxy(cx);
        let watcher = watch::Watcher::new(move |changes| proxy.send(Msg::Changed(changes)));
        let proxy = olo_ui::proxy(cx);
        olo_kit_base::settings::watch(move |s| {
            if let Ok(mut u) = fs::UNGROUPED.lock() {
                *u = s.tiling.ungrouped_apps.clone();
            }
            proxy.send(Msg::Settings(s.tracker))
        });
        let desktop = args.iter().any(|a| a == "--desktop");
        let app = cx.new(|cx| Tracker {
            windows: Vec::new(),
            hidden: false,
            // Folders opened at startup can't wait for the watcher's first
            // read.
            default_view: olo_kit_base::settings::current().map(|s| s.tracker.default_view).unwrap_or_default(),
            jobs: Vec::new(),
            next_job: 1,
            status: None,
            notes: Notifier::new(olo_ui::proxy(cx), "Tracker", "olo-tracker"),
            note_dirs: HashMap::new(),
            watcher,
            watched: BTreeSet::new(),
            quicklook: None,
            preview_generation: 0,
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |t, cx| t.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state(cx));
        // A file operation under way: the new version waits for it.
        let busy = app.clone();
        olo_ui::upgrade_waits_while(cx, move |cx| !busy.read(cx).jobs.is_empty());
        if desktop {
            olo_ui::stay_resident(cx);
            app.update(cx, |t, cx| {
                let me = cx.weak_entity();
                let d = cx.new(|cx| Desktop::new(me, cx));
                let key = d.entity_id();
                let content = d.clone();
                if let Some(handle) = olo_ui::open_window(cx, desktop::options(), move |_, _| content) {
                    t.register(key, handle.into(), Win::Desktop(d), cx);
                }
            });
            // Other programs' "show in folder", while the desktop's Tracker
            // runs (`filemanager`).
            let proxy = olo_ui::proxy(cx);
            let _ = std::thread::Builder::new().name("tracker-filemanager".into()).spawn(move || {
                let reveal = move |paths: Vec<PathBuf>| {
                    let args = paths.into_iter().flat_map(|p| ["--reveal".to_owned(), p.display().to_string()]).collect();
                    proxy.send(Launch { args, cwd: None, taken: None });
                };
                match filemanager::serve(reveal) {
                    Ok(_conn) => loop {
                        std::thread::park();
                    },
                    Err(e) => tracing::info!("tracker: not serving {}: {e}", filemanager::NAME),
                }
            });
        }
        let restored = olo_ui::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        app.update(cx, |t, cx| match restored {
            Some(saved) => t.restore(saved, cx),
            None => t.handle_args(&args, cx),
        });
    });
}
