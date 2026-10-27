//! The desktop: a transparent background layer showing volumes, Home,
//! the contents of ~/Desktop and the Trash, laid out as on BeOS (volumes
//! down the right-hand side next to the Deskbar, Trash in the bottom-right
//! corner).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use tabula_kit::{
    Layer, LayerOptions, Menu, WindowKind, WindowOptions,
    egui::{self, Key, Pos2, Rect, Sense, Ui, pos2},
    dnd, metrics as m, style, widgets,
};

use crate::{
    fs::{self, Entry},
    view::{self, Action, ICON_CELL, Look},
    watch,
};

/// Room left for the Deskbar in the top-right corner (its width).
const DESKBAR_WIDTH: f32 = 172.0;
/// From the screen edge to the first column of icons.
const EDGE: f32 = m::SPACING;

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
    band: Option<Pos2>,
    /// A ~/Desktop item whose name is being edited in place.
    editing: Option<(PathBuf, String)>,
    /// Where icons were put by hand (the rest fill the grid around them).
    places: Places,
    /// The screen, as last laid out.
    screen: Rect,
    /// A drag of icons started here: where the button went down, and
    /// where they were then (to put them where they are dropped).
    dragging: Option<(Pos2, Vec<(String, Pos2)>)>,
}

/// Icons placed by hand, by [`place_key`], as the top-left of each one's
/// cell from the screen's top-right corner (where the grid starts, so
/// they keep their place beside the Deskbar when the screen size
/// changes). Kept in `~/.config/tabula/desktop.json`, which agents can
/// read too.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Places {
    #[serde(default)]
    icons: BTreeMap<String, [f32; 2]>,
}

fn places_path() -> PathBuf {
    tabula_kit::config_dir().join("desktop.json")
}

impl Places {
    fn load() -> Places {
        std::fs::read(places_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(tabula_kit::config_dir());
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = places_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, places_path());
            }
        }
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

pub fn options() -> WindowOptions {
    WindowOptions {
        title: "Desktop".into(),
        size: (0, 0),
        kind: WindowKind::Layer(LayerOptions {
            layer: Layer::Background,
            anchor: (true, true, true, true),
            exclusive_zone: -1,
            margin: (0, 0, 0, 0),
            keyboard: tabula_kit::Keyboard::OnDemand,
            namespace: "desktop".into(),
            output: None,
        }),
        panel: false,
        ..Default::default()
    }
}

/// The folders the desktop shows: ~/Desktop, and the Trash (for its icon).
pub fn watched() -> [PathBuf; 2] {
    [desktop_dir(), fs::trash_files()]
}

fn desktop_dir() -> PathBuf {
    fs::home().join("Desktop")
}

impl Desktop {
    pub fn new() -> Self {
        // Made up front so it can be watched from the start.
        let _ = std::fs::create_dir_all(desktop_dir());
        let mut d = Desktop {
            items: Vec::new(),
            trash: trash_item(),
            selected: BTreeSet::new(),
            band: None,
            editing: None,
            places: Places::load(),
            screen: Rect::NOTHING,
            dragging: None,
        };
        d.reload();
        d
    }

    pub fn reload(&mut self) {
        let selected: Vec<PathBuf> = self.selected.iter().filter_map(|&i| self.item(i)).map(|it| it.entry.path.clone()).collect();
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
                let icon = view::icon_of(&entry);
                items.push(Item { label: entry.name.clone(), entry, icon });
            }
        }
        self.items = items;
        self.trash = trash_item();
        self.selected = (0..=self.items.len())
            .filter(|&i| self.item(i).is_some_and(|it| selected.contains(&it.entry.path)))
            .collect();
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
                            self.items[i].icon = view::icon_of(e);
                            self.items[i].entry = e.clone();
                        }
                        (None, Some(e)) if !e.name.starts_with('.') => {
                            let icon = view::icon_of(e);
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
    pub fn edit_name(&mut self, p: &std::path::Path) {
        if let Some(i) = self.items.iter().position(|it| it.entry.path == p) {
            self.selected = BTreeSet::from([i]);
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.editing = Some((p.to_owned(), name));
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries().into_iter().map(|e| e.path).collect()
    }

    /// Selected items that are files or folders on the Desktop (not
    /// volumes, Home or the Trash, which can't be moved to the Trash).
    pub fn selected_entries(&self) -> Vec<Entry> {
        self.selected.iter().filter_map(|&i| self.item(i)).map(|it| it.entry.clone()).collect()
    }

    fn movable(&self) -> Vec<PathBuf> {
        let dir = desktop_dir();
        self.selected_paths().into_iter().filter(|p| p.parent() == Some(dir.as_path())).collect()
    }

    /// Where each item's cell is (the Trash last). Icons placed by hand
    /// stay where they were put (kept on screen); the others fill the
    /// grid around them as on BeOS: columns downward from the top right,
    /// beside the Deskbar, wrapping leftward, with the Trash in the
    /// bottom-right corner unless it was moved.
    fn cells(&self, screen: Rect) -> Vec<Rect> {
        let n = self.items.len() + 1;
        let placed = |i: usize| {
            let p = self.places.icons.get(&place_key(&self.item(i)?.entry.path))?;
            let min = pos2(screen.right() - p[0], screen.top() + p[1]);
            let max = (screen.max - ICON_CELL).max(screen.min);
            Some(Rect::from_min_size(min.clamp(screen.min, max), ICON_CELL))
        };
        let mut cells: Vec<Option<Rect>> = (0..n).map(placed).collect();
        if cells[n - 1].is_none() {
            let min = pos2(screen.right() - EDGE - ICON_CELL.x, screen.bottom() - EDGE - ICON_CELL.y);
            cells[n - 1] = Some(Rect::from_min_size(min, ICON_CELL));
        }
        let usable = screen.height() - 2.0 * EDGE - ICON_CELL.y; // keep the Trash's row free
        let per_col = ((usable / ICON_CELL.y).floor() as usize).max(1);
        let slot = |k: usize| {
            let (col, row) = (k / per_col, k % per_col);
            let right = screen.right() - DESKBAR_WIDTH - EDGE - col as f32 * ICON_CELL.x;
            Rect::from_min_size(pos2(right - ICON_CELL.x, screen.top() + EDGE + row as f32 * ICON_CELL.y), ICON_CELL)
        };
        let mut k = 0;
        for i in 0..n - 1 {
            if cells[i].is_some() {
                continue;
            }
            // The next grid slot no placed icon covers (a few thousand
            // slots at most: past the left edge it gives up and overlaps).
            while k < 4096 && cells.iter().flatten().any(|c| c.shrink(4.0).intersects(slot(k))) {
                k += 1;
            }
            cells[i] = Some(slot(k));
            k += 1;
        }
        cells.into_iter().map(|c| c.unwrap_or(Rect::NOTHING)).collect()
    }

    /// Put the icons that were dragged from here where they were dropped
    /// (`at`, the pointer).
    pub fn place_dragged(&mut self, at: Pos2) {
        let Some((from, icons)) = self.dragging.take() else { return };
        let screen = self.screen;
        for (key, was) in icons {
            let to = was + (at - from);
            self.places.icons.insert(key, [screen.right() - to.x, to.y - screen.top()]);
        }
        self.places.save();
    }

    /// Put the things just dropped here from elsewhere (now in ~/Desktop,
    /// as `names`) where they were dropped, side by side.
    pub fn place_new(&mut self, names: &[String], at: Pos2) {
        let screen = self.screen;
        for (k, name) in names.iter().enumerate() {
            let min = at - ICON_CELL / 2.0 + egui::vec2(k as f32 * ICON_CELL.x, 0.0);
            self.places.icons.insert(name.clone(), [screen.right() - min.x, min.y - screen.top()]);
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

    pub fn ui(&mut self, ui: &mut Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let p = style::palette(ui.ctx());

        let screen = ui.max_rect();
        self.screen = screen;
        let response = ui.allocate_rect(screen, Sense::click_and_drag());
        let n = self.items.len() + 1;
        let cells = self.cells(screen);
        let layouts: Vec<view::ItemLayout> = (0..n)
            .map(|i| {
                let it = self.item(i).unwrap();
                view::layout_item(ui, cells[i], &it.label, Look::Icons, self.selected.contains(&i), it.entry.is_link)
            })
            .collect();
        let hit = |pos: Pos2| layouts.iter().position(|l| l.hit().contains(pos));
        let mods = ui.input(|i| i.modifiers);
        let id = response.id;

        if response.drag_started() || response.clicked() || response.secondary_clicked() {
            // A drag: where the button went down, not where it got to.
            let pos = if response.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { None };
            let pos = pos.or(response.interact_pointer_pos()).unwrap_or_default();
            match hit(pos) {
                Some(i) if mods.ctrl || mods.shift => {
                    if !response.secondary_clicked() && !self.selected.remove(&i) {
                        self.selected.insert(i);
                    }
                }
                Some(i) => {
                    if !self.selected.contains(&i) {
                        self.selected = BTreeSet::from([i]);
                    }
                    if response.drag_started() {
                        // Every icon can be moved about the desktop.
                        let icons = self.selected.iter().filter_map(|&j| Some((place_key(&self.item(j)?.entry.path), cells[j].min))).collect();
                        self.dragging = Some((pos, icons));
                    }
                    // Everything but the Trash can go elsewhere.
                    let paths: Vec<PathBuf> = self.selected.iter().filter(|&&j| j < self.items.len()).map(|&j| self.items[j].entry.path.clone()).collect();
                    if response.drag_started() && (i < self.items.len() && !paths.is_empty() || i == self.items.len()) {
                        let l = &layouts[i];
                        actions.push(Action::Drag(view::Drag {
                            paths,
                            icon: self.item(i).map_or("trash-empty", |it| it.icon),
                            size: l.icon.width(),
                            hotspot: (pos - l.icon.min).clamp(egui::Vec2::ZERO, l.icon.size()),
                            ask: response.dragged_by(egui::PointerButton::Secondary),
                            name: self.item(i).map(|it| it.label.clone()).unwrap_or_default(),
                            below: true,
                        }));
                    }
                }
                None => {
                    if !(mods.ctrl || mods.shift) {
                        self.selected.clear();
                    }
                    if response.drag_started() {
                        self.band = Some(pos);
                    }
                }
            }
        }
        if response.double_clicked()
            && let Some(i) = response.interact_pointer_pos().and_then(hit)
        {
            actions.push(Action::Open(self.item(i).unwrap().entry.clone()));
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let menu = match hit(pos) {
                Some(i) if i == self.items.len() => Menu::new("")
                    .item("Open", "open", None)
                    .item("Empty Trash", "empty-trash", None)
                    .enabled(!fs::trash_is_empty()),
                Some(_) => Menu::new("")
                    .item("Open", "open", None)
                    .item("Get Info", "info", None)
                    .separator()
                    .item("Move to Trash", "trash", None)
                    .enabled(!self.movable().is_empty()),
                None => Menu::new("")
                    .item("New Folder", "new-folder", None)
                    .item("Open in Terminal", "terminal", None)
                    .separator()
                    .item("Clean Up", "clean-up", None)
                    .enabled(!self.places.icons.is_empty())
                    .separator()
                    .item("Change Background…", "backgrounds", None),
            };
            widgets::popup_menu_at(ui.ctx(), id, pos, menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), id) {
            match cmd.as_str() {
                "clean-up" => self.clean_up(),
                "open" => actions.extend(self.selected_entries().into_iter().map(Action::Open)),
                "trash" => {
                    // Only items that live in ~/Desktop can go to the Trash.
                    let keep: BTreeSet<usize> = self
                        .selected
                        .iter()
                        .copied()
                        .filter(|&i| self.item(i).is_some_and(|it| it.entry.path.parent() == Some(desktop_dir().as_path())))
                        .collect();
                    self.selected = keep;
                    actions.push(Action::Command(cmd));
                }
                _ => actions.push(Action::Command(cmd)),
            }
        }

        // Keyboard (when the desktop has focus and no name is being
        // edited): Enter opens, ⌘T trashes, ⌘E / F2 edits a name.
        let typing = self.editing.is_some() || ui.ctx().egui_wants_keyboard_input();
        let mut edit = false;
        ui.input(|i| {
            if typing {
                return;
            }
            for e in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                    match (key, modifiers.ctrl) {
                        (Key::Enter, false) | (Key::ArrowDown, true) => {
                            actions.extend(self.selected_entries().into_iter().map(Action::Open))
                        }
                        (Key::T, true) | (Key::Delete, false) if !self.movable().is_empty() => {
                            actions.push(Action::Command("trash".into()))
                        }
                        (Key::I, true) => actions.push(Action::Command("info".into())),
                        (Key::E, true) | (Key::F2, false) => edit = true,
                        _ => {}
                    }
                }
            }
        });

        if edit && let [p] = self.movable().as_slice() {
            let p = p.clone();
            self.edit_name(&p);
        }

        let band = match (self.band, response.dragged(), response.interact_pointer_pos()) {
            (Some(start), true, Some(now)) => {
                let r = Rect::from_two_pos(start, now);
                self.selected = layouts.iter().enumerate().filter(|(_, l)| l.hit().intersects(r)).map(|(i, _)| i).collect();
                Some(r)
            }
            _ => {
                self.band = None;
                None
            }
        };

        // Something dragged over it: onto a folder or the Trash, or onto
        // the desktop (into ~/Desktop).
        let drop_on = dnd::hover(ui.ctx()).and_then(|h| {
            let over = hit(h.pos).map(|i| (i, &self.item(i).unwrap().entry, self.selected.contains(&i)));
            let here = desktop_dir();
            let (took, on) = view::item_drop(ui.ctx(), &h, over, Some(&here));
            // Icons dragged from here and let go anywhere but on a folder
            // or the Trash: put them there.
            if !took && h.from_here && self.dragging.is_some() && h.actions.move_ && h.has_files() {
                dnd::accept(ui.ctx(), view::DropTarget::Place.name(), "text/uri-list", dnd::Action::Move);
            }
            on.filter(|_| took)
        });

        let editing = self.editing.as_ref().and_then(|(p, _)| self.items.iter().position(|it| &it.entry.path == p));
        for (i, l) in layouts.iter().enumerate() {
            let it = self.item(i).unwrap();
            view::paint_item(ui, l, it.icon, self.selected.contains(&i) || drop_on == Some(i), true, editing != Some(i), &p);
        }
        match editing {
            Some(i) => {
                let link = self.items[i].entry.is_link;
                if let Some(a) = view::edit_label(ui, &layouts[i], &mut self.editing, link) {
                    actions.push(a);
                }
            }
            None => self.editing = None,
        }
        if let Some(r) = band {
            style::selection_band(ui.painter(), r, &p);
        }
        actions
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
    });
    Item { entry, label: "Trash".into(), icon }
}
