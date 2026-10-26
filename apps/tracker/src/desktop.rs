//! The desktop: a transparent background layer showing volumes, Home,
//! the contents of ~/Desktop and the Trash, laid out as on BeOS (volumes
//! down the right-hand side next to the Deskbar, Trash in the bottom-right
//! corner).

use std::{
    collections::BTreeSet,
    path::PathBuf,
};

use tabula_kit::{
    Layer, LayerOptions, Menu, WindowKind, WindowOptions,
    egui::{self, Key, Pos2, Rect, Sense, Ui, pos2},
    metrics as m, style, widgets,
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

    /// Cell of item `i`: columns fill downward from the top right, beside
    /// the Deskbar, and wrap leftward; the Trash sits in the corner.
    fn cell(&self, screen: Rect, i: usize) -> Rect {
        if i == self.items.len() {
            let min = pos2(screen.right() - EDGE - ICON_CELL.x, screen.bottom() - EDGE - ICON_CELL.y);
            return Rect::from_min_size(min, ICON_CELL);
        }
        let usable = screen.height() - 2.0 * EDGE - ICON_CELL.y; // keep the Trash's row free
        let per_col = ((usable / ICON_CELL.y).floor() as usize).max(1);
        let (col, row) = (i / per_col, i % per_col);
        let right = screen.right() - DESKBAR_WIDTH - EDGE - col as f32 * ICON_CELL.x;
        Rect::from_min_size(pos2(right - ICON_CELL.x, screen.top() + EDGE + row as f32 * ICON_CELL.y), ICON_CELL)
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let p = style::palette(ui.ctx());

        let screen = ui.max_rect();
        let response = ui.allocate_rect(screen, Sense::click_and_drag());
        let n = self.items.len() + 1;
        let layouts: Vec<view::ItemLayout> = (0..n)
            .map(|i| {
                let it = self.item(i).unwrap();
                view::layout_item(ui, self.cell(screen, i), &it.label, Look::Icons, self.selected.contains(&i), it.entry.is_link)
            })
            .collect();
        let hit = |pos: Pos2| layouts.iter().position(|l| l.hit().contains(pos));
        let mods = ui.input(|i| i.modifiers);
        let id = response.id;

        if response.drag_started() || response.clicked() || response.secondary_clicked() {
            let pos = response.interact_pointer_pos().unwrap_or_default();
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
                    .separator()
                    .item("Change Background…", "backgrounds", None),
            };
            widgets::popup_menu_at(ui.ctx(), id, pos, menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), id) {
            match cmd.as_str() {
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

        let editing = self.editing.as_ref().and_then(|(p, _)| self.items.iter().position(|it| &it.entry.path == p));
        for (i, l) in layouts.iter().enumerate() {
            let it = self.item(i).unwrap();
            view::paint_item(ui, l, it.icon, self.selected.contains(&i), true, editing != Some(i), &p);
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
