//! BFilePanel: the Open, Save and folder panels every app shares. Show one in its
//! own window (`PANEL_SIZE`) and call [`FilePanel::ui`] from that window's
//! `App::ui`; it returns the choice once. Folders are listed on a worker
//! thread, so a slow disk never stalls the window, and listed again the
//! moment they change on disk (`crate::watch`), the selection kept.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use egui::{Align, Key, Rect, Ui, pos2, vec2};

use crate::{
    files::{self, Kind},
    icons,
    menu::Menu,
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Column, ColumnListView, ListState, TextControl},
};

/// A good window size for a panel.
pub const PANEL_SIZE: (u32, u32) = (560, 420);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    Save,
    /// Choose a folder: the selected one, or the one shown.
    Folder,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PanelResult {
    Chosen(PathBuf),
    Cancelled,
}

#[derive(Debug, Clone)]
struct Item {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    kind: Kind,
}

type Listing = Arc<Mutex<Option<(PathBuf, Result<Vec<Item>, String>)>>>;

pub struct FilePanel {
    mode: Mode,
    dir: PathBuf,
    name: String,
    /// Lowercase extensions to offer ("pdf"); empty means everything.
    types: Vec<String>,
    items: Vec<Item>,
    error: Option<String>,
    loading: bool,
    listing: Listing,
    list: ListState,
    /// Asking whether to replace this existing file.
    confirm: Option<PathBuf>,
    started: bool,
    /// The folder shown, followed as it changes (`crate::watch`): `stale`
    /// says to list it again.
    watch: Option<crate::watch::Watch>,
    stale: Arc<std::sync::atomic::AtomicBool>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

impl FilePanel {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self::new(Mode::Open, dir.into(), String::new())
    }

    /// A Save panel in `dir`, proposing `name`.
    pub fn save(dir: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self::new(Mode::Save, dir.into(), name.into())
    }

    /// A panel that picks a folder, starting in `dir`.
    pub fn folder(dir: impl Into<PathBuf>) -> Self {
        Self::new(Mode::Folder, dir.into(), String::new())
    }

    fn new(mode: Mode, dir: PathBuf, name: String) -> Self {
        let dir = if dir.is_dir() { dir } else { home() };
        FilePanel {
            mode,
            dir,
            name,
            types: Vec::new(),
            items: Vec::new(),
            error: None,
            loading: false,
            listing: Arc::new(Mutex::new(None)),
            list: ListState::default(),
            confirm: None,
            started: false,
            watch: None,
            stale: Arc::default(),
        }
    }

    /// Only offer files with these extensions (folders always show).
    pub fn types(mut self, exts: &[&str]) -> Self {
        self.types = exts.iter().map(|e| e.to_lowercase()).collect();
        self
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn matches(&self, it: &Item) -> bool {
        it.is_dir
            || (self.mode != Mode::Folder && self.types.is_empty())
            || self.mode != Mode::Folder && Path::new(&it.name).extension().is_some_and(|e| self.types.contains(&e.to_string_lossy().to_lowercase()))
    }

    fn go(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.dir = dir.clone();
        self.list = ListState::default();
        self.loading = true;
        match &self.watch {
            Some(w) => w.set([dir.clone()]),
            None => {
                let (stale, ctx) = (self.stale.clone(), ctx.clone());
                self.watch = crate::watch::watch([dir.clone()], move |_| {
                    stale.store(true, std::sync::atomic::Ordering::Relaxed);
                    ctx.request_repaint();
                });
            }
        }
        self.list_again(ctx);
    }

    /// List the folder shown (again), off the UI thread.
    fn list_again(&mut self, ctx: &egui::Context) {
        let dir = self.dir.clone();
        let listing = self.listing.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = read(&dir);
            *listing.lock().unwrap() = Some((dir, result));
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<PanelResult> {
        if !self.started {
            self.started = true;
            self.go(ui.ctx(), self.dir.clone());
        }
        if self.stale.swap(false, std::sync::atomic::Ordering::Relaxed) {
            // Changed on disk: listed again, the selection kept by name.
            self.list_again(ui.ctx());
        }
        if let Some((dir, result)) = self.listing.lock().unwrap().take()
            && dir == self.dir
        {
            self.loading = false;
            match result {
                Ok(items) => {
                    let chosen = self.list.single().and_then(|i| self.items.get(i)).map(|it| it.name.clone());
                    self.items = items.into_iter().filter(|i| self.matches(i)).collect();
                    self.list.selected.clear();
                    if let Some(i) = chosen.and_then(|n| self.items.iter().position(|it| it.name == n)) {
                        self.list.select(i);
                    }
                    self.error = None;
                }
                Err(e) => {
                    self.items.clear();
                    self.error = Some(e);
                }
            }
        }
        let p = style::palette(ui.ctx());
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let mut result = None;

        // Top: where we are (a pop-up of the enclosing folders) and Home.
        let top = Rect::from_min_size(full.min, vec2(full.width(), m::CONTROL_HEIGHT));
        let home_w = 64.0;
        let path_r = Rect::from_min_max(top.min, pos2(top.right() - home_w - m::SPACING, top.bottom()));
        let path_resp = ui.put(path_r, Button::new("").min_width(path_r.width()));
        let icon = Rect::from_center_size(pos2(path_r.left() + 16.0, path_r.center().y), vec2(16.0, 16.0));
        icons::paint(ui.painter(), icon, if self.dir == home() { "home" } else { "folder" });
        let title = self.dir.file_name().map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned());
        style::label(ui.painter(), icon.right() + m::TIGHT * 2.0, path_r.center().y, Align::Min, &title, style::plain_font(), c(p.control_text));
        // A small down arrow at the right of the pop-up.
        let ax = path_r.right() - 14.0;
        let ay = path_r.center().y;
        ui.painter().add(egui::Shape::convex_polygon(
            vec![pos2(ax - 4.0, ay - 2.0), pos2(ax + 4.0, ay - 2.0), pos2(ax, ay + 2.0)],
            c(p.control_text),
            egui::Stroke::NONE,
        ));
        let popup_id = ui.make_persistent_id("panel-path");
        if path_resp.clicked() {
            let mut menu = Menu::new("");
            for (k, a) in self.dir.ancestors().enumerate().skip(1) {
                let name = a.file_name().map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned());
                menu = menu.item(name, format!("{k}"), None);
            }
            widgets::popup_menu_at(ui.ctx(), popup_id, path_r.left_bottom(), menu);
        }
        if let Some(k) = widgets::popup_result(ui.ctx(), popup_id).and_then(|s| s.parse::<usize>().ok())
            && let Some(a) = self.dir.ancestors().nth(k)
        {
            let a = a.to_path_buf();
            self.go(ui.ctx(), a);
        }
        let home_r = Rect::from_min_max(pos2(top.right() - home_w, top.top()), top.max);
        if ui.put(home_r, Button::new("Home").min_width(home_w)).clicked() {
            self.go(ui.ctx(), home());
        }

        // Bottom: the name (Save) and the buttons.
        let bottom_h = if self.mode == Mode::Save { 2.0 * m::CONTROL_HEIGHT + m::SPACING } else { m::CONTROL_HEIGHT };
        let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - bottom_h), full.max);
        let list_r = Rect::from_min_max(pos2(full.left(), top.bottom() + m::SPACING), pos2(full.right(), bottom.top() - m::SPACING));

        // The list.
        let columns = [Column::new("Name", list_r.width() * 0.55), Column::new("Size", 80.0).right(), Column::new("Modified", 140.0)];
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_r));
        let items = &self.items;
        let name = |i: usize| items[i].name.as_str();
        let resp = ColumnListView::new(("file-panel", &self.dir), &columns, items.len(), &mut self.list).type_select(&name).show(&mut child, |cell| {
            let Some(it) = items.get(cell.row) else { return };
            match cell.column {
                0 => {
                    let ir = Rect::from_center_size(pos2(cell.rect.left() + m::CELL_INSET + 8.0, cell.rect.center().y), vec2(16.0, 16.0));
                    icons::paint(cell.painter, ir, if it.is_dir { "folder" } else { it.kind.icon() });
                    let r = Rect::from_min_max(pos2(ir.right() + 2.0, cell.rect.top()), cell.rect.max);
                    // (Read by agents too, whole, as the row's name.)
                    let width = (r.width() - m::CELL_INSET).max(0.0);
                    style::label_fit(cell.painter, r.left() + m::TIGHT, r.center().y, Align::Min, &it.name, style::plain_font(), cell.text_color, width);
                }
                1 if !it.is_dir => cell.text(&files::format_size(it.size)),
                1 => cell.text("—"),
                _ => cell.text(&files::format_time(it.modified)),
            }
        });
        if self.loading || self.error.is_some() || (self.items.is_empty() && !self.loading) {
            let msg = match (&self.error, self.loading) {
                (Some(e), _) => e.as_str(),
                (None, true) => "",
                (None, false) => "No items",
            };
            style::label(ui.painter(), list_r.center().x, list_r.center().y, Align::Center, msg, style::plain_font(), style::disabled_text(&p));
        }
        if resp.selection_changed
            && self.mode == Mode::Save
            && let Some(it) = self.list.single().and_then(|i| self.items.get(i))
            && !it.is_dir
        {
            self.name = it.name.clone();
        }
        if let Some(i) = resp.activated
            && let Some(it) = self.items.get(i).cloned()
        {
            if it.is_dir {
                let d = self.dir.join(&it.name);
                self.go(ui.ctx(), d);
            } else if self.mode == Mode::Open {
                result = Some(PanelResult::Chosen(self.dir.join(&it.name)));
            } else if self.mode == Mode::Save {
                self.name = it.name.clone();
                result = self.try_save();
            }
        }

        // Bottom row.
        let mut by = bottom.top();
        if self.mode == Mode::Save {
            let field = Rect::from_min_size(pos2(bottom.left(), by), vec2(bottom.width() * 0.7, m::CONTROL_HEIGHT));
            let mut fchild = ui.new_child(egui::UiBuilder::new().max_rect(field));
            fchild.add(TextControl::new(&mut self.name).label("Save as:").autofocus(true).width(field.width()));
            by += m::CONTROL_HEIGHT + m::SPACING;
        }
        let bw = m::BUTTON_MIN_WIDTH;
        let ok_r = Rect::from_min_size(pos2(bottom.right() - bw, by), vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = ok_r.translate(vec2(-(bw + m::SPACING + 2.0), 0.0));
        if let Some(existing) = self.confirm.clone() {
            let msg = format!("“{}” already exists. Replace it?", existing.file_name().unwrap_or_default().to_string_lossy());
            let mr = Rect::from_min_max(pos2(bottom.left(), by), pos2(cancel_r.left() - m::SPACING, by + m::CONTROL_HEIGHT));
            let mut job = egui::text::LayoutJob::simple_singleline(msg, style::bold_font(), c(p.panel_text));
            job.wrap = egui::text::TextWrapping::truncate_at_width(mr.width());
            let g = ui.painter().layout_job(job);
            let pos = style::text_origin(ui.ctx(), &g, mr.left(), mr.center().y, style::PLAIN_SIZE);
            ui.painter().galley(pos, g, c(p.panel_text));
            if ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                self.confirm = None;
            } else if ui.put(ok_r, Button::new("Replace").default(true)).clicked() {
                result = Some(PanelResult::Chosen(existing));
            }
            return result;
        }
        if ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            result = Some(PanelResult::Cancelled);
        }
        let label = match self.mode {
            Mode::Save => "Save",
            Mode::Open => "Open",
            Mode::Folder => "Select",
        };
        let enabled = match self.mode {
            Mode::Save => !self.name.trim().is_empty(),
            Mode::Open => self.list.single().is_some(),
            Mode::Folder => true,
        };
        let ok = ui.add_enabled_ui(enabled, |ui| ui.put(ok_r, Button::new(label).default(true))).inner;
        if ok.clicked() {
            match self.mode {
                Mode::Save => {
                    if result.is_none() {
                        result = self.try_save();
                    }
                }
                Mode::Folder => {
                    let picked = self.list.single().and_then(|i| self.items.get(i)).map(|it| self.dir.join(&it.name));
                    result = Some(PanelResult::Chosen(picked.unwrap_or_else(|| self.dir.clone())));
                }
                Mode::Open => {
                    if let Some(it) = self.list.single().and_then(|i| self.items.get(i)).cloned() {
                        if it.is_dir {
                            let d = self.dir.join(&it.name);
                            self.go(ui.ctx(), d);
                        } else {
                            result = Some(PanelResult::Chosen(self.dir.join(&it.name)));
                        }
                    }
                }
            }
        }
        result
    }

    /// Save: the path, or ask first if it exists.
    fn try_save(&mut self) -> Option<PanelResult> {
        let name = self.name.trim();
        if name.is_empty() || name.contains('/') {
            return None;
        }
        let path = self.dir.join(name);
        if path.exists() {
            self.confirm = Some(path);
            None
        } else {
            Some(PanelResult::Chosen(path))
        }
    }
}

fn read(dir: &Path) -> Result<Vec<Item>, String> {
    use std::os::unix::fs::PermissionsExt;
    let rd = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "You don't have permission to see this folder.".to_string(),
        _ => e.to_string(),
    })?;
    let mut items: Vec<Item> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let meta = std::fs::metadata(e.path()).ok()?;
            let is_dir = meta.is_dir();
            Some(Item {
                kind: files::kind_for(&e.path(), is_dir, meta.permissions().mode()),
                name,
                is_dir,
                size: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .collect();
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(items)
}
