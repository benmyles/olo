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
    Bounds, ClickEvent, CursorStyle, Cx, DISABLED_OPACITY, DropAccept, DropOffer, Dropped, Element, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu,
    MenuBar, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, PathBuilder, Pixels, Point, ScrollHandle, SharedString, Styled as _, View, WindowCx,
    canvas, div, menu, metrics as m, point, px, size,
    style::{self, Align},
    theme,
    widgets::{
        Button, SCROLL_BAR, ScrollView,
        list::{Cell, Column, ColumnListView, ListModel},
        open_alert,
    },
};
use olo_kit_base::list::{ListState, Mods, SelectionBand, TypeSelect};
use olo_theme::mix;

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
/// Navigator bar geometry: 33 pt bar (1 pt bottom shadow + 32 pt interior, giving
/// symmetric 6 pt top/bottom margins around 22 × 20 pt bevelled Back / Forward / Up /
/// Refresh buttons and the sunken path well 6 pt from them).
const NAV_HEIGHT: f32 = 33.0;
const NAV_BTN_W: f32 = 22.0;
const NAV_BTN_H: f32 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NavBtn {
    Back,
    Forward,
    Up,
    Refresh,
}

/// What a folder window is told: by its workers, by the app, by itself
/// later.
pub enum FolderMsg {
    /// A folder listed (asked as request `generation`).
    Listed {
        path: PathBuf,
        generation: u64,
        result: Result<Vec<Entry>, String>,
        remote_free: Option<u64>,
        host_state: Option<crate::rex::HostState>,
        resolved: Option<PathBuf>,
    },
    /// A new remote folder created off the UI thread, ready to rename in place.
    Created {
        dir: PathBuf,
        result: Result<(PathBuf, Option<Entry>), String>,
    },
    /// Changes seen on disk.
    Changed(Arc<Vec<watch::Change>>),
    /// List everything again (hidden files shown or not, now).
    Reload(bool),
    /// These, selected (as their listing brings them in).
    Select(Vec<PathBuf>),
    /// Opened from this folder's window: Back goes there.
    CameFrom(PathBuf),
    /// Goes to a folder in this window (a link opened here found one:
    /// `fs::open_files`).
    GoTo(PathBuf),
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
    pub(crate) path_said: bool,
    /// Last location reflected in the window title & agent path (follows
    /// Columns view's deepest open subfolder so tab title and path well agree).
    shown_loc: PathBuf,
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
    /// Free bytes reported by a remote host's last `olo-rex-link fs list`.
    pub remote_free: Option<u64>,
    /// Connection state of a remote host, shown as the Navigator's live status light.
    pub remote_state: Option<crate::rex::HostState>,
    nav_hover: Option<NavBtn>,
    nav_press: Option<NavBtn>,
    /// What the app was last told of this window.
    told: Option<WinInfo>,
}

/// The icon view's grid as last laid out; with where its content is (in
/// the window: it scrolls) and what of it shows when asked for
/// (`FolderWin::icon_geo`).
#[derive(Default, Clone, Copy)]
struct IconGeo {
    origin: Point<Pixels>,
    view: Bounds<Pixels>,
    cols: usize,
    cell: (f32, f32),
    inset: f32,
    /// From the left to the first column: the room past the columns
    /// shared by both sides in the icon view (`inset` and the rest on the
    /// right, the grid sat off to the left: Gemini's look, 2026-11-28).
    inset_x: f32,
}

impl IconGeo {
    /// Item `i`'s cell, in the window.
    fn cell_at(&self, i: usize) -> Bounds<Pixels> {
        let (c, r) = (i % self.cols.max(1), i / self.cols.max(1));
        Bounds { origin: point(self.origin.x + px(self.inset_x + c as f32 * self.cell.0), self.origin.y + px(self.inset + r as f32 * self.cell.1)), size: size(px(self.cell.0), px(self.cell.1)) }
    }

    /// The item whose cell `p` is in, of `n`.
    fn index_at(&self, p: Point<Pixels>, n: usize) -> Option<usize> {
        let (x, y) = (f32::from(p.x - self.origin.x) - self.inset_x, f32::from(p.y - self.origin.y) - self.inset);
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
    /// The second press of a double click: it opens at its release,
    /// unless dragged.
    opens: bool,
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
            shown_loc: PathBuf::new(),
            focus_after_edit: false,
            focused_view: None,
            stamp: 0,
            names: (u64::MAX, Rc::from(Vec::new())),
            polling: false,
            free: RefCell::new(None),
            remote_free: None,
            remote_state: None,
            nav_hover: None,
            nav_press: None,
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
        if self.view == ViewMode::Columns { self.panes.last().map_or_else(|| self.path.clone(), |p| p.path.clone()) } else { self.path.clone() }
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
        if self.view == ViewMode::Columns && fs::starts_with_dir(&path, &self.path) {
            self.show_in_columns(&path, &from);
            cx.invalidate();
            return;
        }
        self.show_folder(path.clone(), cx);
        // Back out of a folder inside it: that folder selected.
        if let Some(segs) = fs::relative_segments(&from, &path)
            && let Some(first) = segs.first()
        {
            self.pending_select.push((path.clone(), vec![fs::join_name(&path, first)]));
        }
    }

    /// Column view: the columns opened down to `dir` (inside its folder),
    /// each folder on the way selected in the column before, nothing in
    /// `dir`'s. Columns already showing the way stay; each other opens as
    /// the one before it is listed (`pending_select`).
    fn show_in_columns(&mut self, dir: &std::path::Path, from: &std::path::Path) {
        let Some(segs) = fs::relative_segments(dir, &self.path) else { return };
        let mut way = vec![self.path.clone()];
        for c in segs {
            let next = fs::join_name(&way[way.len() - 1], &c);
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
        if !fs::is_remote(&path) {
            let places = path.clone();
            std::thread::spawn(move || olo_kit_base::places::visited(&places));
            self.remote_free = None;
            self.remote_state = None;
        } else {
            self.remote_free = None;
        }
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

    /// Opens `e`: a folder in this window (`go_to`), a file by what opens
    /// its kind.
    pub fn open(&mut self, e: &Entry, cx: &mut Cx<Self>) {
        if e.is_dir {
            self.go_to(e.path.clone(), None, cx);
        } else {
            fs::open(e);
        }
    }

    /// Goes to folder `path` in this window (Ben, 2026-11-25: "when
    /// opening a folder it shouldn't open it in a new window it should
    /// take you to that folder in the current window you open it from"),
    /// where BeOS's Tracker opened a window a folder: a new place in its
    /// history, so Back comes back to where it was. `select`: what is
    /// selected there once it is listed (Open Parent: the folder it came
    /// out of). File ▸ New Window and Open in New Window still give a
    /// window of its own.
    pub fn go_to(&mut self, path: PathBuf, select: Option<PathBuf>, cx: &mut Cx<Self>) {
        if path == self.location() {
            return;
        }
        self.record_location();
        self.history.truncate(self.at + 1);
        self.history.push(path.clone());
        self.at = self.history.len() - 1;
        self.show_folder(path.clone(), cx);
        if let Some(select) = select {
            self.pending_select.push((path, vec![select]));
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
        let dir = self.current_dir();
        let remote = fs::is_remote(&dir);
        let in_trash = !remote && dir.starts_with(fs::trash_files());
        let selected = self.selected_entries();

        // Group 1: Open family (Opus, 2026-11-25: keep all Open… items together).
        let mut menu = Menu::new("").item("Open", "open").key("o");
        if selected.iter().any(|e| e.is_dir) {
            menu = menu.item("Open in New Window", "open-new-window");
        }
        if !remote {
            if one && selected.first().and_then(fs::running_app).is_some() {
                menu = menu.item("New Window", "new-window");
            }
            if one {
                menu = fs::grouping_item(menu, selected.first());
            }
            if let Some(open_with) = open_with_menu(&selected) {
                menu = menu.submenu(open_with);
            }
            if let Some(play) = play_on_menu(&selected) {
                menu = menu.submenu(play);
            }
            if selected.iter().all(|e| e.is_dir) {
                menu = menu.item("Open in Terminal", "terminal-selected");
                menu = menu.item("Open in Rex", "rex-selected");
            }
        } else if selected.iter().all(|e| e.is_dir) {
            menu = menu.item("Show Host in Rex", "rex-selected");
        }

        // Group 2: Inspect.
        menu = menu.separator().item("Quick Look", "quick-look").key("y").enabled(one).item("Get Info", "info").key("i");
        if !remote && crate::backups::on() {
            let file = one && selected.first().is_some_and(|e| !e.is_dir);
            menu = menu.item("Older Versions…", "older-versions").enabled(file);
        }

        // Group 3: Modify.
        menu = menu.separator().item("Edit Name", "rename").key("e").enabled(one).item("Duplicate", "duplicate").key("d");
        if !remote {
            menu = menu.item("Create Link", "link");
            if !in_trash {
                menu = crate::archives::menu_items(menu, &selected);
            }
        }

        // Group 4: Clipboard.
        menu = menu.separator().item("Cut", "cut").key("x").item("Copy", "copy").key("c").separator();

        // Group 5: Remove / Restore.
        if remote {
            menu = menu.item("Delete…", "remote-delete");
        } else if !in_trash {
            menu = menu.item("Move to Trash", "trash").key("t");
        } else if dir == fs::trash_files() {
            // Items at the top of the Trash go back where they came from.
            menu = menu.item("Restore", "restore");
        }
        menu
    }

    pub fn background_menu(&self, can_paste: bool) -> Menu {
        let dir = self.current_dir();
        let remote = fs::is_remote(&dir);
        let in_trash = !remote && dir.starts_with(fs::trash_files());
        let mut menu = Menu::new("")
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
            .key("i");
        if remote {
            menu = menu.item("Refresh", "reload").key("r").item("Show Host in Rex", "rex");
        } else {
            menu = menu.item("Open in Terminal", "terminal").item("Open in Rex", "rex");
        }
        menu.item("Open Parent", "parent").key("up").enabled(fs::parent_of(&self.location()).is_some())
    }

    /// The menu bar's menus.
    /// `undo`: Edit ▸ Undo's and Redo's names, and whether there's a step
    /// for each (the app's history).
    fn menus(&self, can_paste: bool, places: &olo_kit_base::places::Places, undo: &[(String, bool); 2]) -> Vec<Menu> {
        let current = self.current().1;
        let has_sel = !current.selected.is_empty();
        let dir = self.current_dir();
        let remote = fs::is_remote(&dir);
        let in_trash = !remote && dir.starts_with(fs::trash_files());
        // ⌘N a new window, ⇧⌘N a new folder, as Finder has them (Ben,
        // 2026-11-04: "pressing super+n should open a new tracker window").
        // (Its settings: its own, and which app opens which files.)
        let mut file = Menu::new("File")
            .app_items(&["tracker", "default-apps"])
            .item("New Window", "new")
            .key("n")
            .item("New Folder", "new-folder")
            .shift_key("n")
            .enabled(!in_trash)
            .separator()
            .item("Open", "open")
            .key("o")
            .enabled(has_sel)
            .item("Open Parent", "parent")
            .key("up")
            .enabled(fs::parent_of(&self.location()).is_some())
            .item("Get Info", "info")
            .key("i");
        if remote {
            file = file.item("Show Host in Rex", "rex");
        } else {
            file = file.item("Open in Terminal", "terminal").item("Open in Rex", "rex");
        }
        file = file
            .item("Quick Look", "quick-look")
            .key("y")
            .enabled(has_sel)
            .item("Edit Name", "rename")
            .key("e")
            .enabled(current.selected.len() == 1)
            .item("Duplicate", "duplicate")
            .key("d")
            .enabled(has_sel);
        if !remote {
            file = file.item("Create Link", "link").enabled(has_sel);
            if has_sel && !in_trash {
                file = crate::archives::menu_items(file, &self.selected_entries());
            }
        }
        let mut file =
            if remote { file.separator().item("Delete…", "remote-delete").enabled(has_sel) } else { file.separator().item("Move to Trash", "trash").key("t").enabled(has_sel && !in_trash) };
        if in_trash {
            file = file.item("Restore", "restore").enabled(has_sel && dir == fs::trash_files()).item("Empty Trash", "empty-trash").enabled(!fs::trash_is_empty());
        }
        let file = file.close_quit();
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
            .key("s")
            .separator()
            // (A query of the disk, BeOS's File ▸ Find…, where every app's
            // Find… is.)
            .item("Find…", "find")
            .key("f");
        let sort = |k: SortKey| self.sort.is_some_and(|(s, _)| s == k);
        let window = Menu::new("View")
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
            .item("Show Navigator", "navigator")
            .checked(crate::show_navigator())
            .item("Show Hidden Files", "hidden")
            .checked(self.hidden)
            .separator()
            .item("Refresh", "reload")
            .key("r");
        // The standard folders, favourites (this one added or taken away)
        // and recent folders, as every file panel has them.
        let here = self.current_dir();
        let mut favorites = Menu::new("Favorites");
        for d in places.favorites.iter() {
            favorites = favorites.item(olo_kit_base::places::label(d), format!("go-place:{}", d.display()));
        }
        if !remote {
            if !places.favorites.is_empty() {
                favorites = favorites.separator();
            }
            let name = olo_kit_base::places::label(&here);
            favorites = if places.favorites.contains(&here) {
                favorites.item(olo_kit::tr!("Remove “{}” from Favorites", name), "favorite-off")
            } else {
                favorites.item(olo_kit::tr!("Add “{}” to Favorites", name), "favorite-on")
            };
        }
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
                if fs::is_remote(&self.current_dir()) {
                    self.run("remote-delete", cx);
                } else {
                    self.run("trash", cx);
                }
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

    /// The icon view's grid, with where its content is in the window now
    /// and what shows of it: from where it's scrolled to, not where it
    /// was last drawn (its rows are drawn only when they change).
    fn icon_geo(&self) -> IconGeo {
        let mut g = *self.geo.borrow();
        let view = self.scroll.bounds();
        g.origin = view.origin + self.scroll.offset();
        g.view = view;
        g
    }

    /// What shows of the icon view's content, in the content.
    fn icon_shown(&self) -> Bounds<Pixels> {
        let o = self.scroll.offset();
        Bounds { origin: point(-o.x, -o.y), size: self.scroll.bounds().size }
    }

    /// The outline round what shows while what's dragged over it would
    /// go into the folder, in the content.
    fn icon_outline(&self) -> Option<Bounds<Pixels>> {
        matches!(self.dropping, Some((_, None))).then(|| self.icon_shown())
    }

    /// The band being drawn, in the content, as far as it shows.
    fn icon_band(&self) -> Option<Bounds<Pixels>> {
        let (start, _) = self.band.as_ref().filter(|_| self.press.as_ref().is_some_and(|p| p.item.is_none() && p.button == MouseButton::Left))?;
        let o = self.icon_geo().origin;
        let now = point(self.band_at.x - o.x, self.band_at.y - o.y);
        let r = Bounds::from_corners(point(start.x.min(now.x), start.y.min(now.y)), point(start.x.max(now.x), start.y.max(now.y)));
        Some(r.intersect(&self.icon_shown()))
    }

    /// Row `row` of the icon view's grid (`cols` items of `look` in
    /// cells of `cell`, `inset` from the left), in `b`.
    #[allow(clippy::too_many_arguments)]
    fn paint_icon_row(&self, g: &mut Painter, b: Bounds<Pixels>, row: usize, look: Look, cols: usize, cell: (f32, f32), inset: f32) {
        let p = theme::palette();
        let drop_on = self.dropping.and_then(|(_, on)| on);
        for i in row * cols..((row + 1) * cols).min(self.entries.len()) {
            let e = &self.entries[i];
            let c = Bounds { origin: point(b.left() + px(inset + (i % cols) as f32 * cell.0), b.top()), size: size(px(cell.0), px(cell.1)) };
            let selected = self.list.selected.contains(&i);
            let l = items::layout_item(c, e.label(), look, e.is_link);
            let editing = self.editing.as_ref().is_some_and(|r| r.path == e.path);
            // (A picture's icon is the picture, a video's a frame of it:
            // the big icons; the small ones are their kinds'.)
            let small = if look == Look::Icons { items::small(g, e) } else { None };
            items::paint_item(g, &l, items::icon_of(e), small.as_ref(), selected || drop_on == Some(i), false, !editing, e.broken, &p);
            if let Some(done) = e.progress() {
                items::paint_progress(g, l.icon, done, &p);
            }
            if crate::backups::backed_up(&e.path) {
                items::paint_mark(g, l.icon, &l.name, &p);
            }
        }
    }

    /// What row `row` of the icon view shows, as a number that changes
    /// when it does: each of its items' look (`row_key`), whether it's
    /// selected, dropped on or being renamed.
    fn icon_row_key(&self, row: usize, look: Look, cols: usize) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (look == Look::Icons, cols).hash(&mut h);
        let drop_on = self.dropping.and_then(|(_, on)| on);
        for i in row * cols..((row + 1) * cols).min(self.entries.len()) {
            let e = &self.entries[i];
            (row_key(e), self.list.selected.contains(&i), drop_on == Some(i), self.editing.as_ref().is_some_and(|r| r.path == e.path)).hash(&mut h);
            // (A row with a picture in it is drawn again as pictures are
            // made small: its own may be the one that came.)
            if look == Look::Icons && items::small_of(e).is_some() {
                crate::thumbs::made().hash(&mut h);
            }
        }
        h.finish()
    }

    /// Item `i` of the icon view, laid out where it is in the window.
    fn icon_layout(&self, i: usize) -> Option<items::ItemLayout> {
        let e = self.entries.get(i)?;
        Some(items::layout_item(self.icon_geo().cell_at(i), e.label(), self.icon_look(), e.is_link))
    }

    /// The item of the icon view at `p` (on its icon or its label).
    fn icon_hit(&self, p: Point<Pixels>) -> Option<usize> {
        let i = self.icon_geo().index_at(p, self.entries.len())?;
        self.icon_layout(i).filter(|l| l.hit().contains(&p)).map(|_| i)
    }

    // ---- the icon views ----------------------------------------------------

    fn icon_press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        self.focus.focus(cx);
        let pos = e.position;
        let hit = self.icon_hit(pos);
        let on_label = hit.and_then(|i| self.icon_layout(i)).is_some_and(|l| l.highlight().contains(&pos));
        let m = mods(&e.modifiers);
        // (A double click opens at its release, where it wasn't dragged:
        // a press that moves is a drag, the second of two quick ones too.
        // It opened at the press, so a name clicked and then dragged at
        // once opened its file, or did nothing, and never dragged: Ben,
        // 2026-11-17, "it always works if i drag by icon but only
        // sometimes if i drag by the name".)
        let opens = e.button == MouseButton::Left && e.click_count >= 2 && hit.is_some();
        if opens {
            self.pending_edit = None;
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
                    let origin = self.icon_geo().origin;
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
        self.press = Some(Press { at: pos, button: e.button, item: hit, was_selected, on_label, mods: e.modifiers, dragging: false, opens });
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
        let g = self.icon_geo();
        let here = point(now.x - g.origin.x, now.y - g.origin.y);
        let r = Bounds::from_corners(point(start.x.min(here.x), start.y.min(here.y)), point(start.x.max(here.x), start.y.max(here.y)));
        let r = Bounds { origin: point(r.origin.x + g.origin.x, r.origin.y + g.origin.y), size: r.size };
        let (cols, n) = (g.cols.max(1), self.entries.len());
        let row = |y: Pixels| (f32::from(y - g.origin.y - px(g.inset)) / g.cell.1).floor().max(0.0) as usize;
        let range = (row(r.top()) * cols).min(n)..((row(r.bottom()) + 1) * cols).min(n);
        let mut covered = Vec::new();
        for i in range {
            let e = &self.entries[i];
            if items::layout_item(g.cell_at(i), e.label(), look, e.is_link).hit().intersects(&r) {
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
        if press.opens {
            self.pending_edit = None;
            if (e.position - press.at).magnitude() < DRAG_SLOP as f64
                && let Some(entry) = press.item.and_then(|i| self.entries.get(i)).cloned()
            {
                self.open(&entry, cx);
            }
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
            // (Measured, the scroll view's own, its bar left out already:
            // less the bar again, the grid sat off to the left.)
            let w = f32::from(self.scroll.bounds().size.width);
            if w > 0.0 { w } else { f32::from(cx.viewport_size().width) - SCROLL_BAR }
        };
        let cols = (((view_w - 2.0 * inset) / cell.0).floor() as usize).max(1);
        let rows = self.entries.len().div_ceil(cols);
        let height = rows as f32 * cell.1 + 2.0 * inset;
        // (Icons: the room past the columns shared by both sides, on a
        // device pixel. Mini icons start at the left, their names after.)
        let s = cx.scale_factor();
        let inset_x = if look == Look::Icons { (((view_w - cols as f32 * cell.0) / 2.0).max(inset) * s).floor() / s } else { inset };
        {
            let mut g = self.geo.borrow_mut();
            g.cols = cols;
            g.cell = cell;
            g.inset = inset;
            g.inset_x = inset_x;
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
            let c = Bounds { origin: point(px(inset_x + (i % cols) as f32 * cell.0), px(inset + (i / cols) as f32 * cell.1)), size: size(px(cell.0), px(cell.1)) };
            let l = items::layout_item(c, e.label(), look, e.is_link);
            Some(r.element(cx, &l, Point::default()))
        });
        let ground = theme::palette().document_background;
        // The items a row of the grid at a time, each row drawn again only
        // when what it shows changes, and made as it comes into view: a
        // step of the selection draws the row it left and the one it's
        // on, a taller window only the rows it uncovers.
        let grid = olo_kit::uniform_rows(rows, px(cell.1), move |near| {
            near.map(|row| {
                olo_kit::view_canvas::<FolderWin>(move |f, g, b| f.paint_icon_row(g, b, row, look, cols, cell, inset_x))
                    .key_by::<FolderWin>(move |f| style::key((f.icon_row_key(row, look, cols), inset_x.to_bits())))
                    .id(row)
                    .w_full()
            })
            .collect()
        })
        // (A row is made of `look` and `cols`, and what its key says.)
        // (The inset too: kept, a row widened past it painted where its
        // clicks no longer went.)
        .row_key_by::<FolderWin>(move |f, row| style::key((f.icon_row_key(row, look, cols), inset_x.to_bits())))
        .absolute()
        .left(px(0.0))
        .top(px(inset));
        // Over them: the band, and the outline round what shows while
        // what's dragged over would go into the folder.
        let bits = |r: Option<Bounds<Pixels>>| r.map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height].map(|v| f32::from(v).to_bits()));
        let over = olo_kit::view_canvas::<FolderWin>(|f, g, b| {
            let p = theme::palette();
            let at = |r: Bounds<Pixels>| Bounds { origin: r.origin + b.origin, size: r.size };
            if let Some(r) = f.icon_outline() {
                items::outline(g, at(r), &p);
            }
            if let Some(r) = f.icon_band() {
                style::selection_band(g, at(r), &p);
            }
        })
        .key_by::<FolderWin>(move |f| style::key((bits(f.icon_outline()), bits(f.icon_band()))))
        .absolute()
        .inset_0();
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
                    .on_drag_move(cx.listener(|f: &mut FolderWin, e: &MouseMoveEvent, cx| {
                        if f.press.is_some() {
                            f.icon_move(e, cx);
                        }
                    }))
                    .on_window_mouse_up(cx.listener(|f: &mut FolderWin, e: &MouseUpEvent, cx| f.icon_release(e, cx)))
                    .child(grid)
                    .child(over)
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
        let cols = list_columns();
        // Names for typing to select, made again only when the items change.
        if self.names.0 != self.stamp {
            self.names = (self.stamp, self.entries.iter().map(|e| SharedString::from(e.name.clone())).collect());
        }
        let names = self.names.1.clone();
        if self.editing.as_ref().is_some_and(|r| !self.entries.iter().any(|e| e.path == r.path)) {
            self.editing = None; // the item went away
        }
        // Editing a name: the field over its name, inside the cell (a
        // longer name scrolls in it), as the name cell lays it out.
        let field = self.editing.as_ref().and_then(|r| {
            let row = self.entries.iter().position(|e| e.path == r.path)?;
            let e = &self.entries[row];
            let cell = self.list_model.cell_bounds(&cols, row, 0);
            let inset = px(m::CELL_INSET - m::TIGHT);
            let name = Bounds { origin: point(inset, px(0.0)), size: size(cell.size.width - inset, cell.size.height) };
            let l = items::layout_item(name, e.label(), items::Look::Mini, e.is_link);
            Some((row, r.element_within(cx, &l, Point::default(), Some(cell.size.width - px(1.0)))))
        });
        // What something dragged over it would go into: a folder's row
        // framed, or the rows outlined (the folder shown), as the other
        // views show it; drawn by the list, over its rows.
        self.list_model.mark_drop(self.dropping.map(|(_, on)| on));
        let list = ColumnListView::new(cx, "list", &self.list_model, cols, self.entries.len(), |f: &FolderWin, cell, g| {
            let Some(e) = f.entries.get(cell.row) else { return };
            let editing = f.editing.as_ref().is_some_and(|r| r.path == e.path);
            match cell.column {
                0 => name_cell(cell, g, e, !editing),
                1 => cell.text(g, e.size_text_within(f32::from(cell.bounds.size.width) - 2.0 * m::CELL_INSET)),
                2 => cell.text(g, e.shown().modified.clone()),
                _ => cell.text(g, e.shown().kind.clone()),
            }
        })
        // (A row is drawn again only when what it shows changes: a step
        // of the selection draws two rows, not every one showing.)
        .row_key(|f: &FolderWin, row| f.entries.get(row).map_or(0, |e| style::key((row_key(e), f.editing.as_ref().is_some_and(|r| r.path == e.path)))))
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
                f.open(&e, cx);
            }
        }))
        .on_context(cx.listener(|f: &mut FolderWin, (_, at): &(usize, Point<Pixels>), cx| f.context_menu(true, *at, cx)))
        .on_drag(cx.listener(|f: &mut FolderWin, row: &usize, cx| {
            let paths: Vec<PathBuf> = f.list_model.state().selected.iter().filter_map(|&i| f.entries.get(i)).map(|e| e.path.clone()).collect();
            let Some(e) = f.entries.get(*row).cloned() else { return };
            let d = items::Drag { paths, icon: items::icon_of(&e), size: items::MINI, hotspot: point(px(items::MINI / 2.0), px(items::MINI / 2.0)), ask: false, name: e.name.clone() };
            items::start(cx, f.id, d, || FolderMsg::DragOver);
        }));
        match field {
            Some((row, field)) => list.over_cell(row, 0, field).into_any_element(),
            None => list.into_any_element(),
        }
    }

    // ---- the window --------------------------------------------------------

    /// The Navigator bar directly under the menu bar (View ▸ Show Navigator):
    /// Back, Forward, and Up bevelled 22 × 20 pt buttons with unified BeOS/Haiku
    /// shafted arrows, plus a sunken path well 6 pt to their right (showing the
    /// live host status light and bold host chip for `rex://HOST/PATH` folders,
    /// with a Refresh button 6 pt past the well on remote folders).
    fn navigator_bar(&self, cx: &mut Cx<Self>) -> Element {
        let loc = self.location();
        let remote = fs::rex_path(&loc);
        let can_back = self.can_go(-1);
        let can_fwd = self.can_go(1);
        let is_remote = remote.is_some();
        let remote_state = self.remote_state.clone();
        let is_connecting =
            matches!(remote_state, Some(crate::rex::HostState::Connecting | crate::rex::HostState::Reconnecting)) || (is_remote && self.loading && self.error.is_none() && remote_state.is_none());
        let is_disconnected = is_remote && !is_connecting && !matches!(remote_state, Some(crate::rex::HostState::Connected));
        let can_up = !is_disconnected && fs::parent_of(&loc).is_some();
        let can_refresh = !is_connecting && !self.loading;

        let nav_button = |btn: NavBtn, label: &'static str, tip: &'static str, enabled: bool, cmd: &'static str, cx: &mut Cx<Self>| -> Element {
            let hovered = enabled && self.nav_hover == Some(btn);
            let pressed = enabled && self.nav_press == Some(btn);
            let el = canvas(move |g, b| {
                let p = theme::palette();
                let bg = p.panel_background;
                let dark = p.mode == olo_theme::ThemeMode::Dark;
                let own_dim = !enabled && dark;
                let fade = |c: Hsla| if own_dim { c.opacity(DISABLED_OPACITY) } else { c };
                let base = p.control_background;
                // Unified BeOS push-button geometry matching `widgets::Button`:
                // light enabled buttons sit between white and the bar fill so pure
                // white is reserved for sunken wells; dark disabled buttons keep
                // their frame and fade control_background instead of looking sunken.
                let (top, bottom) = if !enabled && !dark {
                    let flat = style::c(mix(bg, base, 0.28));
                    (flat, flat)
                } else if pressed {
                    (style::shade(&p, base, 1.2), style::shade(&p, base, 1.12))
                } else if hovered {
                    if !dark { (style::c(mix(base, bg, 0.05)), style::c(mix(base, bg, 0.35))) } else { (style::tint(base, 0.7), style::shade(&p, base, 1.02)) }
                } else if !dark {
                    (style::c(mix(base, bg, 0.14)), style::c(mix(base, bg, 0.52)))
                } else {
                    (style::tint(base, 0.85), style::shade(&p, base, 1.06))
                };
                let border = if enabled || own_dim { style::shade(&p, bg, 1.6) } else { style::shade(&p, bg, 1.3) };
                let inner = style::inset(g, b, 1.0);
                style::vgradient(g, inner, fade(top), fade(bottom));
                style::frame_rect(g, b, border);
                if pressed {
                    style::bevel(g, inner, fade(style::shade(&p, base, 1.35)), fade(style::shade(&p, base, 1.1)));
                } else if enabled || own_dim {
                    style::bevel(g, inner, fade(style::c(p.shine)), fade(style::shade(&p, base, 1.2)));
                }

                let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
                let scale = g.scale_factor();
                // Unified stroke across all four glyphs: 1px at 1×, 2px at 1.5×, 3px at 2×.
                let sw_px = (1.35 * scale).round().max(1.0);
                let sw = px(sw_px / scale);
                let align_axis = |v: Pixels| -> Pixels {
                    let p_val = (f32::from(v) * scale).round();
                    let snapped = if (sw_px as i32) % 2 == 1 { p_val + 0.5 } else { p_val };
                    px(snapped / scale)
                };
                let off = if pressed { style::hairline(g) } else { px(0.0) };
                let cx_pt = align_axis(b.center().x + off);
                let cy_pt = align_axis(b.center().y + off);
                match btn {
                    NavBtn::Back => {
                        let mut path = PathBuilder::stroke(sw);
                        path.move_to(point(cx_pt + px(3.5), cy_pt));
                        path.line_to(point(cx_pt - px(3.5), cy_pt));
                        path.move_to(point(cx_pt - px(0.5), cy_pt - px(3.0)));
                        path.line_to(point(cx_pt - px(3.5), cy_pt));
                        path.line_to(point(cx_pt - px(0.5), cy_pt + px(3.0)));
                        if let Ok(path) = path.build() {
                            g.paint_path(path, ink);
                        }
                    }
                    NavBtn::Forward => {
                        let mut path = PathBuilder::stroke(sw);
                        path.move_to(point(cx_pt - px(3.5), cy_pt));
                        path.line_to(point(cx_pt + px(3.5), cy_pt));
                        path.move_to(point(cx_pt + px(0.5), cy_pt - px(3.0)));
                        path.line_to(point(cx_pt + px(3.5), cy_pt));
                        path.line_to(point(cx_pt + px(0.5), cy_pt + px(3.0)));
                        if let Ok(path) = path.build() {
                            g.paint_path(path, ink);
                        }
                    }
                    NavBtn::Up => {
                        let mut path = PathBuilder::stroke(sw);
                        path.move_to(point(cx_pt, cy_pt + px(3.5)));
                        path.line_to(point(cx_pt, cy_pt - px(3.5)));
                        path.move_to(point(cx_pt - px(3.0), cy_pt - px(0.5)));
                        path.line_to(point(cx_pt, cy_pt - px(3.5)));
                        path.line_to(point(cx_pt + px(3.0), cy_pt - px(0.5)));
                        if let Ok(path) = path.build() {
                            g.paint_path(path, ink);
                        }
                    }
                    NavBtn::Refresh => {
                        // 300° clockwise arc opening at top-right (2 o'clock → 12 o'clock),
                        // with the exact same open chevron head as ← → ↑ at 12 o'clock
                        // pointing clockwise (+X) so it reads unmistakably as Refresh at 1×–2×.
                        let r = 3.5_f32;
                        let a_start = -std::f32::consts::FRAC_PI_6; // 2 o'clock (-30°)
                        let a_end = 1.5 * std::f32::consts::PI; // 12 o'clock (270°)
                        let mut arc = PathBuilder::stroke(sw);
                        let steps = 20;
                        for s in 0..=steps {
                            let a = a_start + (s as f32 / steps as f32) * (a_end - a_start);
                            let pt = point(cx_pt + px(r * a.cos()), cy_pt + px(r * a.sin()));
                            if s == 0 {
                                arc.move_to(pt);
                            } else {
                                arc.line_to(pt);
                            }
                        }
                        // Open chevron centered on the 12 o'clock stroke endpoint pointing right.
                        let tx = cx_pt + px(0.5);
                        let ty = cy_pt - px(r);
                        arc.move_to(point(tx - px(2.2), ty - px(2.2)));
                        arc.line_to(point(tx, ty));
                        arc.line_to(point(tx - px(2.2), ty + px(2.2)));
                        if let Ok(path) = arc.build() {
                            g.paint_path(path, ink);
                        }
                    }
                }
                olo_kit::agent::record(g, olo_kit::agent::Node { role: "Button", name: Some(label.into()), bounds: b, enabled, clickable: enabled, ..Default::default() });
            })
            .w(px(NAV_BTN_W))
            .h(px(NAV_BTN_H))
            .cursor(if enabled { CursorStyle::PointingHand } else { CursorStyle::Arrow })
            .on_hover(cx.listener(move |f: &mut FolderWin, over: &bool, cx| {
                let next = if *over { Some(btn) } else { f.nav_hover.filter(|&h| h != btn) };
                if next != f.nav_hover {
                    f.nav_hover = next;
                    cx.invalidate();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |f: &mut FolderWin, _: &MouseDownEvent, cx| {
                    if enabled {
                        f.nav_press = Some(btn);
                        cx.invalidate();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |f: &mut FolderWin, _: &MouseUpEvent, cx| {
                    if f.nav_press.take() == Some(btn) && enabled {
                        f.run(cmd, cx);
                    } else {
                        cx.invalidate();
                    }
                }),
            );
            olo_kit::tip::on(el, format!("nav-{cmd}"), tip)
        };

        let shown_path = fs::display_path(&loc);
        let path_tip: SharedString = match &remote {
            Some(rex) => {
                let status = if is_connecting {
                    "Connecting…"
                } else if is_disconnected {
                    "Not connected"
                } else {
                    "Connected"
                };
                format!("{} — {} · Go to File or Folder (⇧⌘G)", rex.host, status).into()
            }
            None => "Go to File or Folder (⇧⌘G)".into(),
        };
        let path_well = olo_kit::tip::on(
            canvas(move |g, b| {
                let p = theme::palette();
                let line = style::hairline(g);
                style::fill_rect(g, b, style::c(p.document_background));
                // Inside the 2-hairline BeOS sunken bevel.
                let cavity = Bounds::from_corners(point(b.left() + line * 2.0, b.top() + line * 2.0), point(b.right() - line * 2.0, b.bottom() - line * 2.0));
                let font = style::ui_font(FontWeight::NORMAL);
                let cy = cavity.center().y;
                // Shared exact baseline for bold host chip & regular path, lifted slightly
                // so descenders (g, p) have comfortable clearance above the bottom bevel.
                let baseline = style::snap_coord(g, cy + px(style::SMALL_SIZE * 0.30));
                // Keep path text in primary document colour (clicking opens Go to Folder even
                // when disconnected); the host chip's hollow red ring + secondary host name
                // clearly carries the disconnected state without looking like placeholder text.
                let doc_ink = style::c(p.document_text);
                if let Some(rex) = &remote {
                    let bold = style::ui_font(FontWeight::BOLD);
                    let dot_d = 6.0_f32;
                    let pad_l = 5.5_f32;
                    let dot_gap = 4.0_f32;
                    let pad_r = 6.5_f32;
                    let max_host_w = (f32::from(cavity.size.width) * 0.40 - (pad_l + dot_d + dot_gap + pad_r)).max(24.0);
                    let host_fit = style::fit_with(&olo_kit::thread_text_system(), SharedString::from(rex.host.clone()), style::SMALL_SIZE, &bold, px(max_host_w));
                    let host_w = f32::from(style::text_width(host_fit.clone(), style::SMALL_SIZE, bold.clone()));
                    let badge_w = (pad_l + dot_d + dot_gap + host_w + pad_r).min(f32::from(cavity.size.width) - 24.0).max(24.0);
                    let div_x = style::snap_coord(g, cavity.left() + px(badge_w));
                    // Distinct raised-bar chip fill in both light and dark themes so the host
                    // reads clearly as a fixed prefix segment against the recessed path field.
                    let chip_bg = if p.mode == olo_theme::ThemeMode::Dark { mix(p.panel_background, p.control_background, 0.55) } else { p.panel_background };
                    let badge_rect = Bounds::from_corners(cavity.origin, point(div_x, cavity.bottom()));
                    style::fill_rect(g, badge_rect, style::c(chip_bg));
                    let div_color = if p.mode == olo_theme::ThemeMode::Dark { style::c(mix(p.panel_text, chip_bg, 0.72)) } else { style::shade(&p, p.panel_background, 1.3) };
                    style::vline(g, div_x, cavity.top(), cavity.bottom(), div_color);

                    let dot_center = style::snap_centred(g, point(cavity.left() + px(pad_l + dot_d / 2.0), cy), dot_d, dot_d).center();
                    let r_out = dot_d / 2.0;
                    if is_connecting {
                        g.paint_sector(dot_center, px(0.0), px(r_out), 0.0, std::f32::consts::TAU, style::c(p.warning));
                    } else if is_disconnected {
                        // Hollow red ring (not colour alone) so disconnected reads clearly even
                        // without colour vision; stroke snaps to >= 1px at 1× with open centre.
                        let stroke_pt = (1.4 * g.scale_factor()).round().max(1.0) / g.scale_factor();
                        g.paint_sector(dot_center, px((r_out - stroke_pt).max(0.5)), px(r_out), 0.0, std::f32::consts::TAU, style::c(p.failure));
                    } else {
                        g.paint_sector(dot_center, px(0.0), px(r_out), 0.0, std::f32::consts::TAU, style::c(p.success));
                    }
                    let host_ink = if is_disconnected { style::c(olo_theme::readable(mix(p.panel_text, chip_bg, 0.35), chip_bg, 4.8)) } else { style::c(p.panel_text) };
                    let host_x = cavity.left() + px(pad_l + dot_d + dot_gap);
                    style::label_on_baseline(g, host_x, baseline, Align::Left, host_fit, style::SMALL_SIZE, bold, host_ink);

                    let path_x = div_x + line + px(6.5);
                    let avail = (cavity.right() - px(6.0) - path_x).max(px(0.0));
                    paint_path_tail(g, path_x, baseline, &rex.path, style::SMALL_SIZE, font, doc_ink, avail);
                } else {
                    let path_x = cavity.left() + px(5.5);
                    let avail = (cavity.right() - px(6.0) - path_x).max(px(0.0));
                    paint_path_tail(g, path_x, baseline, &shown_path, style::SMALL_SIZE, font, doc_ink, avail);
                }
                style::sunken(g, b, &p, false);
                olo_kit::agent::record(g, olo_kit::agent::Node { role: "Button", name: Some(format!("Path: {shown_path}").into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
            })
            .w_full()
            .h(px(NAV_BTN_H))
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|f: &mut FolderWin, _: &MouseDownEvent, cx| {
                    f.run("go-to", cx);
                }),
            ),
            "nav-path",
            path_tip,
        );

        let bg = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
            let y = b.bottom() - style::hairline(g);
            style::hline(g, b.left(), b.right(), y, style::shade(&p, p.panel_background, 1.3));
        })
        .absolute()
        .inset_0();

        let mut row = div()
            .relative()
            .w_full()
            .h(px(NAV_HEIGHT))
            .px(px(m::CELL_INSET))
            .flex()
            .flex_row()
            .items_center()
            .child(bg)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .flex_shrink_0()
                    .child(nav_button(NavBtn::Back, "Back", "Back", can_back, "back", cx))
                    .child(nav_button(NavBtn::Forward, "Forward", "Forward", can_fwd, "forward", cx))
                    .child(nav_button(NavBtn::Up, "Up", "Open Parent", can_up, "parent", cx)),
            )
            .child(div().w(px(6.0)).flex_shrink_0())
            .child(div().flex_1().min_w_0().child(path_well));
        if is_remote {
            row = row.child(div().w(px(6.0)).flex_shrink_0()).child(nav_button(NavBtn::Refresh, "Refresh", "Refresh (⌘R)", can_refresh, "reload", cx));
        }
        row.into_any_element()
    }

    /// Designed empty/error state shown inside the document canvas when a remote
    /// host isn't connected, Rex isn't running, or a folder can't be read.
    fn error_view(&self, err: &str, cx: &mut Cx<Self>) -> Element {
        let rex = fs::rex_path(&self.path);
        let is_rex_stopped = matches!(self.remote_state, Some(crate::rex::HostState::RexNotRunning)) || err.contains("Rex isn");
        let is_remote_conn =
            is_rex_stopped || (rex.is_some() && matches!(self.remote_state, Some(crate::rex::HostState::Disconnected | crate::rex::HostState::Error(_) | crate::rex::HostState::Unknown)));
        let (title, body): (String, String) = if is_rex_stopped {
            ("Rex isn’t running".to_owned(), "Open Rex to browse files on remote hosts.".to_owned())
        } else if let Some(rp) = &rex
            && is_remote_conn
        {
            (format!("{} isn’t connected", rp.host), "Connect it in Rex to browse its files.".to_owned())
        } else if let Some(rp) = &rex {
            (format!("Couldn’t open folder on {}", rp.host), err.to_owned())
        } else {
            ("Couldn’t open folder".to_owned(), err.to_owned())
        };
        let is_remote = rex.is_some();

        let icon = canvas(move |g, b| {
            let p = theme::palette();
            let icon_rect = style::snap_centred(g, b.center(), 32.0, 32.0);
            olo_kit::icons::paint(g, icon_rect, if is_remote { "rex" } else { "folder" });
            if is_remote_conn {
                // Unplugged red badge with crisp white × at bottom-right of the 32pt icon.
                let badge_c = style::snap_centred(g, point(icon_rect.right() - px(4.0), icon_rect.bottom() - px(4.0)), 12.0, 12.0).center();
                g.paint_sector(badge_c, px(0.0), px(6.5), 0.0, std::f32::consts::TAU, style::c(p.document_background));
                g.paint_sector(badge_c, px(0.0), px(5.5), 0.0, std::f32::consts::TAU, style::c(p.failure));
                style::close_mark(g, badge_c, 5.0, style::c([255, 255, 255, 255]));
            }
        })
        .w(px(40.0))
        .h(px(40.0));

        let title_el = {
            let t = title.clone();
            canvas(move |g, b| {
                let p = theme::palette();
                let at = style::label_fit(
                    g,
                    b.center().x,
                    b.center().y,
                    Align::Center,
                    t.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::BOLD),
                    style::c(p.document_text),
                    (b.size.width - px(24.0)).max(px(0.0)),
                );
                olo_kit::agent::record(g, olo_kit::agent::Node::label(t.clone(), at));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        };

        let body_el = {
            let msg = body.clone();
            canvas(move |g, b| {
                let p = theme::palette();
                // Readable secondary contrast (>= 4.8:1) on document_background in both themes.
                let ink = style::c(olo_theme::readable(mix(p.document_text, p.document_background, 0.42), p.document_background, 4.8));
                let at =
                    style::label_fit(g, b.center().x, b.center().y, Align::Center, msg.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, (b.size.width - px(24.0)).max(px(0.0)));
                olo_kit::agent::record(g, olo_kit::agent::Node::label(msg.clone(), at));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        };

        let retry = cx.listener(|f: &mut FolderWin, _: &ClickEvent, cx| {
            f.run("reload", cx);
        });
        let buttons = if is_remote_conn {
            div().flex().flex_row().items_center().gap(px(m::SPACING + 2.0)).child(Button::new("Try Again").on_click(retry)).child(Button::new("Open Rex").default(true).on_click(|_, _| {
                if let Some(bin) = olo_xdg::find_program("olo-rex") {
                    let _ = std::process::Command::new(bin).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
                }
            }))
        } else {
            div().flex().flex_row().items_center().child(Button::new("Try Again").default(true).on_click(retry))
        };

        let bg = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
        })
        .absolute()
        .inset_0();

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .child(bg)
            .child(div().flex_1())
            .child(div().w_full().flex().flex_col().items_center().child(icon).child(div().h(px(8.0))).child(title_el).child(div().h(px(4.0))).child(body_el).child(div().h(px(12.0))).child(buttons))
            .child(div().flex_1())
            .child(div().h(px(32.0)).flex_shrink_0())
            .into_any_element()
    }

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
        let is_connecting = matches!(self.remote_state, Some(crate::rex::HostState::Connecting | crate::rex::HostState::Reconnecting));
        let (count, muted_count) = if self.error.is_some() {
            let status = if is_connecting {
                "Connecting…".to_owned()
            } else if fs::is_remote(&self.path) {
                "Not connected".to_owned()
            } else {
                String::new()
            };
            (status, true)
        } else if loading {
            let found = self.listed.load(std::sync::atomic::Ordering::Relaxed);
            let label = if is_connecting {
                "Connecting…".to_owned()
            } else if found == 0 {
                "Loading…".to_owned()
            } else {
                format!("Loading… {found} items")
            };
            (label, true)
        } else {
            let text = match (n, list.selected.len()) {
                (1, 0) => "1 item".to_owned(),
                (n, 0) => format!("{n} items"),
                (n, s) => olo_kit::tr!("{} of {} selected", s, n),
            };
            (text, false)
        };
        let free = if self.error.is_some() { None } else { self.free_space() };
        // (The list view's own sunken frame edges it: an etched edge
        // under that one too crowded the two, a line apart.)
        let etched = self.view != ViewMode::List || self.error.is_some();
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
            // Readable secondary ink (>= 4.8:1 in both light and dark modes).
            let quiet_ink = style::c(olo_theme::readable(mix(p.panel_text, p.panel_background, 0.38), p.panel_background, 4.8));
            if !count.is_empty() {
                let ink = if muted_count { quiet_ink } else { style::c(p.panel_text) };
                // In line with the list's first column (its icons) above.
                let at = style::label(g, b.left() + px(m::CELL_INSET), cy, Align::Left, count.clone(), style::SMALL_SIZE, font.clone(), ink);
                olo_kit::agent::record(g, olo_kit::agent::Node::label(count.clone(), at));
            }
            if let Some(free) = free {
                let text = format!("{} free", fs::format_size(free));
                let at = style::label(g, b.right() - px(m::CELL_INSET), cy, Align::Right, text.clone(), style::SMALL_SIZE, font, quiet_ink);
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
    /// not every frame; remote folders use what the host's last list reported).
    fn free_space(&self) -> Option<u64> {
        let dir = self.current_dir();
        if fs::is_remote(&dir) {
            return self.remote_free;
        }
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
        if !self.icon_geo().view.contains(&at) {
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

    /// What's dragged over the list view at `at`: onto a folder's row, or
    /// (anywhere else over the rows) into the folder shown, as the icon
    /// view takes it. (The list view took no drops at all: a file dragged
    /// onto a folder there, or in from another window, was refused.)
    fn list_drop_over(&mut self, o: &DropOffer, at: Point<Pixels>, cx: &mut Cx<Self>) -> Option<DropAccept> {
        let Some(row) = self.list_model.row_under(at) else {
            self.dropping = None;
            self.rest_on(None, cx);
            return None;
        };
        let over = row.and_then(|i| self.entries.get(i).map(|e| (i, e, self.list.selected.contains(&i))));
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
        if let Err(e) = crate::dropped_into_owned(drop, paths, dest, Some(self.id), cx) {
            self.error(e, cx);
        }
    }

    // ---- commands ------------------------------------------------------------

    /// A command from the menus, a context menu, a key, an agent.
    pub fn run(&mut self, cmd: &str, cx: &mut Cx<Self>) {
        let (selected, entries, dir) = (self.selected_paths(), self.selected_entries(), self.current_dir());
        let job = |kind: ops::Kind, sources: Vec<PathBuf>, id: u64| to_hub(ToHub::Job { kind, sources, dest: None, origin: Some(id) });
        if (cmd.starts_with("compress") || cmd.starts_with("extract")) && crate::archives::command(cmd, selected.clone(), dir.clone(), self.id) {
            return;
        }
        match cmd {
            "quick-look" => to_hub(ToHub::QuickLook(self.id)),
            // (One folder: there, in this window. Several: a window each, as
            // one window can't be in several places; files by their apps,
            // together, so links that can't be opened are said once.)
            // (Links, to folders too, looked at as they are now on a worker,
            // which brings a folder one back here: `fs::open_files`.)
            "open" => {
                let (folders, files): (Vec<&Entry>, Vec<&Entry>) = entries.iter().partition(|e| e.is_dir && !e.is_link);
                let all_folders = entries.iter().filter(|e| e.is_dir).count();
                for e in &folders {
                    if all_folders > 1 {
                        to_hub(ToHub::Open { path: e.path.clone(), from: Some(self.path.clone()) });
                    } else {
                        self.open(e, cx);
                    }
                }
                let from = fs::OpenedFrom { win: self.id, path: self.path.clone(), folders: folders.len() };
                fs::open_files(&files.into_iter().cloned().collect::<Vec<_>>(), Some(from));
            }
            "open-new-window" => {
                for e in entries.iter().filter(|e| e.is_dir) {
                    to_hub(ToHub::Open { path: e.path.clone(), from: Some(self.path.clone()) });
                }
            }
            // The folder it's in, with this one selected (to go on from),
            // in this window.
            "parent" => {
                let loc = self.location();
                if let Some(up) = fs::parent_of(&loc) {
                    self.go_to(up, Some(loc), cx);
                }
            }
            "new-folder" => {
                if let Some(rp) = fs::rex_path(&dir) {
                    let to = cx.messenger();
                    let dir_clone = dir.clone();
                    std::thread::spawn(move || {
                        let result = crate::rex::mkdir(&rp.join("New Folder"), true).map(|(p, e)| (p.to_path_buf(), e));
                        to.send(FolderMsg::Created { dir: dir_clone, result });
                    });
                } else {
                    match fs::new_folder(&dir) {
                        Ok(p) => {
                            to_hub(ToHub::Done(olo_kit::words::tr("New Folder").into(), undo::Change::Made(vec![p.clone()])));
                            self.add_entry(&p);
                            self.edit_name(&p, cx);
                        }
                        Err(e) => self.error(olo_kit::tr!("Couldn't create a folder: {}", e), cx),
                    }
                }
            }
            "reload" => self.reload(self.hidden),
            "navigator" => to_hub(ToHub::ToggleNavigator),
            "terminal" => {
                if !fs::is_remote(&dir) {
                    fs::open_in_terminal(&dir);
                }
            }
            "rex" => fs::open_in_rex(&dir),
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
                for e in entries.iter().filter(|e| e.is_dir && !fs::is_remote(&e.path)) {
                    fs::open_in_terminal(&e.path);
                }
            }
            "rex-selected" => {
                for e in entries.iter().filter(|e| e.is_dir) {
                    fs::open_in_rex(&e.path);
                }
            }
            "older-versions" => {
                if let Some(e) = entries.iter().find(|e| !e.is_dir && !fs::is_remote(&e.path)) {
                    crate::backups::older_versions(&e.path);
                }
            }
            "info" => {
                if entries.is_empty() {
                    if fs::is_remote(&dir) {
                        std::thread::spawn(move || {
                            if let Some(e) = fs::entry(&dir) {
                                to_hub(ToHub::GetInfo(vec![e]));
                            }
                        });
                    } else {
                        to_hub(ToHub::GetInfo(fs::entry(&dir).into_iter().collect()));
                    }
                } else {
                    to_hub(ToHub::GetInfo(entries));
                }
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
                if !fs::is_remote(&dir) {
                    let made: Vec<PathBuf> = selected.iter().filter_map(|p| fs::make_link(p).ok()).collect();
                    to_hub(ToHub::Done(if made.len() == 1 { "Create Link" } else { "Create Links" }.into(), undo::Change::Made(made)));
                    to_hub(ToHub::ReloadAll);
                }
            }
            "undo" => to_hub(ToHub::UndoRedo(false, Some(self.id))),
            "redo" => to_hub(ToHub::UndoRedo(true, Some(self.id))),
            "trash" => {
                if !selected.is_empty() && !fs::is_remote(&dir) {
                    job(ops::Kind::Trash, selected, self.id);
                }
            }
            "remote-delete" => {
                if !selected.is_empty()
                    && let Some(rp) = fs::rex_path(&dir)
                {
                    let what = match selected.as_slice() {
                        [one] => format!("“{}”", fs::leaf_name(one)),
                        many => format!("{} items", many.len()),
                    };
                    let msg = olo_kit::tr!("Permanently delete {} on {}? This cannot be undone.", what, rp.host);
                    let id = self.id;
                    let _ = open_alert(cx, "Delete", "trash-full", &msg, &["Cancel", "Delete"], move |choice, _| {
                        if *choice == Some(1) {
                            to_hub(ToHub::Job { kind: ops::Kind::Delete, sources: selected.clone(), dest: None, origin: Some(id) });
                        }
                    });
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
            // (The Go menu's places: there, in this window.)
            "go-home" => self.go_to(fs::home(), None, cx),
            "go-desktop" => self.go_to(fs::home().join("Desktop"), None, cx),
            "go-root" => self.go_to(PathBuf::from("/"), None, cx),
            "go-trash" => {
                let t = fs::trash_files();
                let _ = std::fs::create_dir_all(&t);
                self.go_to(t, None, cx);
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
                    self.go_to(dir, None, cx);
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
    fn pasted(&mut self, dir: &Path, mime: &str, data: Vec<u8>, cx: &mut Cx<Self>) {
        // Check MIME BEFORE calling `copied_files` so large binary/image pastes
        // never incur a lossy UTF-8 allocation on Tracker's UI thread.
        if matches!(mime, "x-special/gnome-copied-files" | "text/uri-list")
            && let Some((paths, cut)) = olo_kit_base::clipboard::copied_files(mime, &data)
        {
            // Nothing goes into itself.
            if let Some(p) = paths.iter().find(|p| (fs::is_remote(p) || p.is_dir()) && fs::starts_with_dir(dir, p)) {
                let name = fs::leaf_name(p);
                return self.error(olo_kit::tr!("“{}” can't go inside itself.", name), cx);
            }
            if cut {
                // Moved: the clipboard has them where they are now, copied.
                let moved: Vec<PathBuf> = paths.iter().map(|p| fs::join_name(dir, &fs::leaf_name(p))).collect();
                crate::copy_clip(olo_kit_base::clipboard::files(&moved, false));
            }
            let kind = if cut { ops::Kind::Move } else { ops::Kind::Copy };
            return to_hub(ToHub::Job { kind, sources: paths, dest: Some(dir.to_owned()), origin: Some(self.id) });
        }
        if fs::is_remote(dir) {
            let Some(save) = crate::prepare_remote_paste(mime, data) else { return };
            return to_hub(ToHub::RemoteSave { dir: dir.to_owned(), save, origin: Some(self.id) });
        }
        let saved = if let Some(kind) = mime.strip_prefix("image/") {
            fs::pasted_picture(dir, kind, &data)
        } else if mime.starts_with("text/") || mime == "UTF8_STRING" {
            let text = String::from_utf8_lossy(&data);
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
            Err(e) => self.error(olo_kit::tr!("Couldn't paste: {}", e), cx),
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
    if e.broken {
        olo_kit::icons::paint_faded(g, icon, items::icon_of(e), 0.5);
        items::paint_broken(g, icon);
    } else {
        olo_kit::icons::paint(g, icon, items::icon_of(e));
    }
    if crate::backups::backed_up(&e.path) {
        items::paint_mark(g, icon, e.label(), &theme::palette());
    }
    let rest = Bounds::from_corners(point(icon.right() + px(m::MARK_GAP - m::CELL_INSET), b.top()), b.bottom_right());
    let saved = cell.bounds;
    cell.bounds = rest;
    if text {
        let color = cell.text_color;
        cell.text_with(g, e.shown().name.clone(), items::name_font(e.is_link), color);
    } else {
        cell.say(e.shown().name.clone());
    }
    cell.bounds = saved;
}

/// What a row of a list of entries shows of `e`, as a number that changes
/// when it does (`ColumnListView::row_key`): its name, icon, mark and
/// columns.
pub fn row_key(e: &Entry) -> u64 {
    style::key((e.look(), items::icon_of(e), crate::backups::backed_up(&e.path)))
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
            FolderMsg::Listed { path, generation, result, remote_free, host_state, resolved } => {
                self.listed(&path, generation, result, remote_free, host_state, resolved);
            }
            FolderMsg::Created { dir, result } => match result {
                Ok((p, entry)) => {
                    if self.current_dir() == dir {
                        if let Some(e) = entry {
                            self.insert_entry(e);
                        }
                        self.edit_name(&p, cx);
                    }
                    to_hub(ToHub::ReloadAll);
                }
                Err(e) => self.error(olo_kit::tr!("Couldn't create a folder: {}", e), cx),
            },
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
            FolderMsg::GoTo(path) => self.go_to(path, None, cx),
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
                    self.pasted(&dir, &mime, data, cx);
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
            (None, _) => {
                self.dropping = None;
                self.rest_on(None, cx);
                None
            }
            (Some(o), ViewMode::Columns) => self.columns_drop_over(o, at, cx),
            (Some(o), ViewMode::List) if self.error.is_none() => self.list_drop_over(o, at, cx),
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
        let loc = self.location();
        if !self.path_said || self.shown_loc != loc {
            self.path_said = true;
            self.shown_loc = loc.clone();
            olo_kit::agent::set_path(cx, Some(loc.clone()));
            cx.set_window_title(fs::display_name(&loc));
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
        let bar = MenuBar::of(self, cx, |v, _| v.menus(crate::can_paste(), &crate::places(), &crate::undo_names()));
        let nav = crate::show_navigator().then(|| self.navigator_bar(cx));
        let body: Element = match (&self.error, self.view) {
            (Some(err), _) => self.error_view(err, cx),
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
            .children(nav)
            .child(div().flex_1().min_h_0().relative().flex().flex_col().child(body))
            .child(self.count_bar(cx))
            .into_any_element()
    }
}

/// Paint a filesystem path on `baseline` within `max_width`, truncating from the
/// start (`…/src/jato`) so the current folder name at the end stays visible.
#[allow(clippy::too_many_arguments)]
fn paint_path_tail(g: &mut Painter, x: Pixels, baseline: Pixels, path: &str, font_size: f32, font: olo_kit::Font, color: Hsla, max_width: Pixels) {
    let fits = |s: &str| style::text_width(s.to_owned(), font_size, font.clone()) <= max_width;
    if fits(path) {
        style::label_on_baseline(g, x, baseline, Align::Left, path.to_owned(), font_size, font, color);
        return;
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut chosen: Option<String> = None;
    for skip in 1..parts.len() {
        let candidate = format!("…/{}", parts[skip..].join("/"));
        if fits(&candidate) {
            chosen = Some(candidate);
            break;
        }
    }
    let shown = chosen.unwrap_or_else(|| {
        // Even the leaf component alone overflows: trim leading chars with `…`.
        let leaf = parts.last().copied().unwrap_or(path);
        let mut chars: Vec<char> = leaf.chars().collect();
        while !chars.is_empty() {
            let cand = format!("…{}", chars.iter().collect::<String>());
            if fits(&cand) {
                return cand;
            }
            chars.remove(0);
        }
        "…".to_owned()
    });
    let at = style::label_on_baseline(g, x, baseline, Align::Left, shown, font_size, font, color);
    g.note_cut(SharedString::from(path.to_owned()), at);
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

/// The list view's columns as they first are, together a few points
/// narrower than a new window's list (560 points less its frame and
/// scroll bar), so they don't scroll sideways there and Kind, the last,
/// fills what's left: at 580 together, "PDF Document" was cut off under
/// the scroll bar (QA, 2026-11-28). Modified holds the widest date
/// ("Nov 28, 2026, 12:59 PM"), Size the widest size ("1023.9 MB"; a
/// download's progress where it fits, `Entry::size_text_within`), Kind
/// in a new window every kind, "Olo Settings Backup" and "Downloading,
/// 99%" the widest (at 106, a review found them cut;
/// `list_columns_fit_a_new_window`).
pub fn list_columns() -> [Column; 4] {
    [Column::new("Name", 196.0), Column::new("Size", 74.0).right(), Column::new("Modified", 146.0), Column::new("Kind", 122.0)]
}

#[cfg(test)]
mod tests {
    use olo_kit::{FontWeight, metrics as m, style, widgets::scroll::SCROLL_BAR};

    use super::list_columns;

    /// A new window's list (560 points wide, less its sunken frame's
    /// two lines each side and its scroll bar) holds the columns without
    /// scrolling sideways, and each column what the formatters give it
    /// whole: every date and size, every kind (Kind with what's left of
    /// the window), and a download's progress as it is put to fit
    /// (`Entry::size_text_within`).
    #[test]
    fn list_columns_fit_a_new_window() {
        use crate::fs::{Entry, Kind, format_size};
        let cols = list_columns();
        let view = 560.0 - 2.0 * 2.0 - SCROLL_BAR;
        let total: f32 = cols.iter().map(|c| c.width).sum();
        assert!(total + 4.0 <= view, "the columns, {total} together, scroll sideways in a new window's list, {view} wide");
        let width = |text: &str| f32::from(style::text_width(text.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL)));
        // (Kind, the last, has what the others leave of the list.)
        let room = |col: usize| cols[col].width + if col == 3 { view - total } else { 0.0 } - 2.0 * m::CELL_INSET;
        let fits = |text: &str, col: usize| assert!(width(text) <= room(col), "“{text}” ({:.1} wide) doesn't fit {} ({})", width(text), cols[col].title, cols[col].width);
        fits("Nov 28, 2026, 12:59 PM", 2);
        fits("Sep 30, 2026, 10:59 PM", 2);
        for n in [1, 1023, 1023 * 1024 + 900 * 1024 / 1000, 1023 * 1024 * 1024 + 1000 * 1024, 999 << 40] {
            fits(&format_size(n), 1);
        }
        let kinds = [
            Kind::Folder,
            Kind::Text,
            Kind::Image,
            Kind::Audio,
            Kind::Video,
            Kind::Archive,
            Kind::Pdf,
            Kind::Application,
            Kind::Executable,
            Kind::Document,
            Kind::WordDocument,
            Kind::Spreadsheet,
            Kind::Presentation,
            Kind::Font,
            Kind::SettingsBackup,
        ];
        for kind in kinds.iter().map(|k| k.name()).chain(["Broken Link", "Downloading", "Downloading, 99%"]) {
            fits(kind, 3);
        }
        // A download: "3.1 of 10 MB" where the column holds it; in a new
        // window's, what's done so far, whole.
        let download = |done: u64, total: u64| Entry {
            path: "/f/big.iso.part".into(),
            name: "big.iso.part".into(),
            is_dir: false,
            is_link: false,
            broken: false,
            size: done,
            modified: None,
            mode: 0o644,
            kind: Kind::Archive,
            app_icon: None,
            downloading: Some(Some(total)),
            shown: Default::default(),
        };
        for (done, total) in [(3_250_000, 10_485_760), (1023, 1023), (999 << 20, 1023 << 20), (5 << 30, 9 << 30)] {
            let e = download(done, total);
            let shown = e.size_text_within(room(1));
            assert!(width(&shown) <= room(1), "“{shown}” doesn't fit Size");
            assert_eq!(e.size_text_within(200.0), e.size_text().as_str());
        }
        assert_eq!(download(3_250_000, 10_485_760).size_text_within(room(1)), "3.1 MB");
        assert_eq!(download(3_250_000, 10_485_760).size_text_within(200.0), "3.1 of 10 MB");
    }
}
