//! The desktop: a transparent background layer showing volumes, Home,
//! the contents of ~/Desktop and the Trash, laid out as on BeOS (volumes
//! down the right-hand side next to the Deskbar, Trash in the bottom-right
//! corner).

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use tabula_kit::{
    Layer, LayerOptions, Menu, WindowKind, WindowOptions,
    egui::{self, Key, Pos2, Rect, Sense, Ui, pos2},
    metrics as m, style, widgets,
};

use crate::{
    fs::{self, Entry},
    view::{self, Action, ICON_CELL, Look},
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
    stamp: Option<SystemTime>,
    checked: Instant,
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
            keyboard: true,
            namespace: "desktop".into(),
        }),
        panel: false,
        ..Default::default()
    }
}

fn desktop_dir() -> PathBuf {
    fs::home().join("Desktop")
}

impl Desktop {
    pub fn new() -> Self {
        let mut d = Desktop {
            items: Vec::new(),
            trash: trash_item(),
            selected: BTreeSet::new(),
            band: None,
            stamp: None,
            checked: Instant::now(),
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
        self.stamp = std::fs::metadata(desktop_dir()).and_then(|m| m.modified()).ok();
        self.selected = (0..=self.items.len())
            .filter(|&i| self.item(i).is_some_and(|it| selected.contains(&it.entry.path)))
            .collect();
    }

    /// Items by index; the Trash comes last.
    fn item(&self, i: usize) -> Option<&Item> {
        if i == self.items.len() { Some(&self.trash) } else { self.items.get(i) }
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
        if self.checked.elapsed() >= Duration::from_secs(2) {
            self.checked = Instant::now();
            let stamp = std::fs::metadata(desktop_dir()).and_then(|m| m.modified()).ok();
            let trash_empty = fs::trash_is_empty();
            if stamp != self.stamp || (self.trash.icon == "trash-empty") != trash_empty || self.items.len() != self.count_now() {
                self.reload();
            }
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));

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

        // Keyboard (when the desktop has focus): Enter opens, ⌘T trashes.
        ui.input(|i| {
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
                        _ => {}
                    }
                }
            }
        });

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

        for (i, l) in layouts.iter().enumerate() {
            let it = self.item(i).unwrap();
            view::paint_item(ui, l, it.icon, self.selected.contains(&i), true, &p);
        }
        if let Some(r) = band {
            style::selection_band(ui.painter(), r, &p);
        }
        actions
    }

    fn count_now(&self) -> usize {
        let desktop = std::fs::read_dir(desktop_dir()).map(|rd| rd.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count()).unwrap_or(0);
        fs::volumes().len() + 1 + desktop
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
