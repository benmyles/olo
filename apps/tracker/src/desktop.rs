//! The desktop: a clear background layer showing volumes, Home, the
//! contents of ~/Desktop and the Trash, laid out as on BeOS (volumes down
//! the right-hand side next to the Deskbar, Trash in the bottom-right
//! corner).

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use olo_kit::{
    Bounds, Cx, DragAction, DropAccept, DropOffer, Dropped, Element, FocusHandle, InteractiveElement as _, IntoElement as _, KeyDownEvent, Keystroke, LayerKeyboard, LayerShellLayer, Menu,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Styled as _, View, WindowOptions, div, menu, metrics as m, point, px, size, style, theme,
    widgets::{TextFieldEvent, open_alert},
};
use olo_kit_base::list::SelectionBand;

use crate::{
    ToHub, WinInfo,
    folder::mods,
    fs::{self, Entry},
    items::{self, DropTarget, ICON_CELL, ItemLayout, Look, Rename, Spring},
    ops, to_hub, undo, watch,
};

/// Room left for the Deskbar in the top-right corner (its width).
const DESKBAR_WIDTH: f32 = 172.0;
/// From the screen edge to the first column of icons.
const EDGE: f32 = m::SPACING;
const DRAG_SLOP: f32 = 3.0;

/// Icons being moved, by [`place_key`], and where each cell was.
type Dragged = Vec<(String, Point<Pixels>)>;

struct Item {
    entry: Entry,
    label: String,
    icon: &'static str,
}

/// What the desktop is told: by the app, by itself later.
pub enum DesktopMsg {
    /// Changes seen on disk.
    Changed(Arc<Vec<watch::Change>>),
    Reload,
    Select(Vec<PathBuf>),
    /// `from` is `to` now: its icon's place goes with it.
    Renamed(Vec<(PathBuf, PathBuf)>),
    /// Icons back where they were (Undo, Redo).
    PutPlaces(Placed),
    /// Something it shows that isn't its own changed (a backup's marks).
    Refresh,
    /// A drag out of it is over.
    DragOver,
}

pub struct Desktop {
    /// The app's number for this window.
    id: u64,
    items: Vec<Item>,
    /// The Trash, pinned to the bottom-right corner.
    trash: Item,
    selected: BTreeSet<usize>,
    /// A band: where it began, what was selected, where it reaches now.
    band: Option<(Point<Pixels>, SelectionBand, Point<Pixels>)>,
    /// A ~/Desktop item whose name is being edited in place.
    editing: Option<Rename>,
    focus_after_edit: bool,
    /// Where icons were put by hand (the rest fill the grid around them).
    places: Places,
    /// The screen, as last laid out.
    screen: Bounds<Pixels>,
    /// A drag of icons started here: where the button went down, and
    /// where they were then (to put them where they are dropped).
    dragging: Option<(Point<Pixels>, Dragged)>,
    press: Option<Press>,
    /// The items as last laid out.
    layouts: RefCell<Vec<ItemLayout>>,
    spring: Spring,
    /// The item what's dragged over the desktop would be dropped on
    /// (a folder, the Trash), if it would be taken there.
    drop_on: Option<usize>,
    focus: FocusHandle,
    /// What the app was last told of it.
    told: Option<WinInfo>,
}

struct Press {
    at: Point<Pixels>,
    button: MouseButton,
    item: Option<usize>,
    was_selected: bool,
    ctrl: bool,
    shift: bool,
    dragging: bool,
}

/// Icons placed by hand, by [`place_key`], as the top-left of each one's
/// cell from the screen's top-right corner (where the grid starts, so
/// they keep their place beside the Deskbar when the screen size
/// changes). Kept in `~/.config/olo/desktop.json`, which agents can read
/// too.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Places {
    #[serde(default)]
    icons: BTreeMap<String, [f32; 2]>,
}

fn places_path() -> PathBuf {
    olo_kit::config_dir().join("desktop.json")
}

impl Places {
    fn load() -> Places {
        olo_kit::store::load(&places_path()).unwrap_or_default()
    }

    /// Written on a worker (nothing waits for the disk).
    fn save(&self) {
        olo_kit::store::save(places_path(), self);
    }
}

/// Icons whose places changed: each one's key, and its place before and
/// after (None: on the grid).
pub type Placed = Vec<(String, Option<[f32; 2]>, Option<[f32; 2]>)>;

/// What a desktop icon's place is kept under: a ~/Desktop item's name,
/// else its path (Home, volumes), or "Trash".
fn place_key(path: &Path) -> String {
    if path == fs::trash_files() {
        return "Trash".into();
    }
    match path.strip_prefix(desktop_dir()) {
        Ok(name) if !name.as_os_str().is_empty() => name.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// The desktop's surface: a layer behind every window, over the whole
/// screen, taking the keyboard when clicked.
pub fn options() -> WindowOptions {
    WindowOptions::new("Desktop", 1.0, 1.0)
        .layer("desktop", LayerShellLayer::Background, (true, true, true, true))
        .layer_options(|l| {
            l.exclusive_zone = -1;
            l.keyboard = LayerKeyboard::OnDemand;
        })
        .no_panel()
}

/// The folders the desktop shows: ~/Desktop, and the Trash (for its icon).
pub fn watched() -> [PathBuf; 2] {
    [desktop_dir(), fs::trash_files()]
}

fn desktop_dir() -> PathBuf {
    fs::home().join("Desktop")
}

fn pressed(k: &Keystroke, command: &str) -> bool {
    olo_kit_base::keys::shortcut(command).is_some_and(|s| olo_kit::keys::pressed(&s, k))
}

impl Desktop {
    pub fn new(id: u64, selected: Vec<PathBuf>) -> Self {
        // Made up front so it can be watched from the start.
        let _ = std::fs::create_dir_all(desktop_dir());
        let mut d = Desktop {
            id,
            items: Vec::new(),
            trash: trash_item(),
            selected: BTreeSet::new(),
            band: None,
            editing: None,
            focus_after_edit: false,
            places: Places::load(),
            screen: Bounds::default(),
            dragging: None,
            press: None,
            layouts: RefCell::default(),
            spring: Spring::default(),
            drop_on: None,
            focus: FocusHandle::new(),
            told: None,
        };
        d.reload();
        d.select_paths(&selected);
        d
    }

    pub fn reload(&mut self) {
        let selected = self.selected_paths();
        let mut items = Vec::new();
        for (label, path, icon) in fs::volumes() {
            if let Some(entry) = fs::entry(&path) {
                items.push(Item { entry, label, icon });
            }
        }
        if let Some(entry) = fs::entry(&fs::home()) {
            items.push(Item { entry, label: "Home".into(), icon: "home" });
        }
        if let Ok(list) = fs::list(&desktop_dir(), false) {
            for entry in list {
                let icon = items::icon_of(&entry);
                items.push(Item { label: entry.name.clone(), entry, icon });
            }
        }
        // What Open With offers for the kinds here, worked out off the
        // UI thread so a menu finds it at once.
        let kinds: std::collections::BTreeSet<String> = items.iter().map(|it| olo_xdg::open::kind_of(&it.entry.name, it.entry.is_dir)).collect();
        std::thread::spawn(move || olo_xdg::open::warm(kinds));
        self.items = items;
        self.trash = trash_item();
        self.select_paths(&selected);
    }

    /// Splice in changes seen on disk: ~/Desktop items that came or went,
    /// the Trash filling or emptying, volumes (un)mounted. Returns whether
    /// anything shown changed.
    pub fn apply(&mut self, changes: &[watch::Change]) -> bool {
        let (desk, trash) = (desktop_dir(), fs::trash_files());
        let mut changed = false;
        let selected: Vec<PathBuf> = self.selected_paths();
        for c in changes {
            match c {
                watch::Change::Mounts => {
                    self.reload();
                    return true;
                }
                watch::Change::Rescan(d) if *d == desk => {
                    self.reload();
                    return true;
                }
                watch::Change::Rescan(d) | watch::Change::Item { dir: d, .. } if *d == trash => {
                    let t = trash_item();
                    if t.icon != self.trash.icon {
                        self.trash = t;
                        changed = true;
                    }
                }
                watch::Change::Item { dir, path, entry } if *dir == desk => {
                    let at = self.items.iter().position(|it| it.entry.path == *path);
                    match (at, entry) {
                        (Some(i), Some(e)) => {
                            self.items[i].icon = items::icon_of(e);
                            self.items[i].entry = e.clone();
                        }
                        (None, Some(e)) if !e.name.starts_with('.') => {
                            let icon = items::icon_of(e);
                            self.items.push(Item { label: e.name.clone(), entry: e.clone(), icon });
                        }
                        (Some(i), None) => {
                            self.items.remove(i);
                        }
                        _ => continue,
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.select_paths(&selected);
        }
        changed
    }

    /// Items by index; the Trash comes last.
    fn item(&self, i: usize) -> Option<&Item> {
        if i == self.items.len() { Some(&self.trash) } else { self.items.get(i) }
    }

    pub fn select_paths(&mut self, paths: &[PathBuf]) {
        self.selected = (0..=self.items.len()).filter(|&i| self.item(i).is_some_and(|it| paths.contains(&it.entry.path))).collect();
    }

    /// Select `p` and start editing its name in place.
    pub fn edit_name(&mut self, p: &Path, cx: &mut Cx<Self>) {
        if let Some(i) = self.items.iter().position(|it| it.entry.path == p) {
            self.selected = BTreeSet::from([i]);
            let rename = Rename::new(p.to_owned(), self.items[i].entry.is_link, cx);
            cx.subscribe(&rename.field, |d: &mut Desktop, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    d.end_rename(true, cx);
                } else {
                    cx.invalidate();
                }
            });
            self.editing = Some(rename);
            cx.invalidate();
        }
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries().into_iter().map(|e| e.path).collect()
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        self.selected.iter().filter_map(|&i| self.item(i)).map(|it| it.entry.clone()).collect()
    }

    /// Selected items that are files or folders on the Desktop (not
    /// volumes, Home or the Trash, which can't be moved to the Trash).
    pub fn movable(&self) -> Vec<PathBuf> {
        let dir = desktop_dir();
        self.selected_paths().into_iter().filter(|p| p.parent() == Some(dir.as_path())).collect()
    }

    /// Where each item's cell is (the Trash last). Icons placed by hand
    /// stay where they were put (kept on screen); the others fill the
    /// grid around them as on BeOS: columns downward from the top right,
    /// beside the Deskbar, wrapping leftward, with the Trash in the
    /// bottom-right corner unless it was moved.
    fn cells(&self, screen: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
        let n = self.items.len() + 1;
        let cell = size(px(ICON_CELL.0), px(ICON_CELL.1));
        let placed = |i: usize| {
            let p = self.places.icons.get(&place_key(&self.item(i)?.entry.path))?;
            let x = (screen.right() - px(p[0])).clamp(screen.left(), (screen.right() - cell.width).max(screen.left()));
            let y = (screen.top() + px(p[1])).clamp(screen.top(), (screen.bottom() - cell.height).max(screen.top()));
            Some(Bounds { origin: point(x, y), size: cell })
        };
        let mut cells: Vec<Option<Bounds<Pixels>>> = (0..n).map(placed).collect();
        if cells[n - 1].is_none() {
            cells[n - 1] = Some(Bounds { origin: point(screen.right() - px(EDGE) - cell.width, screen.bottom() - px(EDGE) - cell.height), size: cell });
        }
        let usable = f32::from(screen.size.height) - 2.0 * EDGE - ICON_CELL.1; // keep the Trash's row free
        let per_col = ((usable / ICON_CELL.1).floor() as usize).max(1);
        let slot = |k: usize| {
            let (col, row) = (k / per_col, k % per_col);
            let right = screen.right() - px(DESKBAR_WIDTH + EDGE + col as f32 * ICON_CELL.0);
            Bounds { origin: point(right - cell.width, screen.top() + px(EDGE + row as f32 * ICON_CELL.1)), size: cell }
        };
        let shrink = |b: Bounds<Pixels>| Bounds { origin: point(b.left() + px(4.0), b.top() + px(4.0)), size: size(b.size.width - px(8.0), b.size.height - px(8.0)) };
        let mut k = 0;
        for i in 0..n - 1 {
            if cells[i].is_some() {
                continue;
            }
            // The next grid slot no placed icon covers (a few thousand
            // slots at most: past the left edge it gives up and overlaps).
            while k < 4096 && cells.iter().flatten().any(|c| shrink(*c).intersects(&slot(k))) {
                k += 1;
            }
            cells[i] = Some(slot(k));
            k += 1;
        }
        cells.into_iter().map(|c| c.unwrap_or_default()).collect()
    }

    /// Put the icons that were dragged from here where they were dropped
    /// (`at`, the pointer); where each was and is now (Edit ▸ Undo).
    pub fn place_dragged(&mut self, at: Point<Pixels>) -> Placed {
        let Some((from, icons)) = self.dragging.take() else { return Vec::new() };
        let screen = self.screen;
        let mut moved = Vec::new();
        for (key, was) in icons {
            let to = point(was.x + (at.x - from.x), was.y + (at.y - from.y));
            let to = [f32::from(screen.right() - to.x), f32::from(to.y - screen.top())];
            let before = self.places.icons.insert(key.clone(), to);
            moved.push((key, before, Some(to)));
        }
        self.places.save();
        moved
    }

    /// Each icon of `moved` back where it was before it (Undo; Redo gives
    /// them the other way round): placed, or on the grid.
    pub fn put_places(&mut self, moved: &Placed) {
        for (key, before, _) in moved {
            match before {
                Some(at) => self.places.icons.insert(key.clone(), *at),
                None => self.places.icons.remove(key),
            };
        }
        self.places.save();
    }

    /// Put the things just dropped here from elsewhere (now in ~/Desktop,
    /// as `names`) where they were dropped, side by side.
    pub fn place_new(&mut self, names: &[String], at: Point<Pixels>) {
        let screen = self.screen;
        for (k, name) in names.iter().enumerate() {
            let min = point(at.x - px(ICON_CELL.0 / 2.0) + px(k as f32 * ICON_CELL.0), at.y - px(ICON_CELL.1 / 2.0));
            self.places.icons.insert(name.clone(), [f32::from(screen.right() - min.x), f32::from(min.y - screen.top())]);
        }
        self.places.save();
    }

    /// `from` is now `to`: its place goes with it.
    pub fn renamed(&mut self, from: &Path, to: &Path) {
        if let Some(at) = self.places.icons.remove(&place_key(from)) {
            self.places.icons.insert(place_key(to), at);
            self.places.save();
        }
    }

    /// BeOS's Clean Up: every icon back on the grid; where each was.
    pub fn clean_up(&mut self) -> Placed {
        let moved = std::mem::take(&mut self.places.icons).into_iter().map(|(k, at)| (k, Some(at), None)).collect();
        self.places.save();
        moved
    }

    /// Opens `e`: a folder's window, a file by what opens its kind.
    fn open(&self, e: &Entry) {
        if e.is_dir {
            to_hub(ToHub::Open { path: e.path.clone(), from: None });
        } else {
            fs::open(e);
        }
    }

    fn error(&self, message: String, cx: &mut Cx<Self>) {
        let _ = open_alert(cx, "Tracker", "about", &message, &["OK"], |_, _| {});
    }

    fn end_rename(&mut self, commit: bool, cx: &mut Cx<Self>) {
        let Some(r) = self.editing.take() else { return };
        if commit {
            let name = r.text(cx);
            to_hub(ToHub::Rename { win: None, path: r.path, name });
        }
        self.focus_after_edit = true;
        cx.invalidate();
    }

    /// The desktop's commands (its menus' and its keys').
    pub fn command(&mut self, cmd: &str, cx: &mut Cx<Self>) {
        match cmd {
            "clean-up" => {
                let moved = self.clean_up();
                to_hub(ToHub::Done("Clean Up".into(), undo::Change::Placed(moved)));
            }
            "open" => {
                for e in self.selected_entries() {
                    self.open(&e);
                }
            }
            "new-window" => {
                if let Some(app) = self.selected_entries().first().and_then(fs::running_app) {
                    fs::new_window(app);
                }
            }
            "group-windows" => fs::toggle_grouping(self.selected_entries().first()),
            "trash" => {
                // Only items that live in ~/Desktop can go to the Trash.
                let dir = desktop_dir();
                self.selected.retain(|&i| self.items.get(i).is_some_and(|it| it.entry.path.parent() == Some(dir.as_path())));
                let paths = self.movable();
                if !paths.is_empty() {
                    to_hub(ToHub::Job { kind: ops::Kind::Trash, sources: paths, dest: None, origin: Some(self.id) });
                }
            }
            "rename" => {
                if let [p] = self.movable().as_slice() {
                    let p = p.clone();
                    self.edit_name(&p, cx);
                }
            }
            c if olo_kit::open_with::is_command(c) => crate::open_with(c, &self.selected_entries()),
            c if crate::folder::play_on(c, &self.selected_entries()) => {}
            "new-folder" => {
                let dir = desktop_dir();
                let _ = std::fs::create_dir_all(&dir);
                match fs::new_folder(&dir) {
                    Ok(p) => {
                        to_hub(ToHub::Done("New Folder".into(), undo::Change::Made(vec![p.clone()])));
                        self.reload();
                        self.edit_name(&p, cx);
                    }
                    Err(e) => self.error(format!("Couldn't create a folder: {e}"), cx),
                }
            }
            "empty-trash" => crate::confirm_empty_trash(cx),
            "undo" => to_hub(ToHub::UndoRedo(false, Some(self.id))),
            "redo" => to_hub(ToHub::UndoRedo(true, Some(self.id))),
            "terminal" => fs::open_in_terminal(&desktop_dir()),
            // From the desktop: a window on Home.
            "new" => to_hub(ToHub::NewWindow(fs::home(), None)),
            "backgrounds" => {
                let _ = olo_xdg::spawn_detached("olo-preferences backgrounds");
            }
            "info" => to_hub(ToHub::GetInfo(self.selected_entries())),
            "older-versions" => {
                if let Some(e) = self.selected_entries().first() {
                    crate::backups::older_versions(&e.path);
                }
            }
            _ => {}
        }
        cx.invalidate();
    }

    fn menu(&self, hit: Option<usize>) -> Menu {
        match hit {
            Some(i) if i == self.items.len() => Menu::new("").item("Open", "open").key("o").item("Empty Trash", "empty-trash").enabled(!fs::trash_is_empty()),
            Some(i) => {
                let mut m = Menu::new("").item("Open", "open").key("o");
                // A running app of ours can open another window.
                if self.selected.len() <= 1 && self.item(i).and_then(|it| fs::running_app(&it.entry)).is_some() {
                    m = m.item("New Window", "new-window");
                }
                if self.selected.len() <= 1 {
                    m = fs::grouping_item(m, self.item(i).map(|it| &it.entry));
                }
                if let Some(open_with) = crate::folder::open_with_menu(&self.selected_entries()) {
                    m = m.submenu(open_with);
                }
                if let Some(play) = crate::folder::play_on_menu(&self.selected_entries()) {
                    m = m.submenu(play);
                }
                m = m.item("Get Info", "info").key("i");
                if crate::backups::on() {
                    let file = self.selected.len() <= 1 && self.item(i).is_some_and(|it| !it.entry.is_dir);
                    m = m.item("Older Versions…", "older-versions").enabled(file);
                }
                m.separator().item("Move to Trash", "trash").key("t").enabled(!self.movable().is_empty())
            }
            None => Menu::new("")
                .item("New Window", "new")
                .item("New Folder", "new-folder")
                .item("Open in Terminal", "terminal")
                .separator()
                .item("Clean Up", "clean-up")
                .enabled(!self.places.icons.is_empty())
                .separator()
                .item("Change Background…", "backgrounds"),
        }
    }

    // ---- input -------------------------------------------------------------

    fn hit(&self, p: Point<Pixels>) -> Option<usize> {
        self.layouts.borrow().iter().position(|l| l.hit().contains(&p))
    }

    fn press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        self.focus.focus(cx);
        let pos = e.position;
        let hit = self.hit(pos);
        let (ctrl, shift) = (e.modifiers.control || e.modifiers.platform, e.modifiers.shift);
        if e.button == MouseButton::Left && e.click_count >= 2 {
            if let Some(entry) = hit.and_then(|i| self.item(i)).map(|it| it.entry.clone()) {
                self.open(&entry);
            }
            self.press = None;
            return;
        }
        let was_selected = hit.is_some_and(|i| self.selected.contains(&i));
        match hit {
            // Kept for the menu, or a drag, if it's one of them; ⌘ or ⇧
            // adds it (icons placed anywhere have no order for a range).
            Some(i) if !was_selected => {
                if !(ctrl || shift) {
                    self.selected.clear();
                }
                self.selected.insert(i);
            }
            Some(_) => {}
            None => {
                // A band: with ⇧ adding to what's selected, with ⌘ flipping
                // what it covers.
                if e.button == MouseButton::Left {
                    let base = if ctrl || shift { self.selected.clone() } else { BTreeSet::new() };
                    self.band = Some((pos, SelectionBand { base, flip: ctrl }, pos));
                }
                if !(ctrl || shift) {
                    self.selected.clear();
                }
            }
        }
        if let Some(it) = hit.and_then(|i| self.item(i)) {
            let _ = olo_kit::icons::bitmap(it.icon, 0);
        }
        let _ = mods;
        self.press = Some(Press { at: pos, button: e.button, item: hit, was_selected, ctrl, shift, dragging: false });
        cx.invalidate();
    }

    fn moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let Some(press) = &mut self.press else { return };
        if e.pressed_button.is_none() {
            return;
        }
        let moved = (e.position - press.at).magnitude() >= DRAG_SLOP as f64;
        match press.item {
            Some(i) if moved && !press.dragging => {
                press.dragging = true;
                let (at, ask) = (press.at, press.button == MouseButton::Right);
                // Every icon can be moved about the desktop.
                let layouts = self.layouts.borrow();
                let icons = self
                    .selected
                    .iter()
                    .filter_map(|&j| Some((place_key(&self.item(j)?.entry.path), layouts.get(j)?.icon.origin - point(px((ICON_CELL.0 - items::ICON) / 2.0), px(m::TIGHT)))))
                    .collect();
                let icon = layouts.get(i).map(|l| l.icon).unwrap_or_default();
                drop(layouts);
                self.dragging = Some((at, icons));
                // Everything but the Trash can go elsewhere.
                let paths: Vec<PathBuf> = self.selected.iter().filter(|&&j| j < self.items.len()).map(|&j| self.items[j].entry.path.clone()).collect();
                let Some(it) = self.item(i) else { return };
                let d = items::Drag {
                    paths,
                    icon: it.icon,
                    size: f32::from(icon.size.width),
                    hotspot: point((at.x - icon.left()).clamp(px(0.0), icon.size.width), (at.y - icon.top()).clamp(px(0.0), icon.size.height)),
                    ask,
                    name: it.label.clone(),
                };
                items::start(cx, self.id, d, || DesktopMsg::DragOver);
                cx.invalidate();
            }
            None if press.button == MouseButton::Left => {
                if let Some((start, base, now)) = &mut self.band {
                    *now = e.position;
                    let r = Bounds::from_corners(point(start.x.min(e.position.x), start.y.min(e.position.y)), point(start.x.max(e.position.x), start.y.max(e.position.y)));
                    let covered: Vec<usize> = self.layouts.borrow().iter().enumerate().filter(|(_, l)| l.hit().intersects(&r)).map(|(i, _)| i).collect();
                    self.selected = base.over(covered);
                }
                cx.invalidate();
            }
            _ => {}
        }
    }

    fn released(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let Some(press) = self.press.take() else { return };
        self.band = None;
        if press.dragging {
            cx.invalidate();
            return;
        }
        match (press.button, press.item) {
            (MouseButton::Left, Some(i)) if press.was_selected => {
                if !(press.ctrl || press.shift) {
                    self.selected = BTreeSet::from([i]);
                } else {
                    self.selected.remove(&i);
                }
            }
            (MouseButton::Right, hit) => {
                let me = cx.view_ref();
                menu::context_menu(cx, self.menu(hit), e.position, move |cmd, cx| {
                    let cmd = cmd.to_owned();
                    me.update(cx, move |d, cx| d.command(&cmd, cx));
                });
            }
            _ => {}
        }
        cx.invalidate();
    }

    /// Keys, when the desktop has the keyboard and no name is being
    /// edited: the menus' keys in the keyboard profile (⌘O opens, ⌘⌫
    /// trashes, ⌘I gets info, ⌘E edits a name, on a Mac), and Return, ⌘↓,
    /// Delete and F2 as well.
    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let k = &e.keystroke;
        if self.editing.is_some() {
            if k.key == "escape" {
                self.end_rename(false, cx);
                cx.stop_propagation();
            }
            return;
        }
        let ctrl = k.modifiers.control;
        let plain = !ctrl && !k.modifiers.alt && !k.modifiers.shift;
        let cmd = if pressed(k, "open") || (plain && k.key == "enter") || (ctrl && k.key == "down") {
            "open"
        } else if pressed(k, "trash") || (plain && k.key == "delete") {
            "trash"
        } else if pressed(k, "info") {
            "info"
        } else if pressed(k, "rename") || (plain && k.key == "f2") {
            "rename"
        } else if pressed(k, "new") {
            "new"
        } else if pressed(k, "new-folder") {
            "new-folder"
        } else if pressed(k, "redo") {
            "redo"
        } else if pressed(k, "undo") {
            "undo"
        } else {
            return;
        };
        if cmd == "trash" && self.movable().is_empty() {
            return;
        }
        cx.stop_propagation();
        self.command(cmd, cx);
    }

    /// What the app knows of the desktop, told when it changes: its one
    /// selected item (Quick Look follows it), what an upgrade keeps.
    fn tell_app(&mut self, surface: Option<u32>) {
        let mut sel = self.selected_entries().into_iter();
        let info = WinInfo { path: desktop_dir(), surface, watched: watched().to_vec(), selected: sel.next().filter(|_| sel.next().is_none()), folder: None, find: false };
        if self.told.as_ref().is_none_or(|t| !t.same(&info)) {
            crate::publish(self.id, info.clone());
            self.told = Some(info);
        }
    }
}

impl View for Desktop {
    type Message = DesktopMsg;

    fn message(&mut self, message: DesktopMsg, cx: &mut Cx<Self>) {
        match message {
            DesktopMsg::Changed(changes) => {
                if !self.apply(&changes) {
                    return;
                }
            }
            DesktopMsg::Reload => self.reload(),
            DesktopMsg::Select(paths) => self.select_paths(&paths),
            DesktopMsg::Renamed(pairs) => {
                for (from, to) in &pairs {
                    self.renamed(from, to);
                }
            }
            DesktopMsg::PutPlaces(moved) => self.put_places(&moved),
            DesktopMsg::Refresh => {}
            DesktopMsg::DragOver => {
                self.press = None;
                self.dragging = None;
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        Desktop::command(self, command, cx);
        true
    }

    /// The desktop stays.
    fn close_requested(&mut self, _: &mut Cx<Self>) -> bool {
        false
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        if !active && self.editing.is_some() {
            self.end_rename(true, cx);
        }
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&crate::SavedWindow::Desktop(self.selected_paths())).ok()
    }

    fn takes_drops(&self) -> bool {
        true
    }

    fn drop_over(&mut self, offer: Option<&DropOffer>, at: Point<Pixels>, cx: &mut Cx<Self>) -> Option<DropAccept> {
        let was = self.drop_on;
        let Some(o) = offer else {
            self.drop_on = None;
            self.spring.rest(None);
            if was.is_some() {
                cx.invalidate();
            }
            return None;
        };
        // Onto a folder or the Trash, or onto the desktop (into
        // ~/Desktop).
        let over = self.hit(at).and_then(|i| Some((i, &self.item(i)?.entry, self.selected.contains(&i))));
        let (mut accept, on) = items::item_drop(cx, self.id, o, over, Some(&desktop_dir()));
        // Icons dragged from here and let go anywhere but on a folder or
        // the Trash: put them there. (Home and the volumes too, which
        // aren't in ~/Desktop: let go on the desktop they were taken as
        // going into it, and Home "can't go inside itself".)
        if on.is_none() && items::from_view(self.id) && self.dragging.is_some() && o.move_ && o.has("text/uri-list") {
            accept = Some(DropAccept { target: DropTarget::Place.name(), mime: "text/uri-list".into(), action: DragAction::Move });
        }
        let took = accept.is_some();
        self.drop_on = on.filter(|_| took);
        let spring_to = self.drop_on.and_then(|i| self.item(i)).filter(|it| it.entry.is_dir).map(|it| it.entry.path.clone());
        if self.spring.rest(spring_to.as_deref()) {
            cx.after_do(items::SPRING + Duration::from_millis(10), |d, _| {
                if let Some(dir) = d.spring.due() {
                    to_hub(ToHub::Open { path: dir, from: None });
                }
            });
        }
        if self.drop_on != was {
            cx.invalidate();
        }
        accept
    }

    fn dropped(&mut self, drop: Dropped, cx: &mut Cx<Self>) {
        let Some(target) = DropTarget::parse(&drop.target) else { return };
        let paths = drop.paths();
        let desk = desktop_dir();
        let dest = match target {
            DropTarget::Trash => {
                let trash = fs::trash_files();
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !p.starts_with(&trash)).collect();
                if !paths.is_empty() {
                    to_hub(ToHub::Job { kind: ops::Kind::Trash, sources: paths, dest: None, origin: Some(self.id) });
                }
                return;
            }
            DropTarget::Place => {
                let moved = self.place_dragged(drop.position);
                to_hub(ToHub::Done(if moved.len() == 1 { "Move Icon" } else { "Move Icons" }.into(), undo::Change::Placed(moved)));
                return cx.invalidate();
            }
            DropTarget::Folder(dest) => dest,
        };
        // Dropped on the desktop: they go where they were let go (an
        // application dragged out of Deskbar: its link, once it's made).
        let link = drop.types.iter().any(|t| t == olo_kit::dnd::LINK_TYPE) && drop.mime == "text/uri-list" && drop.action != DragAction::Ask;
        if dest == desk && drop.action != DragAction::Ask && !link {
            let names: Vec<String> = paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            self.place_new(&names, drop.position);
        }
        let on_desk = dest == desk;
        match crate::dropped_into(&drop, paths, dest, Some(self.id), cx) {
            Ok(links) if link && on_desk => self.place_new(&links, drop.position),
            Ok(_) => {}
            Err(e) => self.error(e, cx),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        if self.editing.is_none() && self.focus_after_edit {
            self.focus_after_edit = false;
            self.focus.focus(cx);
        }
        self.tell_app(cx.surface_id());
        let screen = Bounds { origin: point(px(0.0), px(0.0)), size: cx.viewport_size() };
        self.screen = screen;
        let cells = self.cells(screen);
        let n = self.items.len() + 1;
        let layouts: Vec<ItemLayout> = (0..n)
            .map(|i| {
                let it = self.item(i).expect("an item");
                items::layout_item(cells[i], &it.label, Look::Icons, self.selected.contains(&i), it.entry.is_link)
            })
            .collect();
        let editing = self.editing.as_ref().and_then(|r| self.items.iter().position(|it| it.entry.path == r.path));
        if self.editing.is_some() && editing.is_none() {
            self.editing = None;
        }
        let field = editing.and_then(|i| Some(self.editing.as_ref()?.element(cx, &layouts[i], point(px(0.0), px(0.0)))));
        *self.layouts.borrow_mut() = layouts;
        let content = cx
            .canvas(move |d: &Desktop, g, b| {
                let p = theme::palette();
                let layouts = d.layouts.borrow();
                for (i, l) in layouts.iter().enumerate() {
                    let Some(it) = d.item(i) else { continue };
                    let sel = d.selected.contains(&i);
                    items::paint_item(g, l, it.icon, sel || d.drop_on == Some(i), true, editing != Some(i), &p);
                    if crate::backups::backed_up(&it.entry.path) {
                        items::paint_mark(g, l.icon, &l.name, &p);
                    }
                }
                if let Some((start, _, now)) = d.band.as_ref().filter(|_| d.press.as_ref().is_some_and(|p| p.item.is_none())) {
                    let r = Bounds::from_corners(point(start.x.min(now.x), start.y.min(now.y)), point(start.x.max(now.x), start.y.max(now.y)));
                    style::selection_band(g, r.intersect(&b), &p);
                }
            })
            .size_full();
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|d: &mut Desktop, e: &KeyDownEvent, cx| d.key_down(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|d: &mut Desktop, e: &MouseDownEvent, cx| d.press(e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|d: &mut Desktop, e: &MouseDownEvent, cx| d.press(e, cx)))
            .on_window_mouse_move(cx.listener(|d: &mut Desktop, e: &MouseMoveEvent, cx| d.moved(e, cx)))
            .on_window_mouse_up(cx.listener(|d: &mut Desktop, e: &MouseUpEvent, cx| d.released(e, cx)))
            .child(content)
            .children(field)
            .into_any_element()
    }
}

fn trash_item() -> Item {
    let path = fs::trash_files();
    let _ = std::fs::create_dir_all(&path);
    let icon = if fs::trash_is_empty() { "trash-empty" } else { "trash-full" };
    let entry = fs::entry(&path).unwrap_or(Entry {
        path: path.clone(),
        name: "Trash".into(),
        is_dir: true,
        is_link: false,
        size: 0,
        modified: None,
        mode: 0o40755,
        kind: fs::Kind::Folder,
        app_icon: None,
        downloading: None,
        shown: Default::default(),
    });
    Item { entry, label: "Trash".into(), icon }
}
