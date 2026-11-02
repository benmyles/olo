//! The desktop: a clear background layer showing volumes, Home, the
//! contents of ~/Desktop and the Trash, laid out as on BeOS (volumes down
//! the right-hand side next to the Deskbar, Trash in the bottom-right
//! corner).

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use olo_kit_base::list::SelectionBand;
use olo_ui::{
    Menu, WindowOptions,
    gpui::{
        self, Bounds, Context, DragAction, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels,
        Point, Render, Styled as _, WeakEntity, Window, canvas, div, point, px, size,
    },
    menu, metrics as m,
    style, theme,
};

use crate::{
    Tracker,
    folder::{Action, mods},
    fs::{self, Entry},
    items::{self, DropTarget, ICON_CELL, ItemLayout, Look, Rename, Spring},
    watch,
};

/// Room left for the Deskbar in the top-right corner (its width).
const DESKBAR_WIDTH: f32 = 172.0;
/// From the screen edge to the first column of icons.
const EDGE: f32 = m::SPACING;
const DRAG_SLOP: f32 = 3.0;

/// Icons being moved, by [`place_key`], and where each cell was.
type Placed = Vec<(String, Point<Pixels>)>;

struct Item {
    entry: Entry,
    label: String,
    icon: &'static str,
}

pub struct Desktop {
    items: Vec<Item>,
    /// The Trash, pinned to the bottom-right corner.
    trash: Item,
    selected: BTreeSet<usize>,
    band: Option<(Point<Pixels>, SelectionBand)>,
    /// A ~/Desktop item whose name is being edited in place.
    editing: Option<Rename>,
    focus_after_edit: bool,
    /// Where icons were put by hand (the rest fill the grid around them).
    places: Places,
    /// The screen, as last laid out.
    screen: Bounds<Pixels>,
    /// A drag of icons started here: where the button went down, and
    /// where they were then (to put them where they are dropped).
    dragging: Option<(Point<Pixels>, Placed)>,
    press: Option<Press>,
    /// The items as last drawn.
    layouts: Rc<RefCell<Vec<ItemLayout>>>,
    spring: Rc<RefCell<Spring>>,
    pub app: WeakEntity<Tracker>,
    focus: FocusHandle,
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
    olo_ui::config_dir().join("desktop.json")
}

impl Places {
    fn load() -> Places {
        std::fs::read(places_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Written on a worker (nothing waits for the disk).
    fn save(&self) {
        let Ok(json) = serde_json::to_vec_pretty(self) else { return };
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(olo_ui::config_dir());
            let tmp = places_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, places_path());
            }
        });
    }
}

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
    olo_kit_base::keys::shortcut(command).is_some_and(|s| olo_ui::keys::pressed(&s, k))
}

impl Desktop {
    pub fn new(app: WeakEntity<Tracker>, cx: &mut Context<Self>) -> Self {
        // Made up front so it can be watched from the start.
        let _ = std::fs::create_dir_all(desktop_dir());
        let mut d = Desktop {
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
            layouts: Rc::default(),
            spring: Rc::default(),
            app,
            focus: cx.focus_handle(),
        };
        d.reload();
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
    pub fn edit_name(&mut self, p: &Path, cx: &mut Context<Self>) {
        if let Some(i) = self.items.iter().position(|it| it.entry.path == p) {
            self.selected = BTreeSet::from([i]);
            self.editing = Some(Rename::new(p.to_owned(), self.items[i].entry.is_link, cx));
            cx.notify();
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
    /// (`at`, the pointer).
    pub fn place_dragged(&mut self, at: Point<Pixels>) {
        let Some((from, icons)) = self.dragging.take() else { return };
        let screen = self.screen;
        for (key, was) in icons {
            let to = point(was.x + (at.x - from.x), was.y + (at.y - from.y));
            self.places.icons.insert(key, [f32::from(screen.right() - to.x), f32::from(to.y - screen.top())]);
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

    /// BeOS's Clean Up: every icon back on the grid.
    pub fn clean_up(&mut self) {
        self.places.icons.clear();
        self.places.save();
    }

    fn act(&self, a: Action, cx: &mut Context<Self>) {
        let app = self.app.clone();
        cx.defer(move |cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.desktop_action(a, cx));
            }
        });
    }

    fn end_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(r) = self.editing.take() else { return };
        if commit {
            let name = r.text(cx);
            self.act(Action::Rename(r.path, name), cx);
        }
        self.focus_after_edit = true;
        cx.notify();
    }

    /// The desktop's own commands (its menus' and its keys').
    pub fn command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        match cmd {
            "clean-up" => {
                self.clean_up();
                cx.notify();
            }
            "open" => {
                for e in self.selected_entries() {
                    self.act(Action::Open(e), cx);
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
                self.act(Action::Command(cmd.into()), cx);
            }
            "rename" => {
                if let [p] = self.movable().as_slice() {
                    let p = p.clone();
                    self.edit_name(&p, cx);
                }
            }
            _ => self.act(Action::Command(cmd.into()), cx),
        }
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
                m.item("Get Info", "info").key("i").separator().item("Move to Trash", "trash").key("t").enabled(!self.movable().is_empty())
            }
            None => Menu::new("")
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

    fn press(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.end_rename(true, cx);
        }
        window.focus(&self.focus);
        let pos = e.position;
        let hit = self.hit(pos);
        let (ctrl, shift) = (e.modifiers.control || e.modifiers.platform, e.modifiers.shift);
        if e.button == MouseButton::Left && e.click_count >= 2 {
            if let Some(entry) = hit.and_then(|i| self.item(i)).map(|it| it.entry.clone()) {
                self.act(Action::Open(entry), cx);
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
                    self.band = Some((pos, SelectionBand { base, flip: ctrl }));
                }
                if !(ctrl || shift) {
                    self.selected.clear();
                }
            }
        }
        if let Some(it) = hit.and_then(|i| self.item(i)) {
            let _ = olo_ui::icons::bitmap(it.icon, 0);
        }
        let _ = mods;
        self.press = Some(Press { at: pos, button: e.button, item: hit, was_selected, ctrl, shift, dragging: false });
        cx.notify();
    }

    fn moved(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                let icons = self.selected.iter().filter_map(|&j| Some((place_key(&self.item(j)?.entry.path), layouts.get(j)?.icon.origin - point(px((ICON_CELL.0 - items::ICON) / 2.0), px(m::TIGHT))))).collect();
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
                let id = cx.entity_id();
                items::start(window, cx, id, d);
                cx.notify();
            }
            None if press.button == MouseButton::Left => {
                if let Some((start, base)) = &self.band {
                    let r = Bounds::from_corners(point(start.x.min(e.position.x), start.y.min(e.position.y)), point(start.x.max(e.position.x), start.y.max(e.position.y)));
                    let covered: Vec<usize> = self.layouts.borrow().iter().enumerate().filter(|(_, l)| l.hit().intersects(&r)).map(|(i, _)| i).collect();
                    self.selected = base.over(covered);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn released(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else { return };
        self.band = None;
        if press.dragging {
            cx.notify();
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
                let me = cx.entity();
                menu::context_menu(window, cx, self.menu(hit), e.position, move |cmd, _, cx| me.update(cx, |d, cx| d.command(cmd, cx)));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Keys, when the desktop has the keyboard and no name is being
    /// edited: the menus' keys in the keyboard profile (⌘O opens, ⌘⌫
    /// trashes, ⌘I gets info, ⌘E edits a name, on a Mac), and Return, ⌘↓,
    /// Delete and F2 as well.
    fn key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        } else {
            return;
        };
        if cmd == "trash" && self.movable().is_empty() {
            return;
        }
        cx.stop_propagation();
        self.command(cmd, cx);
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(r) = &mut self.editing {
            if !r.focused {
                r.focused = true;
                let field = r.field.clone();
                field.read(cx).focus(window);
                cx.subscribe_in(&field, window, |d, _, e: &olo_ui::widgets::TextFieldEvent, _, cx| {
                    if matches!(e, olo_ui::widgets::TextFieldEvent::Entered) {
                        d.end_rename(true, cx);
                    } else {
                        cx.notify();
                    }
                })
                .detach();
                let handle = olo_ui::gpui::Focusable::focus_handle(field.read(cx), cx);
                cx.on_focus_out(&handle, window, |d, _, _, cx| d.end_rename(true, cx)).detach();
            }
        } else if self.focus_after_edit {
            self.focus_after_edit = false;
            window.focus(&self.focus);
        }
        let screen = Bounds { origin: point(px(0.0), px(0.0)), size: window.viewport_size() };
        self.screen = screen;
        let cells = self.cells(screen);
        let n = self.items.len() + 1;
        let layouts: Vec<ItemLayout> = (0..n)
            .map(|i| {
                let it = self.item(i).expect("an item");
                items::layout_item(window, cells[i], &it.label, Look::Icons, self.selected.contains(&i), it.entry.is_link)
            })
            .collect();
        *self.layouts.borrow_mut() = layouts.clone();
        let editing = self.editing.as_ref().and_then(|r| self.items.iter().position(|it| it.entry.path == r.path));
        if self.editing.is_some() && editing.is_none() {
            self.editing = None;
        }
        let field = editing.and_then(|i| Some(self.editing.as_ref()?.element(window, cx, &layouts[i], point(px(0.0), px(0.0)))));
        let drawn: Vec<(Entry, &'static str, bool)> = (0..n).map(|i| self.item(i).map(|it| (it.entry.clone(), it.icon, self.selected.contains(&i))).expect("an item")).collect();
        let band = self.band.as_ref().filter(|_| self.press.as_ref().is_some_and(|p| p.item.is_none())).map(|(s, _)| *s);
        let dragging_here = self.dragging.is_some();
        let me = cx.entity();
        let (paint_me, spring) = (me.clone(), self.spring.clone());
        let key = cx.entity_id();
        let desk = desktop_dir();
        let content = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                // Something dragged over it: onto a folder or the Trash, or
                // onto the desktop (into ~/Desktop).
                let mut drop_on = None;
                let mut spring_to = None;
                if let Some(o) = window.drop_offer().cloned() {
                    let at = window.mouse_position();
                    let over = layouts.iter().position(|l| l.hit().contains(&at)).map(|i| (i, &drawn[i].0, drawn[i].2));
                    let (took, on) = items::item_drop(window, cx, key, &o, over, Some(&desk));
                    // Icons dragged from here and let go anywhere but on a
                    // folder or the Trash: put them there.
                    if !took && items::from_view(cx, key) && dragging_here && o.move_ && o.has("text/uri-list") {
                        window.accept_drop(DropTarget::Place.name(), "text/uri-list", DragAction::Move);
                    }
                    drop_on = on.filter(|_| took);
                    spring_to = drop_on.filter(|&i| drawn[i].0.is_dir).map(|i| drawn[i].0.path.clone());
                }
                for (i, l) in layouts.iter().enumerate() {
                    let (_, icon, sel) = &drawn[i];
                    items::paint_item(window, cx, l, icon, *sel || drop_on == Some(i), true, editing != Some(i), &p);
                }
                if let Some(start) = band {
                    let now = window.mouse_position();
                    let r = Bounds::from_corners(point(start.x.min(now.x), start.y.min(now.y)), point(start.x.max(now.x), start.y.max(now.y)));
                    style::selection_band(window, r.intersect(&b), &p);
                }
                let open = spring.borrow_mut().rest(spring_to.as_deref(), window, cx);
                if let Some(dir) = open {
                    paint_me.update(cx, |d, cx| d.act(Action::Spring(dir), cx));
                }
                let (mv, up) = (paint_me.clone(), paint_me.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        mv.update(cx, |d, cx| d.moved(e, window, cx));
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        up.update(cx, |d, cx| d.released(e, window, cx));
                    }
                });
            },
        )
        .size_full();
        let (down, right) = (me.clone(), me.clone());
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, move |e, window, cx| down.update(cx, |d, cx| d.press(e, window, cx)))
            .on_mouse_down(MouseButton::Right, move |e, window, cx| right.update(cx, |d, cx| d.press(e, window, cx)))
            .child(content)
            .children(field)
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
    });
    Item { entry, label: "Trash".into(), icon }
}
