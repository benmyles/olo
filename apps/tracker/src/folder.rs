//! A folder window: its menu bar, the folder shown as icons, mini icons,
//! a list or columns, and the count along the bottom. Its window's own
//! view, on a looper of its own: it lists its folder (on workers),
//! selects, takes the keyboard, drags out, takes what's dropped on it,
//! edits a name; what opens other windows or changes files it asks of
//! the app ([`crate::ToHub`]), which tells it what happened
//! ([`FolderMsg`]).

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Cx, DropAccept, DropOffer, Dropped, Element, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, MenuBar, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, Pixels, Point, ScrollHandle, SharedString, Styled as _, View, WindowCx, canvas, div, menu, metrics as m, point, px, size,
    style::{self, Align},
    theme,
    widgets::{
        SCROLL_BAR, ScrollView,
        list::{Cell, Column, ColumnListView, ListModel},
        open_alert,
    },
};
use olo_kit_base::list::{ListState, Mods, SelectionBand, TypeSelect};

use crate::{
    SavedFolder, ToHub, ViewMode, WinInfo, columns,
    fs::{self, Entry},
    items::{self, DropTarget, Look, Rename, Spring},
    model::{Loader, Order, Pane, SortKey},
    ops, to_hub, undo, watch,
};

/// How long after a click on a selected item's name editing starts (a
/// double-click within it opens the item instead), as on BeOS.
const EDIT_DELAY: Duration = Duration::from_millis(600);
/// How far a press moves before it's a drag.
const DRAG_SLOP: f32 = 3.0;

/// What a folder window is told: by its workers, by the app, by itself
/// later.
pub enum FolderMsg {
    /// A folder listed (asked as request `generation`).
    Listed {
        path: PathBuf,
        generation: u64,
        result: Result<Vec<Entry>, String>,
    },
    /// Changes seen on disk.
    Changed(Arc<Vec<watch::Change>>),
    /// List everything again (hidden files shown or not, now).
    Reload(bool),
    /// These, selected (as their listing brings them in).
    Select(Vec<PathBuf>),
    /// Opened from this folder's window: Back goes there.
    CameFrom(PathBuf),
    /// Something it shows that isn't its own changed (Undo's name, the
    /// places, a backup's marks, a thumbnail): shown again.
    Refresh,
    /// Quick Look's arrows: the selection a step on, and shown there.
    Step(isize),
    /// What a Paste found on the clipboard, for the folder asked in.
    Pasted(PathBuf, Option<(String, Vec<u8>)>),
    /// A drag out of it is over.
    DragOver,
    /// Forward (its tab chosen from a notification), with a token.
    Activate(Option<String>),
    Close,
}

pub struct FolderWin {
    pub path: PathBuf,
    /// The folders it showed, the one it was opened from first, and which
    /// it shows now (Go ▸ Back and Forward, the mouse's back and forward
    /// buttons: in this window, as Haiku's Tracker navigates in one).
    pub history: Vec<PathBuf>,
    pub at: usize,
    pub entries: Vec<Entry>,
    pub view: ViewMode,
    pub list: ListState,
    /// Its folder's order (`sorts`: the one chosen for it, else Olo's).
    pub sort: Order,
    /// Changed on disk while being listed: list again when it lands.
    pub dirty: bool,
    pub error: Option<String>,
    /// The item whose name is being edited in place.
    pub editing: Option<Rename>,
    /// A click on an already-selected item's name starts editing it after
    /// a moment, unless a double-click (open) follows.
    pub pending_edit: Option<(PathBuf, Instant)>,
    /// Column view: the columns after the root, which column has the
    /// keyboard (0 is the root).
    pub panes: Vec<Pane>,
    pub active: usize,
    /// What the columns' scroll last brought into view (active column,
    /// columns, preview): scrolling follows the keyboard only when that
    /// changes.
    pub col_focus: Option<(usize, usize, bool)>,
    /// Whether hidden files are listed (Tracker-wide; kept for new columns).
    pub hidden: bool,
    pub loader: Loader,
    /// Listing requests: the last one made, the one the root waits for,
    /// and how many items the root's listing has found so far.
    pub generation: u64,
    pub root_generation: u64,
    pub loading: bool,
    pub listed: Arc<AtomicU64>,
    /// Items to select once their folder's listing brings them in, by
    /// folder.
    pub pending_select: Vec<(PathBuf, Vec<PathBuf>)>,
    /// Selections made in code, to scroll into view: (column, row).
    pub show: Vec<(usize, usize)>,

    /// The app's number for this window.
    pub id: u64,
    /// The window's surface, as last drawn (which workspace it's on),
    /// and its size (kept for an upgrade).
    pub surface: Option<u32>,
    pub size: (u32, u32),
    pub focus: FocusHandle,
    scroll: ScrollHandle,
    geo: RefCell<IconGeo>,
    press: Option<Press>,
    band: Option<(Point<Pixels>, SelectionBand)>,
    /// Where the band reaches now (in the window).
    band_at: Point<Pixels>,
    pub typing: TypeSelect,
    pub spring: Spring,
    /// Where what's dragged over it would go: the column and item it
    /// would be dropped on (None: into the column's folder), if it's
    /// taken there.
    pub dropping: Option<(usize, Option<usize>)>,
    list_model: ListModel,
    pub cols: columns::Cols,
    path_said: bool,
    /// The keyboard goes back to the view (a name's edit ended).
    focus_after_edit: bool,
    /// The view the keyboard was last given to.
    focused_view: Option<ViewMode>,
    /// Bumped whenever the items change (`names` follows it).
    pub stamp: u64,
    names: (u64, Rc<[SharedString]>),
    polling: bool,
    /// The free space under the count, and when it was read.
    free: RefCell<Option<(Instant, PathBuf, Option<u64>)>>,
    /// What the app was last told of this window.
    told: Option<WinInfo>,
}

/// The icon view as last laid out and drawn: where its content is (in
/// the window: it scrolls), what of it shows, and its grid.
#[derive(Default, Clone, Copy)]
struct IconGeo {
    origin: Point<Pixels>,
    view: Bounds<Pixels>,
    cols: usize,
    cell: (f32, f32),
    inset: f32,
}

impl IconGeo {
    /// Item `i`'s cell, in the window.
    fn cell_at(&self, i: usize) -> Bounds<Pixels> {
        let (c, r) = (i % self.cols.max(1), i / self.cols.max(1));
        Bounds { origin: point(self.origin.x + px(self.inset + c as f32 * self.cell.0), self.origin.y + px(self.inset + r as f32 * self.cell.1)), size: size(px(self.cell.0), px(self.cell.1)) }
    }

    /// The item whose cell `p` is in, of `n`.
    fn index_at(&self, p: Point<Pixels>, n: usize) -> Option<usize> {
        let (x, y) = (f32::from(p.x - self.origin.x) - self.inset, f32::from(p.y - self.origin.y) - self.inset);
        if x < 0.0 || y < 0.0 || self.cols == 0 {
            return None;
        }
        let (c, r) = ((x / self.cell.0.max(1.0)) as usize, (y / self.cell.1.max(1.0)) as usize);
        (c < self.cols).then_some(r * self.cols + c).filter(|i| *i < n)
    }
}

/// A press in the icon view: where (in the window and the content), with
/// what, on which item, and what it has become.
struct Press {
    at: Point<Pixels>,
    button: MouseButton,
    item: Option<usize>,
    /// The item was selected before (a click then selects it alone, on
    /// release, so a drag takes the whole selection).
    was_selected: bool,
    on_label: bool,
    mods: Modifiers,
    dragging: bool,
}

pub fn mods(m: &Modifiers) -> Mods {
    Mods { command: m.control || m.platform, shift: m.shift }
}

impl FolderWin {
    pub fn new(id: u64, path: PathBuf, view: ViewMode, hidden: bool, cx: &mut Cx<Self>) -> Self {
        let loader = Loader { to: cx.messenger() };
        let list_model = ListModel::new(true);
        let sort = crate::sorts::of(&path);
        let mut w = FolderWin {
            history: vec![path.clone()],
            at: 0,
            path,
            entries: Vec::new(),
            view,
            list: ListState { multiple: true, ..Default::default() },
            sort,
            dirty: false,
            error: None,
            editing: None,
            pending_edit: None,
            panes: Vec::new(),
            active: 0,
            col_focus: None,
            hidden,
            loader,
            generation: 0,
            root_generation: 0,
            loading: true,
            listed: Arc::default(),
            pending_select: Vec::new(),
            show: Vec::new(),
            id,
            surface: None,
            size: (560, 380),
            focus: FocusHandle::new(),
            scroll: ScrollHandle::new(),
            geo: RefCell::default(),
            press: None,
            band: None,
            band_at: Point::default(),
            typing: TypeSelect::default(),
            spring: Spring::default(),
            dropping: None,
            list_model,
            cols: columns::Cols::default(),
            path_said: false,
            focus_after_edit: false,
            focused_view: None,
            stamp: 0,
            names: (u64::MAX, Rc::from(Vec::new())),
            polling: false,
            free: RefCell::new(None),
            told: None,
        };
        w.reload(hidden);
        w
    }

    /// Opened from `from`'s window: Back goes there.
    pub fn came_from(&mut self, from: PathBuf) {
        if self.history.len() == 1 && from != self.path {
            self.history.insert(0, from);
            self.at = 1;
        }
    }

    pub fn can_go(&self, by: isize) -> bool {
        self.at.checked_add_signed(by).is_some_and(|i| i < self.history.len())
    }

    /// Where it is: its folder, or in column view its deepest column's.
    pub fn location(&self) -> PathBuf {
        self.panes.last().map_or_else(|| self.path.clone(), |p| p.path.clone())
    }

    /// A move into another folder by the person (in column view, a column
    /// opened or closed) kept in its history, the places after the one it
    /// was at forgotten, as a browser's. (Ben, 2026-11-07: "in column mode
    /// and go between columns/files the back/forward navigation doesn't
    /// seem to work": only the folder it was opened from was kept.)
    pub fn record_location(&mut self) {
        const KEPT: usize = 100;
        let here = self.location();
        if self.history.get(self.at) == Some(&here) {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(here);
        if self.history.len() > KEPT {
            self.history.remove(0);
        }
        self.at = self.history.len() - 1;
    }

    /// Back (`by` −1) or Forward (+1) through the places it was: in this
    /// window, the folder it comes back out of selected. In column view,
    /// a place inside its folder is the columns opened down to it.
    pub fn go(&mut self, by: isize, cx: &mut Cx<Self>) {
        let Some(to) = self.at.checked_add_signed(by).filter(|&i| i < self.history.len()) else { return };
        let (from, path) = (self.location(), self.history[to].clone());
        self.at = to;
        if self.view == ViewMode::Columns && path.starts_with(&self.path) {
            self.show_in_columns(&path, &from);
            cx.invalidate();
            return;
        }
        self.show_folder(path.clone(), cx);
        // Back out of a folder inside it: that folder selected.
        if let Ok(rest) = from.strip_prefix(&path)
            && let Some(first) = rest.components().next()
        {
            self.pending_select.push((path.clone(), vec![path.join(first)]));
        }
    }

    /// Column view: the columns opened down to `dir` (inside its folder),
    /// each folder on the way selected in the column before, nothing in
    /// `dir`'s. Columns already showing the way stay; each other opens as
    /// the one before it is listed (`pending_select`).
    fn show_in_columns(&mut self, dir: &std::path::Path, from: &std::path::Path) {
        let Ok(rest) = dir.strip_prefix(&self.path) else { return };
        let mut way = vec![self.path.clone()];
        for c in rest.components() {
            let next = way[way.len() - 1].join(c);
            way.push(next);
        }
        let kept = self.panes.iter().zip(way.iter().skip(1)).take_while(|(p, d)| p.path == **d).count();
        self.panes.truncate(kept.min(way.len() - 1));
        self.pending_select.clear();
        for w in way.windows(2) {
            self.pending_select.push((w[0].clone(), vec![w[1].clone()]));
        }
        // Nothing selected in `dir`'s own column, so no column after it:
        // the folder come back out of, selected, would open again (a
        // folder chosen in a column opens the next), as Finder's columns
        // don't.
        let _ = from;
        match way.len() {
            1 => self.list.selected.clear(),
            n => {
                if let Some(p) = self.panes.get_mut(n - 2) {
                    p.list.selected.clear();
                }
            }
        }
        self.active = way.len() - 1;
        // Columns listed already take their selection now (and open the
        // next); the rest as their listings land.
        let mut k = 0;
        while k <= self.panes.len() {
            self.select_pending(k, false);
            k += 1;
        }
    }

    /// Show `path` in this window: listed afresh, sorted as it's sorted,
    /// nothing selected, the window titled for it.
    fn show_folder(&mut self, path: PathBuf, cx: &mut Cx<Self>) {
        let places = path.clone();
        std::thread::spawn(move || olo_kit_base::places::visited(&places));
        self.sort = crate::sorts::of(&path);
        self.path = path;
        self.entries.clear();
        self.list = ListState { multiple: true, ..Default::default() };
        self.list_model.state_mut().selected.clear();
        self.error = None;
        self.editing = None;
        self.pending_edit = None;
        self.panes.clear();
        self.active = 0;
        self.col_focus = None;
        self.pending_select.clear();
        self.show.clear();
        self.scroll.set_offset(cx, Point::default());
        self.path_said = false;
        self.request(None);
        cx.invalidate();
    }

    /// Opens `e`: a folder's window (whose Back comes back here), a
    /// file by what opens its kind.
    pub fn open(&self, e: &Entry) {
        if e.is_dir {
            to_hub(ToHub::Open { path: e.path.clone(), from: Some(self.path.clone()) });
        } else {
            fs::open(e);
        }
    }

    fn error(&self, message: String, cx: &mut Cx<Self>) {
        let _ = open_alert(cx, "Tracker", "about", &message, &["OK"], |_, _| {});
    }

    /// Ends editing a name: renamed to what's in the field (`commit`), or
    /// left as it was.
    pub fn end_rename(&mut self, commit: bool, cx: &mut Cx<Self>) {
        let Some(r) = self.editing.take() else { return };
        if commit {
            let name = r.text(cx);
            to_hub(ToHub::Rename { win: Some(self.id), path: r.path, name });
        }
        // The keyboard back to the view.
        self.focus_after_edit = true;
        cx.invalidate();
    }

    /// The menus' and the views' items as the window shows them now.
    pub fn item_menu(&self) -> Menu {
        let one = self.current().1.selected.len() == 1;
        let in_trash = self.current_dir().starts_with(fs::trash_files());
        let mut menu = Menu::new("").item("Open", "open").key("o").item("Quick Look", "quick-look").key("y").enabled(one);
        // A running app of ours can open another window.
        if one && self.selected_entries().first().and_then(fs::running_app).is_some() {
            menu = menu.item("New Window", "new-window");
        }
        // An application's windows: grouped in one frame, or each its own.
        if one {
            menu = fs::grouping_item(menu, self.selected_entries().first());
        }
        menu = menu.item("Get Info", "info").key("i").item("Edit Name", "rename").key("e").enabled(one).item("Duplicate", "duplicate").key("d").item("Create Link", "link");
        // DiskBackup's versions of a file (Ben: "if you right click a file in
        // tracker it should show older versions that you can restore").
        if crate::backups::on() {
            let file = one && self.selected_entries().first().is_some_and(|e| !e.is_dir);
            menu = menu.item("Older Versions…", "older-versions").enabled(file);
        }
        // A folder opens in Terminal itself.
        if self.selected_entries().iter().all(|e| e.is_dir) {
            menu = menu.item("Open in Terminal", "terminal-selected");
        }
        if let Some(open_with) = open_with_menu(&self.selected_entries()) {
            menu = menu.submenu(open_with);
        }
        if let Some(play) = play_on_menu(&self.selected_entries()) {
            menu = menu.submenu(play);
        }
        menu = menu.separator().item("Cut", "cut").key("x").item("Copy", "copy").key("c").separator();
        if !in_trash {
            menu = menu.item("Move to Trash", "trash").key("t");
        } else if self.current_dir() == fs::trash_files() {
            // Items at the top of the Trash go back where they came from.
            menu = menu.item("Restore", "restore");
        }
        menu
    }

    pub fn background_menu(&self, can_paste: bool) -> Menu {
        let in_trash = self.current_dir().starts_with(fs::trash_files());
        Menu::new("")
            .item("New Folder", "new-folder")
            .shift_key("n")
            .enabled(!in_trash)
            .item("Paste", "paste")
            .key("v")
            .enabled(can_paste && !in_trash)
            .separator()
            .item("Icon View", "view-icons")
            .key("1")
            .checked(self.view == ViewMode::Icons)
            .item("Mini Icon View", "view-mini")
            .key("2")
            .checked(self.view == ViewMode::MiniIcons)
            .item("List View", "view-list")
            .key("3")
            .checked(self.view == ViewMode::List)
            .item("Column View", "view-columns")
            .key("4")
            .checked(self.view == ViewMode::Columns)
            .separator()
            .item("Get Info", "info")
            .key("i")
            .item("Open in Terminal", "terminal")
            .item("Open Parent", "parent")
            .key("up")
            .enabled(self.path.parent().is_some())
    }

    /// The menu bar's menus.
    /// `undo`: Edit ▸ Undo's and Redo's names, and whether there's a step
    /// for each (the app's history).
    fn menus(&self, can_paste: bool, places: &olo_kit_base::places::Places, undo: &[(String, bool); 2]) -> Vec<Menu> {
        let current = self.current().1;
        let has_sel = !current.selected.is_empty();
        let in_trash = self.current_dir().starts_with(fs::trash_files());
        // ⌘N a new window, ⇧⌘N a new folder, as Finder has them (Ben,
        // 2026-11-04: "pressing super+n should open a new tracker window").
        let mut file = Menu::new("File")
            .item("New Window", "new")
            .key("n")
            .item("New Folder", "new-folder")
            .shift_key("n")
            .enabled(!in_trash)
            .item("Find…", "find")
            .key("f")
            .separator()
            .item("Open", "open")
            .key("o")
            .enabled(has_sel)
            .item("Open Parent", "parent")
            .key("up")
            .enabled(self.path.parent().is_some())
            .item("Get Info", "info")
            .key("i")
            .item("Open in Terminal", "terminal")
            .item("Quick Look", "quick-look")
            .key("y")
            .enabled(has_sel)
            .item("Edit Name", "rename")
            .key("e")
            .enabled(current.selected.len() == 1)
            .item("Duplicate", "duplicate")
            .key("d")
            .enabled(has_sel)
            .item("Create Link", "link")
            .enabled(has_sel)
            .separator()
            .item("Move to Trash", "trash")
            .key("t")
            .enabled(has_sel && !in_trash);
        if in_trash {
            file = file.item("Restore", "restore").enabled(has_sel && self.current_dir() == fs::trash_files()).item("Empty Trash", "empty-trash").enabled(!fs::trash_is_empty());
        }
        let file = file.separator().item("Close", "close").key("w").item("Quit", "quit").key("q");
        let edit = Menu::new("Edit")
            .item(undo[0].0.clone(), "undo")
            .key("z")
            .enabled(undo[0].1)
            .item(undo[1].0.clone(), "redo")
            .shift_key("z")
            .enabled(undo[1].1)
            .separator()
            .item("Cut", "cut")
            .key("x")
            .enabled(has_sel)
            .item("Copy", "copy")
            .key("c")
            .enabled(has_sel)
            .item("Paste", "paste")
            .key("v")
            .enabled(can_paste)
            .separator()
            .item("Select All", "select-all")
            .key("a")
            .item("Invert Selection", "invert")
            .key("s");
        let sort = |k: SortKey| self.sort.is_some_and(|(s, _)| s == k);
        let window = Menu::new("Window")
            .item("Icon View", "view-icons")
            .key("1")
            .checked(self.view == ViewMode::Icons)
            .item("Mini Icon View", "view-mini")
            .key("2")
            .checked(self.view == ViewMode::MiniIcons)
            .item("List View", "view-list")
            .key("3")
            .checked(self.view == ViewMode::List)
            .item("Column View", "view-columns")
            .key("4")
            .checked(self.view == ViewMode::Columns)
            .separator()
            .submenu(
                Menu::new("Sort By")
                    .item("Folders by Name, Files by Date", "sort-default")
                    .checked(self.sort.is_none())
                    .separator()
                    .item("Name", "sort-name")
                    .checked(sort(SortKey::Name))
                    .item("Size", "sort-size")
                    .checked(sort(SortKey::Size))
                    .item("Modified", "sort-modified")
                    .checked(sort(SortKey::Modified))
                    .item("Kind", "sort-kind")
                    .checked(sort(SortKey::Kind)),
            )
            .item("Show Hidden Files", "hidden")
            .checked(self.hidden);
        // The standard folders, favourites (this one added or taken away)
        // and recent folders, as every file panel has them.
        let here = self.current_dir();
        let mut favorites = Menu::new("Favorites");
        for d in places.favorites.iter() {
            favorites = favorites.item(olo_kit_base::places::label(d), format!("go-place:{}", d.display()));
        }
        if !places.favorites.is_empty() {
            favorites = favorites.separator();
        }
        let name = olo_kit_base::places::label(&here);
        favorites = if places.favorites.contains(&here) {
            favorites.item(format!("Remove “{name}” from Favorites"), "favorite-off")
        } else {
            favorites.item(format!("Add “{name}” to Favorites"), "favorite-on")
        };
        let mut go = Menu::new("Go")
            // (Their keys the profile's: `app_keys`' tracker.back and .forward.)
            .item("Back", "back")
            .enabled(self.can_go(-1))
            .item("Forward", "forward")
            .enabled(self.can_go(1))
            .separator()
            .item("Go to File or Folder…", "go-to")
            .shift_key("g")
            .separator();
        for (label, dir) in olo_kit_base::places::standard() {
            let cmd = if dir == fs::home() { "go-home".to_owned() } else { format!("go-place:{}", dir.display()) };
            go = go.item(label, cmd);
        }
        go = go.item(fs::volume_label(), "go-root").item("Trash", "go-trash").separator().submenu(favorites);
        let recent: Vec<&PathBuf> = places.recent.iter().filter(|r| **r != here).collect();
        if !recent.is_empty() {
            let mut sub = Menu::new("Recent Folders");
            for r in recent {
                sub = sub.item(olo_kit_base::places::label(r), format!("go-place:{}", r.display()));
            }
            go = go.submenu(sub);
        }
        vec![file, edit, window, go]
    }

    /// Shows a context menu at `at`: the items' if `on_item`, else the
    /// folder's.
    pub fn context_menu(&self, on_item: bool, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let can_paste = crate::can_paste();
        let menu = if on_item { self.item_menu() } else { self.background_menu(can_paste) };
        let me = cx.view_ref();
        menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |f, cx| f.run(&cmd, cx));
        });
    }

    // ---- keyboard --------------------------------------------------------

    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let k = &e.keystroke;
        if self.editing.is_some() {
            if k.key == "escape" {
                self.end_rename(false, cx);
                cx.stop_propagation();
            }
            return;
        }
        // Alt's are the menus' (Windows' Alt+↑, Alt+Enter).
        if k.modifiers.alt {
            return;
        }
        let command = k.modifiers.control || k.modifiers.platform;
        if command {
            if k.key == "down" {
                self.run("open", cx);
                cx.stop_propagation();
            }
            return;
        }
        // Space is Quick Look, unless it goes on with a name being typed.
        let typing = match self.view {
            ViewMode::List => self.list_model.typing(),
            _ => self.typing.typing(),
        };
        if k.key == "space" && !typing {
            self.run("quick-look", cx);
            cx.stop_propagation();
            return;
        }
        match k.key.as_str() {
            "delete" if plain_extra("trash", "delete") => {
                self.run("trash", cx);
                cx.stop_propagation();
                return;
            }
            "f2" if plain_extra("rename", "f2") => {
                self.run("rename", cx);
                cx.stop_propagation();
                return;
            }
            _ => {}
        }
        match self.view {
            // The list takes its own keys.
            ViewMode::List => {}
            ViewMode::Columns => {
                if self.column_key(e, cx) {
                    cx.stop_propagation();
                }
            }
            ViewMode::Icons | ViewMode::MiniIcons => {
                if self.icon_key(e, cx) {
                    cx.stop_propagation();
                }
            }
        }
    }

    /// The icon views' keys: the arrows, Page Up and Down, Home and End
    /// move (with ⇧, grow or shrink the selection from the anchor), Return
    /// opens, typing selects.
    fn icon_key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
        let n = self.entries.len();
        if n == 0 {
            return false;
        }
        let k = &e.keystroke;
        let (cols, rows_in_view) = {
            let g = self.geo.borrow();
            (g.cols.max(1), ((f32::from(self.scroll.bounds().size.height) - 2.0 * g.inset) / g.cell.1.max(1.0)).floor().max(1.0) as isize)
        };
        let page = rows_in_view * cols as isize;
        let list = &self.list;
        let to = match k.key.as_str() {
            "left" => list.step(-1, n),
            "right" => list.step(1, n),
            "up" => list.step(-(cols as isize), n),
            "down" => list.step(cols as isize, n),
            "pageup" => list.step(-page, n),
            "pagedown" => list.step(page, n),
            "home" => Some(0),
            "end" => Some(n - 1),
            "enter" => {
                self.run("open", cx);
                return true;
            }
            _ => None,
        };
        let (to, extend) = match to {
            Some(t) => (Some(t), k.modifiers.shift),
            None => match k.key_char.as_deref().filter(|c| !c.chars().any(char::is_control)) {
                Some(typed) => {
                    let entries = &self.entries;
                    (self.typing.typed(typed, n, self.list.cursor(), |i| entries[i].name.as_str()), false)
                }
                None => (None, false),
            },
        };
        let Some(to) = to else { return k.key_char.is_some() };
        self.list.move_to(to, extend);
        self.reveal_icon(to, 0.0, cx);
        cx.invalidate();
        true
    }

    /// Scrolls the icon view so item `i` shows, with `around` rows' room
    /// round it.
    fn reveal_icon(&self, i: usize, around: f32, cx: &mut WindowCx) {
        let g = *self.geo.borrow();
        if g.cols == 0 {
            return;
        }
        let view = f32::from(self.scroll.bounds().size.height);
        let top = g.inset + (i / g.cols) as f32 * g.cell.1;
        let (lo, hi) = (top - g.inset - around * g.cell.1, top + g.cell.1 + g.inset + around * g.cell.1);
        let now = -f32::from(self.scroll.offset().y);
        let to = if lo < now {
            lo
        } else if hi > now + view {
            hi - view
        } else {
            return;
        };
        let max = f32::from(self.scroll.max_offset().height);
        self.scroll.set_offset(cx, point(px(0.0), px(-to.clamp(0.0, max.max(0.0)))));
    }

    /// The icon view's look and grid.
    fn icon_look(&self) -> Look {
        if self.view == ViewMode::Icons { Look::Icons } else { Look::Mini }
    }

    /// Item `i` of the icon view, laid out where it is in the window.
    fn icon_layout(&self, i: usize) -> Option<items::ItemLayout> {
        let e = self.entries.get(i)?;
        Some(items::layout_item(self.geo.borrow().cell_at(i), e.label(), self.icon_look(), self.list.selected.contains(&i), e.is_link))
    }

    /// The item of the icon view at `p` (on its icon or its label).
    fn icon_hit(&self, p: Point<Pixels>) -> Option<usize> {
        let i = self.geo.borrow().index_at(p, self.entries.len())?;
        self.icon_layout(i).filter(|l| l.hit().contains(&p)).map(|_| i)
    }

    // ---- the icon views ----------------------------------------------------

    fn icon_press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        self.focus.focus(cx);
        // (Where the content is now: it scrolls.)
        self.geo.borrow_mut().origin = cx.listener_bounds().origin;
        let pos = e.position;
        let hit = self.icon_hit(pos);
        let on_label = hit.and_then(|i| self.icon_layout(i)).is_some_and(|l| l.highlight().contains(&pos));
        let m = mods(&e.modifiers);
        if e.button == MouseButton::Left && e.click_count >= 2 {
            if let Some(i) = hit
                && let Some(entry) = self.entries.get(i).cloned()
            {
                self.pending_edit = None;
                self.press = None;
                self.open(&entry);
            }
            return;
        }
        let was_selected = hit.is_some_and(|i| self.list.selected.contains(&i));
        match hit {
            Some(i) if !was_selected => {
                if m.command || m.shift {
                    self.list.selected.insert(i);
                    self.list.cursor = Some(i);
                } else {
                    self.list.select(i);
                }
            }
            Some(_) => {}
            None => {
                // A band: with ⇧ adding to what's selected, with ⌘ flipping
                // what it covers.
                if e.button == MouseButton::Left {
                    let origin = self.geo.borrow().origin;
                    self.band = Some((point(pos.x - origin.x, pos.y - origin.y), self.list.band(m)));
                    self.band_at = pos;
                }
                if !(m.command || m.shift) {
                    self.list.selected.clear();
                }
            }
        }
        if let Some(i) = hit
            && let Some(e) = self.entries.get(i)
        {
            // The icon's picture ready by the time a drag starts.
            let _ = olo_kit::icons::bitmap(items::icon_of(e), 0);
        }
        self.press = Some(Press { at: pos, button: e.button, item: hit, was_selected, on_label, mods: e.modifiers, dragging: false });
        cx.invalidate();
    }

    fn icon_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let Some(press) = &mut self.press else { return };
        if e.pressed_button.is_none() {
            return;
        }
        let moved = (e.position - press.at).magnitude() >= DRAG_SLOP as f64;
        match press.item {
            Some(i) if moved && !press.dragging => {
                press.dragging = true;
                let ask = press.button == MouseButton::Right;
                let at = press.at;
                self.pending_edit = None;
                let Some(entry) = self.entries.get(i) else { return };
                let icon = self.icon_layout(i).map(|l| l.icon).unwrap_or_default();
                let d = items::Drag {
                    paths: self.selected_paths(),
                    icon: items::icon_of(entry),
                    size: f32::from(icon.size.width).max(items::MINI),
                    hotspot: point((at.x - icon.left()).clamp(px(0.0), icon.size.width), (at.y - icon.top()).clamp(px(0.0), icon.size.height)),
                    ask,
                    name: entry.name.clone(),
                };
                items::start(cx, self.id, d, || FolderMsg::DragOver);
                cx.invalidate();
            }
            None if press.button == MouseButton::Left => {
                self.band_to(e.position, cx);
                cx.invalidate();
            }
            _ => {}
        }
    }

    /// The band now reaches `now` (in the window): what it covers is
    /// selected (every item in the rows it crosses, not only those
    /// showing), and near the top or bottom it scrolls on.
    fn band_to(&mut self, now: Point<Pixels>, cx: &mut WindowCx) {
        let Some((start, base)) = self.band.clone() else { return };
        self.band_at = now;
        let look = self.icon_look();
        let g = *self.geo.borrow();
        let here = point(now.x - g.origin.x, now.y - g.origin.y);
        let r = Bounds::from_corners(point(start.x.min(here.x), start.y.min(here.y)), point(start.x.max(here.x), start.y.max(here.y)));
        let r = Bounds { origin: point(r.origin.x + g.origin.x, r.origin.y + g.origin.y), size: r.size };
        let (cols, n) = (g.cols.max(1), self.entries.len());
        let row = |y: Pixels| (f32::from(y - g.origin.y - px(g.inset)) / g.cell.1).floor().max(0.0) as usize;
        let range = (row(r.top()) * cols).min(n)..((row(r.bottom()) + 1) * cols).min(n);
        let mut covered = Vec::new();
        for i in range {
            let e = &self.entries[i];
            if items::layout_item(g.cell_at(i), e.label(), look, false, e.is_link).hit().intersects(&r) {
                covered.push(i);
            }
        }
        let (view, cell_h) = (self.scroll.bounds(), g.cell.1);
        self.list.selected = base.over(covered);
        // Near or past the top or bottom: scroll on.
        let edge = px(cell_h / 4.0);
        let step = if now.y > view.bottom() - edge {
            f32::from((now.y - view.bottom() + edge) / 2.0).clamp(2.0, cell_h)
        } else if now.y < view.top() + edge {
            -f32::from((view.top() + edge - now.y) / 2.0).clamp(2.0, cell_h)
        } else {
            0.0
        };
        if step != 0.0 {
            let o = self.scroll.offset();
            let max = f32::from(self.scroll.max_offset().height);
            let to = (f32::from(o.y) - step).clamp(-max, 0.0);
            self.geo.borrow_mut().origin.y += px(to) - o.y;
            self.scroll.set_offset(cx, point(o.x, px(to)));
        }
    }

    fn icon_release(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let Some(press) = self.press.take() else { return };
        self.band = None;
        if press.dragging {
            cx.invalidate();
            return;
        }
        match (press.button, press.item) {
            (MouseButton::Left, Some(i)) if press.was_selected => {
                // A plain click on the name of the only selected item edits
                // it, after a moment (a double-click can still open it).
                let plain = !(press.mods.control || press.mods.platform || press.mods.shift);
                if plain && press.on_label && self.list.selected.len() == 1 {
                    if let Some(entry) = self.entries.get(i) {
                        self.pending_edit = Some((entry.path.clone(), Instant::now()));
                        cx.after_do(EDIT_DELAY, |f, cx| f.edit_if_still_pending(cx));
                    }
                } else {
                    self.pending_edit = None;
                }
                self.list.click(i, mods(&press.mods));
            }
            (MouseButton::Right, item) => self.context_menu(item.is_some(), e.position, cx),
            _ => self.pending_edit = None,
        }
        cx.invalidate();
    }

    /// The moment after a click on a selected name passed with no double
    /// click: edit it, if it's still the one selected.
    fn edit_if_still_pending(&mut self, cx: &mut Cx<Self>) {
        let Some((path, at)) = self.pending_edit.take() else { return };
        if at.elapsed() >= EDIT_DELAY - Duration::from_millis(20) && self.list.selected.len() == 1 && self.selected_paths().first() == Some(&path) {
            self.edit_name(&path, cx);
            cx.invalidate();
        } else if at.elapsed() < EDIT_DELAY - Duration::from_millis(20) {
            // (A later click's moment: its own timer comes.)
            self.pending_edit = Some((path, at));
        }
    }

    fn icon_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let look = self.icon_look();
        let (cell, inset) = if look == Look::Icons { (items::ICON_CELL, m::SPACING) } else { (items::MINI_CELL, m::TIGHT) };
        // As wide as the view less its bar (the window's width, before
        // it's been laid out).
        let view_w = {
            let w = f32::from(self.scroll.bounds().size.width);
            if w > 0.0 { w - SCROLL_BAR } else { f32::from(cx.viewport_size().width) - SCROLL_BAR }
        };
        let cols = (((view_w - 2.0 * inset) / cell.0).floor() as usize).max(1);
        let rows = self.entries.len().div_ceil(cols);
        let height = rows as f32 * cell.1 + 2.0 * inset;
        {
            let mut g = self.geo.borrow_mut();
            g.cols = cols;
            g.cell = cell;
            g.inset = inset;
        }
        // A selection made in code (Show in Tracker, a new item, one put
        // back after a reload): into view, with some of what's around it.
        if let Some(row) = self.take_show() {
            let around = ((f32::from(self.scroll.bounds().size.height) / cell.1 / 2.0).floor() - 1.0).max(0.0);
            self.reveal_icon(row, around, cx);
        }
        if self.editing.as_ref().is_some_and(|r| !self.entries.iter().any(|e| e.path == r.path)) {
            self.editing = None; // the item went away
        }
        // Editing a name: the field over its label, where it is in the
        // content (it scrolls with it).
        let field = self.editing.as_ref().and_then(|r| {
            let i = self.entries.iter().position(|e| e.path == r.path)?;
            let e = &self.entries[i];
            let c = Bounds { origin: point(px(inset + (i % cols) as f32 * cell.0), px(inset + (i / cols) as f32 * cell.1)), size: size(px(cell.0), px(cell.1)) };
            let l = items::layout_item(c, e.label(), look, true, e.is_link);
            Some(r.element(cx, &l, Point::default()))
        });
        let ground = theme::palette().document_background;
        let content = cx
            .canvas(move |f: &FolderWin, g, b| {
                let p = theme::palette();
                let clip = g.visible_bounds().intersect(&b);
                {
                    let mut geo = f.geo.borrow_mut();
                    geo.origin = b.origin;
                    geo.view = clip;
                }
                // What shows, and only that.
                let first = ((f32::from(clip.top() - b.top()) - inset) / cell.1).floor().max(0.0) as usize * cols;
                let last = (((f32::from(clip.bottom() - b.top()) - inset) / cell.1).ceil().max(0.0) as usize + 1) * cols;
                let cell_at =
                    |i: usize| Bounds { origin: point(b.left() + px(inset + (i % cols) as f32 * cell.0), b.top() + px(inset + (i / cols) as f32 * cell.1)), size: size(px(cell.0), px(cell.1)) };
                let editing = f.editing.as_ref().and_then(|r| f.entries.iter().position(|e| e.path == r.path));
                let (took, drop_on) = match f.dropping {
                    Some((_, on)) => (true, on),
                    None => (false, None),
                };
                g.with_content_mask(Some(olo_kit::ContentMask { bounds: clip }), |g| {
                    for i in first.min(f.entries.len())..last.min(f.entries.len()) {
                        let e = &f.entries[i];
                        let selected = f.list.selected.contains(&i);
                        let l = items::layout_item(cell_at(i), e.label(), look, selected, e.is_link);
                        items::paint_item(g, &l, items::icon_of(e), selected || drop_on == Some(i), false, editing != Some(i), &p);
                        if let Some(done) = e.progress() {
                            items::paint_progress(g, l.icon, done, &p);
                        }
                        if crate::backups::backed_up(&e.path) {
                            items::paint_mark(g, l.icon, &l.name, &p);
                        }
                    }
                    if took && drop_on.is_none() {
                        items::outline(g, clip, &p);
                    }
                    if let Some((start, _)) = f.band.as_ref().filter(|_| f.press.as_ref().is_some_and(|p| p.item.is_none() && p.button == MouseButton::Left)) {
                        let now = f.band_at;
                        let s = point(start.x + b.left(), start.y + b.top());
                        let r = Bounds::from_corners(point(s.x.min(now.x), s.y.min(now.y)), point(s.x.max(now.x), s.y.max(now.y)));
                        style::selection_band(g, r.intersect(&clip), &p);
                    }
                });
            })
            .size_full();
        ScrollView::new(&self.scroll)
            .border(false)
            .background(ground)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(height))
                    .min_h_full()
                    .on_mouse_down(MouseButton::Left, cx.listener(|f: &mut FolderWin, e: &MouseDownEvent, cx| f.icon_press(e, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|f: &mut FolderWin, e: &MouseDownEvent, cx| f.icon_press(e, cx)))
                    // The pointer held down goes on being followed past the
                    // view's edge.
                    .on_window_mouse_move(cx.listener(|f: &mut FolderWin, e: &MouseMoveEvent, cx| {
                        if f.press.is_some() {
                            f.geo.borrow_mut().origin = cx.listener_bounds().origin;
                            f.icon_move(e, cx);
                        }
                    }))
                    .on_window_mouse_up(cx.listener(|f: &mut FolderWin, e: &MouseUpEvent, cx| f.icon_release(e, cx)))
                    .child(content)
                    .children(field),
            )
            .into_any_element()
    }

    // ---- the list view -----------------------------------------------------

    fn list_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let key_col = |k: SortKey| match k {
            SortKey::Name => 0,
            SortKey::Size => 1,
            SortKey::Modified => 2,
            SortKey::Kind => 3,
        };
        // (Olo's own order shows as its files': newest first.)
        let sort = Some(self.sort.map_or((key_col(SortKey::Modified), false), |(k, asc)| (key_col(k), asc)));
        let reveal = self.take_show();
        // The window's selection is the list's.
        {
            let mut st = self.list_model.state_mut();
            if st.selected != self.list.selected || st.anchor != self.list.anchor || st.cursor != self.list.cursor || st.sort != sort {
                st.selected = self.list.selected.clone();
                st.anchor = self.list.anchor;
                st.cursor = self.list.cursor;
                st.sort = sort;
            }
        }
        if let Some(row) = reveal {
            self.list_model.reveal(cx, row);
        }
        let cols = [Column::new("Name", 220.0), Column::new("Size", 80.0).right(), Column::new("Modified", 160.0), Column::new("Kind", 120.0)];
        // Names for typing to select, made again only when the items change.
        if self.names.0 != self.stamp {
            self.names = (self.stamp, self.entries.iter().map(|e| SharedString::from(e.name.clone())).collect());
        }
        let names = self.names.1.clone();
        ColumnListView::new(cx, "list", &self.list_model, cols, self.entries.len(), |f: &FolderWin, cell, g| {
            let Some(e) = f.entries.get(cell.row) else { return };
            let editing = f.editing.as_ref().is_some_and(|r| r.path == e.path);
            match cell.column {
                0 => name_cell(cell, g, e, !editing),
                1 => cell.text(g, e.size_text()),
                2 => cell.text(g, fs::format_time(e.modified)),
                _ => cell.text(g, e.kind_text()),
            }
        })
        .type_select(move |_, i| names.get(i).cloned().unwrap_or_default())
        .on_select(cx.listener(|f: &mut FolderWin, _: &(), cx| {
            let s = f.list_model.state().clone();
            f.list.selected = s.selected;
            f.list.anchor = s.anchor;
            f.list.cursor = s.cursor;
            if f.editing.is_some() {
                f.end_rename(true, cx);
            }
            cx.invalidate();
        }))
        .on_sort(cx.listener(|f: &mut FolderWin, _: &(), cx| {
            let Some((col, asc)) = f.list_model.state().sort else { return };
            let key = [SortKey::Name, SortKey::Size, SortKey::Modified, SortKey::Kind][col.min(3)];
            if f.sort != Some((key, asc)) {
                f.sort = Some((key, asc));
                crate::sorts::remember(&f.path, f.sort);
                f.resort();
                cx.invalidate();
            }
        }))
        .on_open(cx.listener(|f: &mut FolderWin, row: &usize, cx| {
            // What's selected, if it's one of them.
            if f.list.selected.contains(row) {
                f.run("open", cx);
            } else if let Some(e) = f.entries.get(*row).cloned() {
                f.open(&e);
            }
        }))
        .on_context(cx.listener(|f: &mut FolderWin, (_, at): &(usize, Point<Pixels>), cx| f.context_menu(true, *at, cx)))
        .on_drag(cx.listener(|f: &mut FolderWin, row: &usize, cx| {
            let paths: Vec<PathBuf> = f.list_model.state().selected.iter().filter_map(|&i| f.entries.get(i)).map(|e| e.path.clone()).collect();
            let Some(e) = f.entries.get(*row).cloned() else { return };
            let d = items::Drag { paths, icon: items::icon_of(&e), size: items::MINI, hotspot: point(px(items::MINI / 2.0), px(items::MINI / 2.0)), ask: false, name: e.name.clone() };
            items::start(cx, f.id, d, || FolderMsg::DragOver);
        }))
        .into_any_element()
    }

    // ---- the window --------------------------------------------------------

    /// The bar along the bottom: the count on the left, free space on the
    /// right (BeOS put the count beside the scroll bar). A scroll bar and
    /// its etched edge tall: the small text's line under the edge (as
    /// tall as a bar, it ran a line past the window's bottom). The edge
    /// is two lines of device pixels, so what's under it is the scroll
    /// bar's height at every scale (at 1.5× it was a pixel taller), and
    /// it's that the text is centred in.
    fn count_bar(&self, cx: &WindowCx) -> Element {
        let (entries, list) = self.current();
        let n = entries.len();
        let loading = self.loading && self.entries.is_empty() && self.error.is_none();
        let count = match (n, list.selected.len()) {
            _ if loading => format!("Loading… {} items", self.listed.load(std::sync::atomic::Ordering::Relaxed)),
            (1, 0) => "1 item".to_owned(),
            (n, 0) => format!("{n} items"),
            (n, s) => format!("{s} of {n} selected"),
        };
        let free = self.free_space();
        // (The list view's own sunken frame edges it: an etched edge
        // under that one too crowded the two, a line apart.)
        let etched = self.view != ViewMode::List;
        canvas(move |g, b| {
            let p = theme::palette();
            let line = style::hairline(g);
            if etched {
                style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.3));
                style::hline(g, b.left(), b.right(), b.top() + line, style::c(p.shine));
            } else {
                style::fill_rect(g, Bounds::from_corners(b.origin, point(b.right(), b.top() + line * 2.0)), style::c(p.panel_background));
            }
            // Below the two-line etched edge.
            let under = Bounds::from_corners(point(b.left(), b.top() + line * 2.0), b.bottom_right());
            style::fill_rect(g, under, style::c(p.panel_background));
            let cy = under.center().y;
            let font = style::ui_font(FontWeight::NORMAL);
            // In line with the list's first column (its icons) above.
            let at = style::label(g, b.left() + px(m::CELL_INSET), cy, Align::Left, count.clone(), style::SMALL_SIZE, font.clone(), style::c(p.panel_text));
            olo_kit::agent::record(g, olo_kit::agent::Node::label(count.clone(), at));
            if let Some(free) = free {
                let text = format!("{} free", fs::format_size(free));
                let at = style::label(g, b.right() - px(m::CELL_INSET), cy, Align::Right, text.clone(), style::SMALL_SIZE, font, style::disabled_text(&p));
                olo_kit::agent::record(g, olo_kit::agent::Node::label(text, at));
            }
        })
        .w_full()
        .h(px(Self::count_bar_height(cx.scale_factor())))
        .into_any_element()
    }

    /// The count bar's height in points at `scale`, whole device pixels:
    /// its edge's two lines, and under them the device pixels the scroll
    /// bar above it covers. That bar is `SCROLL_BAR` from its bottom, on a
    /// pixel (this bar's top), so its top at a half pixel rounds down
    /// onto the next: 17.5 pixels at 1.25× cover 17, 24.5 at 1.75× 24
    /// (half rounded down). Measured at 1×, 1.25×, 1.5×, 1.75× and 2×:
    /// 14, 17, 21, 24 and 28, the scroll bar's own.
    fn count_bar_height(scale: f32) -> f32 {
        let under = (SCROLL_BAR * scale - 0.5).ceil();
        (under + 2.0 * style::line_pixels(scale)) / scale
    }

    /// The free space where the folder is (read again every few seconds,
    /// not every frame).
    fn free_space(&self) -> Option<u64> {
        let dir = self.current_dir();
        let mut free = self.free.borrow_mut();
        if let Some((at, d, v)) = &*free
            && *d == dir
            && at.elapsed() < Duration::from_secs(3)
        {
            return *v;
        }
        let v = fs::fs_space(&dir).map(|(f, _)| f);
        *free = Some((Instant::now(), dir, v));
        v
    }

    /// Listing a big folder shows its count growing: shown again a few
    /// times a second while it lists.
    fn poll_while_loading(&mut self, cx: &mut Cx<Self>) {
        let loading = self.loading && self.entries.is_empty() && self.error.is_none();
        if !loading || self.polling {
            return;
        }
        self.polling = true;
        cx.after_do(Duration::from_millis(100), |f, cx| {
            f.polling = false;
            cx.invalidate();
        });
    }

    // ---- what's dragged over it, and dropped ---------------------------------

    /// The folder a drag would spring open, if it rests there: looked at
    /// again when it would be due.
    pub fn rest_on(&mut self, dir: Option<&Path>, cx: &mut Cx<Self>) {
        if self.spring.rest(dir) {
            cx.after_do(items::SPRING + Duration::from_millis(10), |f, _| {
                if let Some(dir) = f.spring.due() {
                    to_hub(ToHub::Open { path: dir, from: None });
                }
            });
        }
    }

    /// What's dragged over the icon view at `at`.
    fn icon_drop_over(&mut self, o: &DropOffer, at: Point<Pixels>, cx: &mut Cx<Self>) -> Option<DropAccept> {
        if !self.geo.borrow().view.contains(&at) {
            self.dropping = None;
            self.rest_on(None, cx);
            return None;
        }
        let over = self.icon_hit(at).map(|i| (i, &self.entries[i], self.list.selected.contains(&i)));
        let (accept, on) = items::item_drop(cx, self.id, o, over, Some(&self.current_dir()));
        let spring_to = on.filter(|_| accept.is_some()).map(|i| self.entries[i].path.clone());
        self.dropping = accept.is_some().then_some((0, on));
        self.rest_on(spring_to.as_deref(), cx);
        accept
    }

    /// Files or text dropped on the folder, or on a folder in it.
    fn take_dropped(&mut self, drop: Dropped, cx: &mut Cx<Self>) {
        let Some(target) = DropTarget::parse(&drop.target) else { return };
        let paths = drop.paths();
        let dest = match target {
            DropTarget::Trash => {
                let trash = fs::trash_files();
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !p.starts_with(&trash)).collect();
                if !paths.is_empty() {
                    to_hub(ToHub::Job { kind: ops::Kind::Trash, sources: paths, dest: None, origin: Some(self.id) });
                }
                return;
            }
            DropTarget::Place => return,
            DropTarget::Folder(dest) => dest,
        };
        if let Err(e) = crate::dropped_into(&drop, paths, dest, Some(self.id), cx) {
            self.error(e, cx);
        }
    }

    // ---- commands ------------------------------------------------------------

    /// A command from the menus, a context menu, a key, an agent.
    pub fn run(&mut self, cmd: &str, cx: &mut Cx<Self>) {
        let (selected, entries, dir) = (self.selected_paths(), self.selected_entries(), self.current_dir());
        let job = |kind: ops::Kind, sources: Vec<PathBuf>, id: u64| to_hub(ToHub::Job { kind, sources, dest: None, origin: Some(id) });
        match cmd {
            "quick-look" => to_hub(ToHub::QuickLook(self.id)),
            "open" => {
                for e in &entries {
                    self.open(e);
                }
            }
            // The folder it's in, with this one selected (to go on from).
            "parent" => to_hub(ToHub::Reveal(dir, true)),
            "new-folder" => match fs::new_folder(&dir) {
                Ok(p) => {
                    to_hub(ToHub::Done("New Folder".into(), undo::Change::Made(vec![p.clone()])));
                    self.add_entry(&p);
                    self.edit_name(&p, cx);
                }
                Err(e) => self.error(format!("Couldn't create a folder: {e}"), cx),
            },
            "terminal" => fs::open_in_terminal(&dir),
            "back" => self.go(-1, cx),
            "forward" => self.go(1, cx),
            // File ▸ New Window (⌘N): another window on this folder, as
            // it shows it, beside this one (always a new one).
            "new" => to_hub(ToHub::NewWindow(dir, Some(self.view))),
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
            "older-versions" => {
                if let Some(e) = entries.iter().find(|e| !e.is_dir) {
                    crate::backups::older_versions(&e.path);
                }
            }
            "info" => {
                let of = if entries.is_empty() { fs::entry(&dir).into_iter().collect() } else { entries };
                to_hub(ToHub::GetInfo(of));
            }
            "rename" => {
                if let [p] = selected.as_slice() {
                    let p = p.clone();
                    self.edit_name(&p, cx);
                }
            }
            "duplicate" => {
                if !selected.is_empty() {
                    job(ops::Kind::Duplicate, selected, self.id);
                }
            }
            "link" => {
                let made: Vec<PathBuf> = selected.iter().filter_map(|p| fs::make_link(p).ok()).collect();
                to_hub(ToHub::Done(if made.len() == 1 { "Create Link" } else { "Create Links" }.into(), undo::Change::Made(made)));
                to_hub(ToHub::ReloadAll);
            }
            "undo" => to_hub(ToHub::UndoRedo(false, Some(self.id))),
            "redo" => to_hub(ToHub::UndoRedo(true, Some(self.id))),
            "trash" => {
                if !selected.is_empty() {
                    job(ops::Kind::Trash, selected, self.id);
                }
            }
            "restore" => {
                let top = fs::trash_files();
                let items: Vec<PathBuf> = selected.into_iter().filter(|p| p.parent() == Some(top.as_path())).collect();
                if !items.is_empty() {
                    job(ops::Kind::Restore, items, self.id);
                }
            }
            "empty-trash" => crate::confirm_empty_trash(cx),
            "cut" | "copy" => {
                // As file managers copy files, so they paste into Chrome,
                // Slack, GTK's and Qt's apps and other file managers too.
                if !selected.is_empty() {
                    crate::copy_clip(olo_kit_base::clipboard::files(&selected, cmd == "cut"));
                }
            }
            "paste" => {
                let to = cx.messenger();
                olo_kit::clipboard::paste_as(crate::PASTEABLE.iter().map(|s| s.to_string()).collect(), move |found| {
                    to.send(FolderMsg::Pasted(dir, found));
                });
                return;
            }
            "select-all" => {
                let (entries, list) = self.current_mut();
                list.select_all(entries.len());
            }
            "invert" => {
                let (entries, list) = self.current_mut();
                list.invert(entries.len());
            }
            "view-icons" => self.set_view(ViewMode::Icons),
            "view-mini" => self.set_view(ViewMode::MiniIcons),
            "view-list" => self.set_view(ViewMode::List),
            "view-columns" => self.set_view(ViewMode::Columns),
            "hidden" => to_hub(ToHub::Hidden),
            "sort-default" => {
                self.sort = None;
                crate::sorts::remember(&self.path, None);
                self.resort();
            }
            "sort-name" | "sort-size" | "sort-modified" | "sort-kind" => {
                let key = match cmd {
                    "sort-size" => SortKey::Size,
                    "sort-modified" => SortKey::Modified,
                    "sort-kind" => SortKey::Kind,
                    _ => SortKey::Name,
                };
                self.sort = Some(match self.sort {
                    Some((k, asc)) if k == key => (key, !asc),
                    _ => (key, true),
                });
                crate::sorts::remember(&self.path, self.sort);
                self.resort();
            }
            "go-home" => to_hub(ToHub::Open { path: fs::home(), from: None }),
            "go-desktop" => to_hub(ToHub::Open { path: fs::home().join("Desktop"), from: None }),
            "go-root" => to_hub(ToHub::Open { path: PathBuf::from("/"), from: None }),
            "go-trash" => {
                let t = fs::trash_files();
                let _ = std::fs::create_dir_all(&t);
                to_hub(ToHub::Open { path: t, from: None });
            }
            "favorite-on" | "favorite-off" => {
                let on = cmd == "favorite-on";
                std::thread::spawn(move || {
                    olo_kit_base::places::set_favorite(&dir, on);
                    crate::places_changed();
                });
                return;
            }
            "go-to" => to_hub(ToHub::GoTo(dir)),
            // (Agents': straight there.)
            c if c.starts_with("go-to:") => {
                if let Some(p) = crate::goto::parse(&c["go-to:".len()..], &dir) {
                    to_hub(ToHub::Reveal(p, false));
                }
            }
            // Open With ▸ an app (Always: and make it the kind's).
            c if olo_kit::open_with::is_command(c) => crate::open_with(c, &entries),
            c if play_on(c, &entries) => {}
            c if c.starts_with("go-place:") => {
                let dir = PathBuf::from(&c["go-place:".len()..]);
                if dir.is_dir() {
                    to_hub(ToHub::Open { path: dir, from: None });
                }
            }
            "find" => to_hub(ToHub::Find),
            "close" => cx.close_window(),
            "quit" => to_hub(ToHub::Quit),
            _ => {}
        }
        cx.invalidate();
    }

    /// What a Paste found on the clipboard: files (copied, or cut to move
    /// them), a picture (a file of it), or text (a clipping, as a drop of
    /// text makes).
    fn pasted(&mut self, dir: &Path, mime: &str, data: &[u8], cx: &mut Cx<Self>) {
        if let Some((paths, cut)) = olo_kit_base::clipboard::copied_files(mime, data) {
            // Nothing goes into itself.
            if let Some(p) = paths.iter().find(|p| p.is_dir() && dir.starts_with(p)) {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                return self.error(format!("“{name}” can't go inside itself."), cx);
            }
            if cut {
                // Moved: the clipboard has them where they are now, copied.
                let moved: Vec<PathBuf> = paths.iter().filter_map(|p| Some(dir.join(p.file_name()?))).collect();
                crate::copy_clip(olo_kit_base::clipboard::files(&moved, false));
            }
            let kind = if cut { ops::Kind::Move } else { ops::Kind::Copy };
            return to_hub(ToHub::Job { kind, sources: paths, dest: Some(dir.to_owned()), origin: Some(self.id) });
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
            Ok(p) => {
                to_hub(ToHub::Done("Paste".into(), undo::Change::Made(vec![p])));
                to_hub(ToHub::ReloadAll);
            }
            Err(e) => self.error(format!("Couldn't paste: {e}"), cx),
        }
    }

    /// What the app knows of this window, told when it changes: its
    /// folders (to watch), its one selected item (Quick Look follows
    /// it), what an upgrade keeps of it.
    fn tell_app(&mut self) {
        let info = WinInfo {
            path: self.path.clone(),
            surface: self.surface,
            watched: self.watched().map(Path::to_owned).collect(),
            selected: {
                let mut sel = self.selected_entries().into_iter();
                sel.next().filter(|_| sel.next().is_none())
            },
            folder: Some(SavedFolder {
                path: self.path.clone(),
                size: self.size,
                view: self.view,
                sort: self.sort.unwrap_or((SortKey::Name, true)),
                own_sort: self.sort.is_some(),
                selected: self.selected_paths(),
            }),
            find: false,
        };
        match &self.told {
            Some(told) if told.same(&info) => {}
            // (Its size alone: kept for the app to find, not said.)
            Some(told) if told.same_but_for_size(&info) => {
                crate::keep(self.id, info.clone());
                self.told = Some(info);
            }
            _ => {
                crate::publish(self.id, info.clone());
                self.told = Some(info);
            }
        }
    }
}

/// A list cell with a 16-point icon before the name (`text`: and the
/// name, unless it's being edited).
pub fn name_cell(cell: &mut Cell, g: &mut Painter, e: &Entry, text: bool) {
    let b = cell.bounds;
    let icon = Bounds { origin: point(b.left() + px(m::CELL_INSET), b.center().y - px(items::MINI / 2.0)), size: size(px(items::MINI), px(items::MINI)) };
    olo_kit::icons::paint(g, icon, items::icon_of(e));
    if crate::backups::backed_up(&e.path) {
        items::paint_mark(g, icon, e.label(), &theme::palette());
    }
    let rest = Bounds::from_corners(point(icon.right() + px(m::MARK_GAP - m::CELL_INSET), b.top()), b.bottom_right());
    let saved = cell.bounds;
    cell.bounds = rest;
    if text {
        let color = cell.text_color;
        cell.text_with(g, e.label().to_owned(), items::name_font(e.is_link), color);
    } else {
        cell.say(e.label().to_owned());
    }
    cell.bounds = saved;
}

/// Delete and F2 trash and rename in a view as well as the menus' keys,
/// unless they are the menus' keys for it already (Windows'), which would
/// do it twice.
pub fn plain_extra(command: &str, key: &str) -> bool {
    olo_kit_base::keys::shortcut(command).is_none_or(|s| !(s.key == olo_kit::keys::evdev(key) && !s.command && !s.alt && !s.shift))
}

impl View for FolderWin {
    type Message = FolderMsg;

    fn message(&mut self, message: FolderMsg, cx: &mut Cx<Self>) {
        match message {
            FolderMsg::Listed { path, generation, result } => self.listed(&path, generation, result),
            FolderMsg::Changed(changes) => {
                let touched = changes.iter().any(|c| match c {
                    watch::Change::Item { dir, .. } | watch::Change::Rescan(dir) => self.watched().any(|w| w == dir),
                    watch::Change::Mounts => false,
                });
                if !touched {
                    return;
                }
                self.apply(&changes);
            }
            FolderMsg::Reload(hidden) => self.reload(hidden),
            FolderMsg::Select(paths) => self.select_paths(&paths),
            FolderMsg::CameFrom(from) => self.came_from(from),
            FolderMsg::Refresh => {}
            FolderMsg::Step(d) => {
                if let Some(e) = self.step_selection(d) {
                    let row = self.current().0.iter().position(|x| x.path == e.path);
                    self.show.extend(row.map(|r| (self.active, r)));
                    to_hub(ToHub::Look(self.id, e));
                }
            }
            FolderMsg::Pasted(dir, found) => {
                if let Some((mime, data)) = found {
                    self.pasted(&dir, &mime, &data, cx);
                }
            }
            FolderMsg::DragOver => {
                self.press = None;
                self.cols.end_press();
            }
            FolderMsg::Activate(token) => {
                match token {
                    Some(t) => cx.activate_window_with_token(&t),
                    None => cx.activate_window(),
                }
                return;
            }
            FolderMsg::Close => return cx.close_window(),
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        // (A name being edited is taken when the person goes elsewhere.)
        if !active && self.editing.is_some() {
            self.end_rename(true, cx);
        }
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        to_hub(ToHub::Closed(self.id));
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        // By what the window is, not by what it last told the app: one
        // brought back and not drawn yet has told nothing, and kept
        // nothing. Told to become its new version and, before its
        // windows had drawn, to start again for an app_server that went
        // (an upgrade does both, a moment apart), Tracker came back
        // without its folder windows one upgrade in three.
        let selected = match self.selected_paths() {
            // (Not listed yet: what it was to select once it is.)
            none if none.is_empty() => self.pending_select.last().map(|(_, paths)| paths.clone()).unwrap_or_default(),
            selected => selected,
        };
        let folder = crate::SavedFolder { path: self.path.clone(), size: self.size, view: self.view, sort: self.sort.unwrap_or((SortKey::Name, true)), own_sort: self.sort.is_some(), selected };
        serde_json::to_string(&crate::SavedWindow::Folder(folder)).ok()
    }

    fn takes_drops(&self) -> bool {
        true
    }

    fn drop_over(&mut self, offer: Option<&DropOffer>, at: Point<Pixels>, cx: &mut Cx<Self>) -> Option<DropAccept> {
        let was = self.dropping;
        let accept = match (offer, self.view) {
            (None, _) | (_, ViewMode::List) => {
                self.dropping = None;
                self.rest_on(None, cx);
                None
            }
            (Some(o), ViewMode::Columns) => self.columns_drop_over(o, at, cx),
            (Some(o), _) if self.error.is_none() => self.icon_drop_over(o, at, cx),
            _ => None,
        };
        if self.dropping != was {
            cx.invalidate();
        }
        accept
    }

    fn dropped(&mut self, dropped: Dropped, cx: &mut Cx<Self>) {
        self.take_dropped(dropped, cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.surface = cx.surface_id().or(self.surface);
        let vs = cx.viewport_size();
        self.size = (f32::from(vs.width).round() as u32, f32::from(vs.height).round() as u32);
        if !self.path_said {
            self.path_said = true;
            olo_kit::agent::set_path(cx, Some(self.path.clone()));
            cx.set_window_title(fs::display_name(&self.path));
        }
        // The keyboard: an edited name's field has it (given as the edit
        // began), else the view.
        if self.editing.is_none() {
            if self.focus_after_edit || self.focused_view != Some(self.view) {
                // (Shown another way, the keyboard goes with it.)
                self.focus_after_edit = false;
                self.focused_view = Some(self.view);
                match self.view {
                    ViewMode::List => self.list_model.focus(cx),
                    _ => self.focus.focus(cx),
                }
            } else if self.view != ViewMode::List {
                self.focus.focus_if_none(cx);
            }
        }
        self.poll_while_loading(cx);
        self.tell_app();
        let places = crate::places();
        let can_paste = crate::can_paste();
        let undo = crate::undo_names();
        let menus = self.menus(can_paste, &places, &undo);
        let bar = MenuBar::new(cx, menus);
        let body: Element = match (&self.error, self.view) {
            (Some(err), _) => {
                let err = err.clone();
                canvas(move |g, b| {
                    let p = theme::palette();
                    style::fill_rect(g, b, style::c(p.document_background));
                    let at = style::label(g, b.center().x, b.center().y, Align::Center, err.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                    olo_kit::agent::record(g, olo_kit::agent::Node::label(err.clone(), at));
                })
                .size_full()
                .into_any_element()
            }
            (None, ViewMode::List) => self.list_view(cx),
            (None, ViewMode::Columns) => self.columns_view(cx),
            (None, _) => self.icon_view(cx),
        };
        use olo_kit::NavigationDirection;
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|f: &mut FolderWin, e: &KeyDownEvent, cx| f.key_down(e, cx)))
            // The mouse's back and forward buttons.
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|f: &mut FolderWin, _: &MouseDownEvent, cx| f.go(-1, cx)))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|f: &mut FolderWin, _: &MouseDownEvent, cx| f.go(1, cx)))
            .child(bar)
            .child(div().flex_1().min_h_0().relative().flex().flex_col().child(body))
            .child(self.count_bar(cx))
            .into_any_element()
    }
}

/// Play On ▸ for a film (one, not a folder): the AirPlay receivers paired
/// with, each fetching and playing the file itself (`olo-airplay play`).
pub fn play_on_menu(entries: &[Entry]) -> Option<Menu> {
    let [e] = entries else { return None };
    (!e.is_dir && olo_xdg::open::kind_of(&e.name, false).starts_with("video/")).then(|| Menu::airplay("Play On", &olo_kit::airplay::paired(), ""))
}

/// Play On ▸'s command, for the film chosen; false if it isn't one.
pub fn play_on(cmd: &str, entries: &[Entry]) -> bool {
    match entries {
        [e] if olo_protocol::airplay::menu_command(cmd).is_some() => olo_kit::airplay::play_from_menu(cmd, &e.path.to_string_lossy()),
        _ => false,
    }
}

/// Open With ▸ for `entries` of one kind (`olo_kit::open_with`: the
/// apps worked out as the folder was listed; none yet, no submenu).
pub fn open_with_menu(entries: &[Entry]) -> Option<Menu> {
    olo_kit::open_with::menu(&open_with_kind(entries)?)
}

/// The kind `entries` all are, for Open With (None: none, or mixed).
pub fn open_with_kind(entries: &[Entry]) -> Option<String> {
    olo_kit::open_with::kind_of(entries.iter().map(|e| (e.name.as_str(), e.is_dir)))
}
