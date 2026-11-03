//! A folder window: its menu bar, the folder shown as icons, mini icons,
//! a list or columns, and the count along the bottom. What it can do
//! itself (selecting, the keyboard, dragging out, editing a name) it
//! does; what opens windows or touches files goes to the [`Tracker`].

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

use olo_kit_base::list::{ListState, Mods, SelectionBand, TypeSelect};
use olo_ui::{
    Menu, MenuBar,
    gpui::Focusable as _,
    gpui::{
        self, App, Bounds, Context, Entity, EntityId, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle, Styled as _, Task, WeakEntity, Window, canvas, div, point, px, size,
    },
    menu, metrics as m,
    style::{self, Align},
    theme,
    widgets::{
        SCROLL_BAR, ScrollView, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
    },
};

use crate::{
    Tracker, ViewMode, columns,
    fs::{self, Entry},
    items::{self, ItemLayout, Look, Rename, Spring},
    model::{Loader, Order, Pane, SortKey},
};

/// How long after a click on a selected item's name editing starts (a
/// double-click within it opens the item instead), as on BeOS.
const EDIT_DELAY: Duration = Duration::from_millis(600);
/// How far a press moves before it's a drag.
const DRAG_SLOP: f32 = 3.0;

/// What a folder window asks of the Tracker.
pub enum Action {
    Open(Entry),
    Command(String),
    /// A name edited in place was committed.
    Rename(PathBuf, String),
    /// A drag rested on a folder: open it, so the drag can go on in.
    Spring(PathBuf),
}

pub struct FolderWin {
    pub path: PathBuf,
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

    pub app: WeakEntity<Tracker>,
    /// The window's surface, as last drawn (which workspace it's on),
    /// and its size (kept for an upgrade).
    pub surface: Option<u32>,
    pub size: (u32, u32),
    pub focus: FocusHandle,
    scroll: ScrollHandle,
    geo: Rc<RefCell<IconGeo>>,
    press: Option<Press>,
    band: Option<(Point<Pixels>, SelectionBand)>,
    pub typing: TypeSelect,
    pub spring: Rc<RefCell<Spring>>,
    list_model: Entity<ListModel>,
    pub cols: columns::Cols,
    path_said: bool,
    /// The keyboard goes back to the view (a name's edit ended).
    focus_after_edit: bool,
    /// The view the keyboard was last given to.
    focused_view: Option<ViewMode>,
    /// Bumped whenever the items change (`names` follows it).
    pub stamp: u64,
    names: (u64, Rc<[olo_ui::gpui::SharedString]>),
    edit_timer: Option<Task<()>>,
    poll: Option<Task<()>>,
    /// The free space under the count, and when it was read.
    free: RefCell<Option<(Instant, PathBuf, Option<u64>)>>,
}

/// The icon view as last drawn: where its content is, how it's laid out,
/// and the items that showed (in the window's coordinates).
#[derive(Default)]
struct IconGeo {
    origin: Point<Pixels>,
    view: Bounds<Pixels>,
    cols: usize,
    cell: (f32, f32),
    inset: f32,
    items: Vec<(usize, ItemLayout)>,
}

impl IconGeo {
    fn hit(&self, p: Point<Pixels>) -> Option<usize> {
        self.items.iter().find(|(_, l)| l.hit().contains(&p)).map(|(i, _)| *i)
    }

    /// Item `i`'s cell, in the window.
    fn cell_at(&self, i: usize) -> Bounds<Pixels> {
        let (c, r) = (i % self.cols.max(1), i / self.cols.max(1));
        Bounds { origin: point(self.origin.x + px(self.inset + c as f32 * self.cell.0), self.origin.y + px(self.inset + r as f32 * self.cell.1)), size: size(px(self.cell.0), px(self.cell.1)) }
    }
}

/// An item that shows in the icon view this frame, laid out.
struct Shown {
    i: usize,
    layout: ItemLayout,
    icon: &'static str,
    selected: bool,
    /// How far along a download being written is.
    progress: Option<f32>,
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
    pub fn new(path: PathBuf, view: ViewMode, hidden: bool, app: WeakEntity<Tracker>, cx: &mut Context<Self>) -> Self {
        let loader = Loader { win: cx.entity_id(), proxy: olo_ui::proxy(cx) };
        let list_model = ListModel::new(cx, true);
        cx.observe(&list_model, |_, _, cx| cx.notify()).detach();
        let sort = crate::sorts::of(cx, &path);
        let mut w = FolderWin {
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
            app,
            surface: None,
            size: (560, 380),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            geo: Rc::default(),
            press: None,
            band: None,
            typing: TypeSelect::default(),
            spring: Rc::default(),
            list_model,
            cols: columns::Cols::default(),
            path_said: false,
            focus_after_edit: false,
            focused_view: None,
            stamp: 0,
            names: (u64::MAX, Rc::from(Vec::new())),
            edit_timer: None,
            poll: None,
            free: RefCell::new(None),
        };
        w.reload(hidden);
        w
    }

    pub fn key(&self) -> EntityId {
        self.loader.win
    }

    /// Hands `a` to the Tracker, after this update (it may change this
    /// window too).
    pub fn act(&self, a: Action, cx: &mut Context<Self>) {
        let (app, key) = (self.app.clone(), self.key());
        cx.defer(move |cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.folder_action(key, a, cx));
            }
        });
    }

    pub fn command(&self, cmd: &str, cx: &mut Context<Self>) {
        self.act(Action::Command(cmd.to_owned()), cx);
    }

    /// Ends editing a name: renamed to what's in the field (`commit`), or
    /// left as it was.
    pub fn end_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(r) = self.editing.take() else { return };
        if commit {
            let name = r.text(cx);
            self.act(Action::Rename(r.path, name), cx);
        }
        // The keyboard back to the view.
        self.focus_after_edit = true;
        cx.notify();
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
        // A folder opens in Terminal itself.
        if self.selected_entries().iter().all(|e| e.is_dir) {
            menu = menu.item("Open in Terminal", "terminal-selected");
        }
        if let Some(open_with) = open_with_menu(&self.selected_entries()) {
            menu = menu.submenu(open_with);
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
            .key("n")
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
    fn menus(&self, can_paste: bool, places: &olo_kit_base::places::Places) -> Vec<Menu> {
        let current = self.current().1;
        let has_sel = !current.selected.is_empty();
        let in_trash = self.current_dir().starts_with(fs::trash_files());
        let mut file = Menu::new("File")
            .item("Find…", "find")
            .key("f")
            .item("New Folder", "new-folder")
            .key("n")
            .enabled(!in_trash)
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
        let mut go = Menu::new("Go").item("Go to File or Folder…", "go-to").shift_key("g").separator();
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
    pub fn context_menu(&self, on_item: bool, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let can_paste = crate::can_paste(cx);
        let menu = if on_item { self.item_menu() } else { self.background_menu(can_paste) };
        let (app, key) = (self.app.clone(), self.key());
        menu::context_menu(window, cx, menu, at, move |cmd, _, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.folder_command(key, cmd, cx));
            }
        });
    }

    // ---- keyboard --------------------------------------------------------

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                self.command("open", cx);
                cx.stop_propagation();
            }
            return;
        }
        // Space is Quick Look, unless it goes on with a name being typed.
        let typing = match self.view {
            ViewMode::List => self.list_model.read(cx).typing(),
            _ => self.typing.typing(),
        };
        if k.key == "space" && !typing {
            self.command("quick-look", cx);
            cx.stop_propagation();
            return;
        }
        match k.key.as_str() {
            "delete" if plain_extra("trash", "delete") => {
                self.command("trash", cx);
                cx.stop_propagation();
                return;
            }
            "f2" if plain_extra("rename", "f2") => {
                self.command("rename", cx);
                cx.stop_propagation();
                return;
            }
            _ => {}
        }
        match self.view {
            // The list takes its own keys.
            ViewMode::List => {}
            ViewMode::Columns => {
                if self.column_key(e, window, cx) {
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
    fn icon_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let n = self.entries.len();
        if n == 0 {
            return false;
        }
        let k = &e.keystroke;
        let (cols, rows_in_view) = {
            let g = self.geo.borrow();
            (g.cols.max(1), ((f32::from(g.view.size.height) - 2.0 * g.inset) / g.cell.1.max(1.0)).floor().max(1.0) as isize)
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
                self.command("open", cx);
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
        self.reveal_icon(to, 0.0);
        cx.notify();
        true
    }

    /// Scrolls the icon view so item `i` shows, with `around` rows' room
    /// round it.
    fn reveal_icon(&self, i: usize, around: f32) {
        let g = self.geo.borrow();
        if g.cols == 0 {
            return;
        }
        let view = f32::from(g.view.size.height);
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
        self.scroll.set_offset(point(px(0.0), px(-to.clamp(0.0, max.max(0.0)))));
    }

    // ---- the icon views ----------------------------------------------------

    fn icon_press(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        window.focus(&self.focus);
        let pos = e.position;
        let (hit, on_label) = {
            let g = self.geo.borrow();
            let hit = g.hit(pos);
            (hit, hit.and_then(|i| g.items.iter().find(|(j, _)| *j == i)).is_some_and(|(_, l)| l.highlight().contains(&pos)))
        };
        let m = mods(&e.modifiers);
        if e.button == MouseButton::Left && e.click_count >= 2 {
            if let Some(i) = hit
                && let Some(entry) = self.entries.get(i).cloned()
            {
                self.pending_edit = None;
                self.press = None;
                self.act(Action::Open(entry), cx);
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
            let _ = olo_ui::icons::bitmap(items::icon_of(e), 0);
        }
        self.press = Some(Press { at: pos, button: e.button, item: hit, was_selected, on_label, mods: e.modifiers, dragging: false });
        cx.notify();
    }

    fn icon_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                let icon = self.geo.borrow().items.iter().find(|(j, _)| *j == i).map(|(_, l)| l.icon).unwrap_or_default();
                let d = items::Drag {
                    paths: self.selected_paths(),
                    icon: items::icon_of(entry),
                    size: f32::from(icon.size.width).max(items::MINI),
                    hotspot: point((at.x - icon.left()).clamp(px(0.0), icon.size.width), (at.y - icon.top()).clamp(px(0.0), icon.size.height)),
                    ask,
                    name: entry.name.clone(),
                };
                items::start(window, cx, self.key(), d);
                cx.notify();
            }
            None if press.button == MouseButton::Left => {
                self.band_to(e.position, window);
                cx.notify();
            }
            _ => {}
        }
    }

    /// The band now reaches `now` (in the window): what it covers is
    /// selected (every item in the rows it crosses, not only those
    /// showing), and near the top or bottom it scrolls on.
    fn band_to(&mut self, now: Point<Pixels>, window: &mut Window) {
        let Some((start, base)) = self.band.clone() else { return };
        let look = if self.view == ViewMode::Icons { Look::Icons } else { Look::Mini };
        let g = self.geo.borrow();
        let here = point(now.x - g.origin.x, now.y - g.origin.y);
        let r = Bounds::from_corners(point(start.x.min(here.x), start.y.min(here.y)), point(start.x.max(here.x), start.y.max(here.y)));
        let r = Bounds { origin: point(r.origin.x + g.origin.x, r.origin.y + g.origin.y), size: r.size };
        let (cols, n) = (g.cols.max(1), self.entries.len());
        let row = |y: Pixels| (f32::from(y - g.origin.y - px(g.inset)) / g.cell.1).floor().max(0.0) as usize;
        let range = (row(r.top()) * cols).min(n)..((row(r.bottom()) + 1) * cols).min(n);
        let mut covered = Vec::new();
        for i in range {
            let e = &self.entries[i];
            if items::layout_item(window, g.cell_at(i), e.label(), look, false, e.is_link).hit().intersects(&r) {
                covered.push(i);
            }
        }
        let (view, cell_h) = (g.view, g.cell.1);
        drop(g);
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
            self.scroll.set_offset(point(o.x, px((f32::from(o.y) - step).clamp(-max, 0.0))));
            window.request_animation_frame();
        }
    }

    fn icon_release(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else { return };
        self.band = None;
        if press.dragging {
            cx.notify();
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
                        self.edit_timer = Some(cx.spawn(async move |me, cx| {
                            cx.background_executor().timer(EDIT_DELAY).await;
                            let _ = me.update(cx, |f, cx| f.edit_if_still_pending(cx));
                        }));
                    }
                } else {
                    self.pending_edit = None;
                }
                self.list.click(i, mods(&press.mods));
            }
            (MouseButton::Right, item) => self.context_menu(item.is_some(), e.position, window, cx),
            _ => self.pending_edit = None,
        }
        cx.notify();
    }

    /// The moment after a click on a selected name passed with no double
    /// click: edit it, if it's still the one selected.
    fn edit_if_still_pending(&mut self, cx: &mut Context<Self>) {
        let Some((path, at)) = self.pending_edit.take() else { return };
        if at.elapsed() >= EDIT_DELAY - Duration::from_millis(20) && self.list.selected.len() == 1 && self.selected_paths().first() == Some(&path) {
            self.edit_name(&path, cx);
            cx.notify();
        }
    }

    fn icon_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let look = if self.view == ViewMode::Icons { Look::Icons } else { Look::Mini };
        let (cell, inset) = if look == Look::Icons { (items::ICON_CELL, m::SPACING) } else { (items::MINI_CELL, m::TIGHT) };
        // As wide as the view less its bar: known from the last frame (the
        // window's width before the first).
        let view_w = {
            let g = self.geo.borrow();
            if g.view.size.width > px(0.0) { f32::from(g.view.size.width) } else { f32::from(window.viewport_size().width) - SCROLL_BAR }
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
            let around = ((f32::from(self.geo.borrow().view.size.height) / cell.1 / 2.0).floor() - 1.0).max(0.0);
            self.reveal_icon(row, around);
        }
        // Editing a name: the field over its label, where it is in the
        // content (it scrolls with it).
        let field = self.editing.as_ref().and_then(|r| {
            let i = self.entries.iter().position(|e| e.path == r.path)?;
            let e = &self.entries[i];
            let c = Bounds { origin: point(px(inset + (i % cols) as f32 * cell.0), px(inset + (i / cols) as f32 * cell.1)), size: size(px(cell.0), px(cell.1)) };
            let origin = self.geo.borrow().origin;
            let c = Bounds { origin: point(c.origin.x + origin.x, c.origin.y + origin.y), size: c.size };
            let l = items::layout_item(window, c, e.label(), look, true, e.is_link);
            Some(r.element(window, cx, &l, origin))
        });
        if self.editing.as_ref().is_some_and(|r| !self.entries.iter().any(|e| e.path == r.path)) {
            self.editing = None; // the item went away
        }
        let me = cx.entity();
        let paint_me = me.clone();
        let geo = self.geo.clone();
        let spring = self.spring.clone();
        let p = theme::palette(cx);
        let ground = p.document_background;
        let content = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let clip = window.content_mask().bounds.intersect(&b);
                // What shows (and only that), taken out of the window's
                // state so it can be drawn.
                let first = ((f32::from(clip.top() - b.top()) - inset) / cell.1).floor().max(0.0) as usize * cols;
                let last = (((f32::from(clip.bottom() - b.top()) - inset) / cell.1).ceil().max(0.0) as usize + 1) * cols;
                let cell_at =
                    |i: usize| Bounds { origin: point(b.left() + px(inset + (i % cols) as f32 * cell.0), b.top() + px(inset + (i / cols) as f32 * cell.1)), size: size(px(cell.0), px(cell.1)) };
                // (Laid out from the window's entries where they are: no
                // copies of what shows, a frame's worth of allocations.)
                let (layouts, editing, band, took, drop_on, spring_to) = {
                    let f = paint_me.read(cx);
                    let range = first.min(f.entries.len())..last.min(f.entries.len());
                    let layouts: Vec<Shown> = range
                        .map(|i| {
                            let e = &f.entries[i];
                            let selected = f.list.selected.contains(&i);
                            Shown { i, layout: items::layout_item(window, cell_at(i), e.label(), look, selected, e.is_link), icon: items::icon_of(e), selected, progress: e.progress() }
                        })
                        .collect();
                    let editing = f.editing.as_ref().and_then(|r| layouts.iter().find(|s| f.entries[s.i].path == r.path)).map(|s| s.i);
                    let band = f.band.clone().zip(f.press.as_ref().filter(|p| p.item.is_none() && p.button == MouseButton::Left)).map(|((start, _), _)| start);
                    // Something dragged over it: onto a folder in it, or into it.
                    let (mut took, mut drop_on, mut spring_to) = (false, None, None);
                    if let Some(o) = window.drop_offer().cloned() {
                        let at = window.mouse_position();
                        if clip.contains(&at) {
                            let over = layouts.iter().find(|s| s.layout.hit().contains(&at)).map(|s| (s.i, &f.entries[s.i], s.selected));
                            (took, drop_on) = items::item_drop(window, cx, f.key(), &o, over, Some(&f.current_dir()));
                            spring_to = drop_on.filter(|_| took).map(|i| f.entries[i].path.clone());
                        }
                    }
                    (layouts, editing, band, took, drop_on, spring_to)
                };
                let mut drawn = Vec::with_capacity(layouts.len());
                window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
                    for s in layouts {
                        items::paint_item(window, cx, &s.layout, s.icon, s.selected || drop_on == Some(s.i), false, editing != Some(s.i), &p);
                        if let Some(done) = s.progress {
                            items::paint_progress(window, s.layout.icon, done, &p);
                        }
                        drawn.push((s.i, s.layout));
                    }
                    if took && drop_on.is_none() {
                        items::outline(window, clip, &p);
                    }
                    if let Some(start) = band {
                        let now = window.mouse_position();
                        let s = point(start.x + b.left(), start.y + b.top());
                        let r = Bounds::from_corners(point(s.x.min(now.x), s.y.min(now.y)), point(s.x.max(now.x), s.y.max(now.y)));
                        style::selection_band(window, r.intersect(&clip), &p);
                    }
                });
                {
                    let mut g = geo.borrow_mut();
                    g.origin = b.origin;
                    g.view = clip;
                    g.items = drawn;
                }
                let open = spring.borrow_mut().rest(spring_to.as_deref(), window, cx);
                if let Some(dir) = open {
                    paint_me.update(cx, |f, cx| f.act(Action::Spring(dir), cx));
                }
                // The pointer held down goes on being followed past the
                // view's edge.
                let (mv, up) = (paint_me.clone(), paint_me.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        mv.update(cx, |f, cx| f.icon_move(e, window, cx));
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        up.update(cx, |f, cx| f.icon_release(e, window, cx));
                    }
                });
            },
        )
        .size_full();
        let (down, right) = (me.clone(), me.clone());
        ScrollView::new("icons", &self.scroll).border(false).background(ground).child(
            div()
                .id("icons-content")
                .relative()
                .w_full()
                .h(px(height))
                .min_h_full()
                .on_mouse_down(MouseButton::Left, move |e, window, cx| down.update(cx, |f, cx| f.icon_press(e, window, cx)))
                .on_mouse_down(MouseButton::Right, move |e, window, cx| right.update(cx, |f, cx| f.icon_press(e, window, cx)))
                .child(content)
                .children(field),
        )
    }

    // ---- the list view -----------------------------------------------------

    fn list_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let list = self.list.clone();
        self.list_model.update(cx, |m, _| {
            if m.state.selected != list.selected || m.state.anchor != list.anchor || m.state.cursor != list.cursor || m.state.sort != sort {
                m.state.selected = list.selected;
                m.state.anchor = list.anchor;
                m.state.cursor = list.cursor;
                m.state.sort = sort;
            }
            if let Some(row) = reveal {
                m.reveal(row);
            }
        });
        let me = cx.entity();
        let cols = [Column::new("Name", 220.0), Column::new("Size", 80.0).right(), Column::new("Modified", 160.0), Column::new("Kind", 120.0)];
        let (cell_me, select_me, sort_me, open_me, context_me, drag_me) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        // Names for typing to select, made again only when the items change.
        if self.names.0 != self.stamp {
            self.names = (self.stamp, self.entries.iter().map(|e| olo_ui::gpui::SharedString::from(e.name.clone())).collect());
        }
        let names = self.names.1.clone();
        let model = self.list_model.clone();
        let (select_model, sort_model, drag_model) = (model.clone(), model.clone(), model.clone());
        let editing = self.editing.as_ref().map(|r| r.path.clone());
        ColumnListView::new("list", &self.list_model, cols, self.entries.len(), move |cell, window, cx| {
            let Some(e) = cell_me.read(cx).entries.get(cell.row).cloned() else { return };
            match cell.column {
                0 => name_cell(cell, window, cx, &e, editing.as_ref() != Some(&e.path)),
                1 => cell.text(window, cx, e.size_text()),
                2 => cell.text(window, cx, fs::format_time(e.modified)),
                _ => cell.text(window, cx, e.kind_text()),
            }
        })
        .type_select(move |i| names.get(i).cloned().unwrap_or_default())
        .on_select(move |_, cx| {
            let s = select_model.read(cx).state.clone();
            select_me.update(cx, |f, cx| {
                f.list.selected = s.selected;
                f.list.anchor = s.anchor;
                f.list.cursor = s.cursor;
                if f.editing.is_some() {
                    f.end_rename(true, cx);
                }
                cx.notify();
            });
        })
        .on_sort(move |_, cx| {
            let Some((col, asc)) = sort_model.read(cx).state.sort else { return };
            let key = [SortKey::Name, SortKey::Size, SortKey::Modified, SortKey::Kind][col.min(3)];
            sort_me.update(cx, |f, cx| {
                if f.sort != Some((key, asc)) {
                    f.sort = Some((key, asc));
                    crate::sorts::remember(cx, &f.path, f.sort);
                    f.resort();
                    cx.notify();
                }
            });
        })
        .on_open(move |row, _, cx| {
            open_me.update(cx, |f, cx| {
                // What's selected, if it's one of them.
                if f.list.selected.contains(&row) {
                    f.command("open", cx);
                } else if let Some(e) = f.entries.get(row).cloned() {
                    f.act(Action::Open(e), cx);
                }
            });
        })
        .on_context(move |_, at, window, cx| context_me.update(cx, |f, cx| f.context_menu(true, at, window, cx)))
        .on_drag(move |row, window, cx| {
            let paths: Vec<PathBuf> = {
                let st = &drag_model.read(cx).state;
                let f = drag_me.read(cx);
                st.selected.iter().filter_map(|&i| f.entries.get(i)).map(|e| e.path.clone()).collect()
            };
            let Some(e) = drag_me.read(cx).entries.get(row).cloned() else { return };
            let key = drag_me.entity_id();
            let d = items::Drag { paths, icon: items::icon_of(&e), size: items::MINI, hotspot: point(px(items::MINI / 2.0), px(items::MINI / 2.0)), ask: false, name: e.name.clone() };
            items::start(window, cx, key, d);
        })
    }

    // ---- the window --------------------------------------------------------

    /// The bar along the bottom: the count on the left, free space on the
    /// right (BeOS put the count beside the scroll bar). A scroll bar and
    /// its etched edge tall: the small text's line under the edge (as
    /// tall as a bar, it ran a line past the window's bottom).
    fn count_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let _ = cx;
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, style::c(p.panel_background));
                style::hline(window, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.3));
                style::hline(window, b.left(), b.right(), b.top() + style::hairline(window), style::c(p.shine));
                // Below the two-line etched edge.
                let cy = b.center().y + style::hairline(window);
                let font = style::ui_font(FontWeight::NORMAL);
                // In line with the list's first column (its icons) above.
                let at = style::label(window, cx, b.left() + px(m::CELL_INSET), cy, Align::Left, count.clone(), style::SMALL_SIZE, font.clone(), style::c(p.panel_text));
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(count.clone(), at));
                if let Some(free) = free {
                    let text = format!("{} free", fs::format_size(free));
                    let at = style::label(window, cx, b.right() - px(m::CELL_INSET), cy, Align::Right, text.clone(), style::SMALL_SIZE, font, style::disabled_text(&p));
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(text, at));
                }
            },
        )
        .w_full()
        .h(px(SCROLL_BAR + 2.0))
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

    /// Listing a big folder shows its count growing: drawn again a few
    /// times a second while it lists.
    fn poll_while_loading(&mut self, cx: &mut Context<Self>) {
        let loading = self.loading && self.entries.is_empty() && self.error.is_none();
        if !loading {
            self.poll = None;
            return;
        }
        if self.poll.is_some() {
            return;
        }
        self.poll = Some(cx.spawn(async move |me, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let go_on = me.update(cx, |f, cx| {
                    cx.notify();
                    f.loading && f.entries.is_empty() && f.error.is_none()
                });
                if !matches!(go_on, Ok(true)) {
                    let _ = me.update(cx, |f, _| f.poll = None);
                    return;
                }
            }
        }));
    }
}

/// A list cell with a 16-point icon before the name (`text`: and the
/// name, unless it's being edited).
pub fn name_cell(cell: &mut olo_ui::widgets::list::Cell, window: &mut Window, cx: &mut App, e: &Entry, text: bool) {
    let b = cell.bounds;
    let icon = Bounds { origin: point(b.left() + px(m::CELL_INSET), b.center().y - px(items::MINI / 2.0)), size: size(px(items::MINI), px(items::MINI)) };
    olo_ui::icons::paint(window, icon, items::icon_of(e));
    let rest = Bounds::from_corners(point(icon.right() + px(m::MARK_GAP - m::CELL_INSET), b.top()), b.bottom_right());
    let saved = cell.bounds;
    cell.bounds = rest;
    if text {
        let color = cell.text_color;
        cell.text_with(window, cx, e.label().to_owned(), items::name_font(e.is_link), color);
    } else {
        cell.say(e.label().to_owned());
    }
    cell.bounds = saved;
}

/// Delete and F2 trash and rename in a view as well as the menus' keys,
/// unless they are the menus' keys for it already (Windows'), which would
/// do it twice.
pub fn plain_extra(command: &str, key: &str) -> bool {
    olo_kit_base::keys::shortcut(command).is_none_or(|s| !(s.key == olo_ui::keys::evdev(key) && !s.command && !s.alt && !s.shift))
}

impl Render for FolderWin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.surface = window.surface_id().or(self.surface);
        let vs = window.viewport_size();
        self.size = (f32::from(vs.width).round() as u32, f32::from(vs.height).round() as u32);
        if !self.path_said {
            self.path_said = true;
            olo_ui::agent::set_path(window, cx, Some(self.path.clone()));
        }
        // The keyboard: an edited name's field, else the view.
        if let Some(r) = &mut self.editing {
            if !r.focused {
                r.focused = true;
                let field = r.field.clone();
                field.read(cx).focus(window);
                cx.subscribe_in(&field, window, |f, _, e: &TextFieldEvent, _, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        f.end_rename(true, cx);
                    } else {
                        cx.notify();
                    }
                })
                .detach();
                let handle = field.read(cx).focus_handle(cx);
                cx.on_focus_out(&handle, window, |f, _, _, cx| f.end_rename(true, cx)).detach();
            }
        } else if self.focus_after_edit || window.focused(cx).is_none() || self.focused_view != Some(self.view) {
            // (Shown another way, the keyboard goes with it.)
            self.focus_after_edit = false;
            self.focused_view = Some(self.view);
            match self.view {
                ViewMode::List => self.list_model.read(cx).focus(window),
                _ => window.focus(&self.focus),
            }
        }
        self.poll_while_loading(cx);
        let places = crate::places(cx);
        let can_paste = crate::can_paste(cx);
        let menus = self.menus(can_paste, &places);
        let (app, key) = (self.app.clone(), self.key());
        let bar = MenuBar::new(menus, move |cmd, _, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.folder_command(key, cmd, cx));
            }
        });
        let p = theme::palette(cx);
        let body = match (&self.error, self.view) {
            (Some(err), _) => {
                let err = err.clone();
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        style::fill_rect(window, b, style::c(p.document_background));
                        let at = style::label(window, cx, b.center().x, b.center().y, Align::Center, err.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                        olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(err.clone(), at));
                    },
                )
                .size_full()
                .into_any_element()
            }
            (None, ViewMode::List) => self.list_view(cx).into_any_element(),
            (None, ViewMode::Columns) => self.columns_view(window, cx).into_any_element(),
            (None, _) => self.icon_view(window, cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(bar)
            .child(div().flex_1().min_h_0().relative().flex().flex_col().child(body))
            .child(self.count_bar(cx))
    }
}

/// Open With ▸ for `entries` of one kind (`olo_ui::open_with`: the
/// apps worked out as the folder was listed; none yet, no submenu).
pub fn open_with_menu(entries: &[Entry]) -> Option<Menu> {
    olo_ui::open_with::menu(&open_with_kind(entries)?)
}

/// The kind `entries` all are, for Open With (None: none, or mixed).
pub fn open_with_kind(entries: &[Entry]) -> Option<String> {
    olo_ui::open_with::kind_of(entries.iter().map(|e| (e.name.as_str(), e.is_dir)))
}
