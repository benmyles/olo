//! DiskUsage: where the space on a disk has gone, and getting it back.
//! Haiku's DiskUsage (a ring chart of the folder, rings outwards for
//! what's inside; click into a folder, click the middle to go back up)
//! beside a list of the folder's contents by size, with a Collector
//! along the bottom, as DaisyDisk has: drag what you don't need into it
//! (or ⌘D), see how much it adds up to, then move it all to the Trash or
//! delete it. Scanning runs on a thread (scan.rs) and the chart grows as
//! it goes.

mod chart;
mod scan;
mod volumes;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, Id, Key, Rect, Sense, Ui, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    files::{format_size, kind_for},
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, Column, ColumnListView, ListState, StatusBar},
};
use olo_theme::mix;
use scan::{NodeId, ROOT, Scan};
use serde::{Deserialize, Serialize};
use volumes::Volume;

const APP_ID: &str = "olo.disk-usage";
const COLLECTOR_HEIGHT: f32 = 58.0;
const CHIP_HEIGHT: f32 = 20.0;

enum Msg {
    Tick,
    Volumes(Vec<Volume>),
    /// Moved to the Trash or deleted: what went, and what couldn't (name,
    /// why).
    Removed { gone: Vec<NodeId>, failed: Vec<(String, String)> },
}

enum Win {
    Main,
    Panel(Box<FilePanel>),
    ConfirmDelete { items: Vec<NodeId>, message: String },
    Error { message: String },
}

/// What comes back after an upgrade or reload.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    root: Option<PathBuf>,
    folder: Option<PathBuf>,
    collected: Vec<PathBuf>,
}

struct DiskUsage {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    volumes: Option<Vec<Volume>>,
    scan: Option<Scan>,
    /// The folder shown.
    folder: NodeId,
    selected: Option<NodeId>,
    list: ListState,
    /// The list's rows: (folder, tree generation, sort) they were made for.
    rows: (NodeId, u64, Option<(usize, bool)>, Vec<NodeId>),
    collected: Vec<NodeId>,
    /// Being dragged towards the Collector.
    drag: Option<NodeId>,
    /// A slice right-clicked, for its menu.
    context: Option<NodeId>,
    /// Moving to the Trash or deleting: what's said meanwhile, and the
    /// flag that stops it.
    busy: Option<(String, Arc<AtomicBool>)>,
    /// To go back to once the scan has found them (after a reload).
    restore: Option<Saved>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// `~` for the home folder.
fn display_path(p: &Path) -> String {
    match p.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// `text`, cut short with "…" to fit `width`.
fn ellipsize(ctx: &egui::Context, text: &str, font: egui::FontId, width: f32) -> String {
    if style::label_width(ctx, text, font.clone()) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if style::label_width(ctx, &candidate, font.clone()) <= width { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

/// 199,111.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

impl DiskUsage {
    fn open(&mut self, cx: &mut Cx) {
        if let Some(id) = self.main {
            return cx.activate(id);
        }
        let id = cx.open_window(WindowOptions::new("DiskUsage", 780, 560).min_size(600, 440));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
        self.read_volumes(cx);
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.redraw(id);
        }
    }

    fn read_volumes(&self, cx: &Cx) {
        let proxy = cx.proxy();
        std::thread::spawn(move || proxy.send(Msg::Volumes(volumes::volumes())));
    }

    fn start_scan(&mut self, cx: &mut Cx, root: PathBuf) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        let proxy = cx.proxy();
        self.scan = Some(scan::start(root.clone(), move || proxy.send(Msg::Tick)));
        self.folder = ROOT;
        self.selected = None;
        self.collected.clear();
        self.list = ListState::default();
        self.rows = (ROOT, u64::MAX, None, Vec::new());
        if let Some(id) = self.main {
            cx.set_title(id, &format!("DiskUsage: {}", display_path(&root)));
        }
        self.redraw(cx);
    }

    fn show_volumes(&mut self, cx: &mut Cx) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        self.collected.clear();
        if let Some(id) = self.main {
            cx.set_title(id, "DiskUsage");
        }
        self.read_volumes(cx);
    }

    fn alert(&mut self, cx: &mut Cx, win: Win, message: &str) {
        let id = cx.open_window(WindowOptions::alert("DiskUsage", message));
        self.windows.insert(id, win);
    }

    /// Run `f` on what `id` is: its path, whether a folder, its name.
    fn item(&self, id: NodeId) -> Option<(PathBuf, bool, String, u64)> {
        let t = self.scan.as_ref()?.tree.read().ok()?;
        let n = t.nodes.get(id as usize)?;
        Some((t.path(id), n.dir, n.name.to_string(), n.size))
    }

    fn enter(&mut self, id: NodeId) {
        let Some(scan) = &self.scan else { return };
        let t = scan.tree.read().unwrap();
        if t.nodes[id as usize].dir && !t.nodes[id as usize].gone {
            drop(t);
            self.folder = id;
            self.selected = None;
            self.list = ListState::default();
        }
    }

    fn up(&mut self) {
        let Some(scan) = &self.scan else { return };
        if self.folder == ROOT {
            return;
        }
        let from = self.folder;
        self.folder = scan.tree.read().unwrap().nodes[from as usize].parent;
        self.selected = Some(from);
        self.list = ListState::default();
    }

    /// Open a file with its application, or go into a folder.
    fn open_item(&mut self, id: NodeId) {
        let Some((path, dir, _, _)) = self.item(id) else { return };
        if dir {
            return self.enter(id);
        }
        if let Err(e) = olo_kit::open::open(&olo_kit::open::Target::Path(path)) {
            tracing::warn!("couldn't open: {e}");
        }
    }

    fn show_in_tracker(&self, id: NodeId) {
        let Some((path, dir, _, _)) = self.item(id) else { return };
        let folder = if dir { path } else { path.parent().map_or_else(home, Path::to_path_buf) };
        if let Some(tracker) = olo_kit::find_program("olo-tracker")
            && let Err(e) = olo_kit::open::spawn(&[tracker.into_os_string(), folder.into()])
        {
            tracing::warn!("couldn't open Tracker: {e}");
        }
    }

    /// Put `id` in the Collector (not the scan's own folder, and not
    /// something already in it, or holding something in it).
    fn collect(&mut self, id: NodeId) {
        let Some(scan) = &self.scan else { return };
        let t = scan.tree.read().unwrap();
        if id == ROOT || t.nodes[id as usize].gone || self.collected.iter().any(|&c| t.is_inside(id, c)) {
            return;
        }
        // Collecting a folder takes in what was collected from inside it.
        self.collected.retain(|&c| !t.is_inside(c, id));
        self.collected.push(id);
    }

    fn collected_size(&self) -> u64 {
        let Some(scan) = &self.scan else { return 0 };
        let t = scan.tree.read().unwrap();
        self.collected.iter().map(|&c| t.nodes[c as usize].size).sum()
    }

    /// Move `items` to the Trash (or delete them) on a thread.
    fn remove(&mut self, cx: &mut Cx, items: Vec<NodeId>, trash: bool) {
        let Some(scan) = &self.scan else { return };
        if items.is_empty() || self.busy.is_some() {
            return;
        }
        let paths: Vec<(NodeId, PathBuf, String)> = {
            let t = scan.tree.read().unwrap();
            items.iter().map(|&i| (i, t.path(i), t.nodes[i as usize].name.to_string())).collect()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let what = if paths.len() == 1 { format!("“{}”", paths[0].2) } else { format!("{} items", paths.len()) };
        let doing = if trash { format!("Moving {what} to the Trash…") } else { format!("Deleting {what}…") };
        self.busy = Some((doing, stop.clone()));
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let (mut gone, mut failed) = (Vec::new(), Vec::new());
            for (id, path, name) in paths {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let result = if trash {
                    olo_kit::trash::move_to_trash(&path).map(|_| ())
                } else if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                match result {
                    Ok(()) => gone.push(id),
                    Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                        failed.push((name, "it is on a different volume from the Trash, so it can only be deleted".into()))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => failed.push((name, "you don't have permission".into())),
                    Err(e) => failed.push((name, e.to_string())),
                }
            }
            proxy.send(Msg::Removed { gone, failed });
        });
        self.redraw(cx);
    }

    fn confirm_delete(&mut self, cx: &mut Cx, items: Vec<NodeId>) {
        if items.is_empty() {
            return;
        }
        let size = {
            let Some(scan) = &self.scan else { return };
            let t = scan.tree.read().unwrap();
            items.iter().map(|&i| t.nodes[i as usize].size).sum::<u64>()
        };
        let what = match items.as_slice() {
            [one] => self.item(*one).map(|(_, _, name, _)| format!("“{name}”")).unwrap_or_default(),
            _ => plural(items.len() as u64, "item", "items"),
        };
        let they = if items.len() == 1 { "It won't" } else { "They won't" };
        let message = format!("Delete {what} ({}) for good? {they} go to the Trash, and this can't be undone.", format_size(size));
        self.alert(cx, Win::ConfirmDelete { items, message: message.clone() }, &message);
    }

    fn command(&mut self, cx: &mut Cx, command: &str) {
        if let Some(path) = command.strip_prefix("scan:") {
            return self.start_scan(cx, PathBuf::from(path));
        }
        if let Some(id) = command.strip_prefix("uncollect:").and_then(|i| i.parse::<NodeId>().ok()) {
            return self.collected.retain(|&c| c != id);
        }
        // Item commands act on the slice right-clicked, else the selection.
        let target = self.context.take().or(self.selected);
        match command {
            "scan-home" => self.start_scan(cx, home()),
            "scan-folder" => {
                let id = cx.open_window(WindowOptions::new("Scan a Folder", PANEL_SIZE.0, PANEL_SIZE.1).min_size(420, 300));
                self.windows.insert(id, Win::Panel(Box::new(FilePanel::folder(home()))));
            }
            "rescan" => {
                if let Some(root) = self.scan.as_ref().map(|s| s.tree.read().unwrap().root.clone()) {
                    let folder = self.item(self.folder).map(|(p, ..)| p);
                    self.start_scan(cx, root);
                    self.restore = Some(Saved { root: None, folder, collected: Vec::new() });
                }
            }
            "stop" => {
                if let Some(s) = &self.scan {
                    s.cancel();
                }
            }
            "volumes" => self.show_volumes(cx),
            "up" => self.up(),
            "open" => {
                if let Some(id) = target {
                    self.open_item(id);
                }
            }
            "show-in-tracker" => {
                if let Some(id) = target {
                    self.show_in_tracker(id);
                }
            }
            "collect" => {
                if let Some(id) = target {
                    self.collect(id);
                }
            }
            "trash" => {
                if let Some(id) = target.filter(|&i| i != ROOT) {
                    self.remove(cx, vec![id], true);
                }
            }
            "delete" => {
                if let Some(id) = target.filter(|&i| i != ROOT) {
                    self.confirm_delete(cx, vec![id]);
                }
            }
            "copy-path" => {
                if let Some((path, ..)) = target.and_then(|id| self.item(id)) {
                    cx.copy(path.display().to_string());
                }
            }
            "collector-trash" => self.remove(cx, self.collected.clone(), true),
            "collector-delete" => self.confirm_delete(cx, self.collected.clone()),
            "collector-clear" => self.collected.clear(),
            "stop-removing" => {
                if let Some((_, stop)) = &self.busy {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            "close" | "quit" => cx.quit(),
            _ => {}
        }
        self.redraw(cx);
    }

    fn item_menu(&self, id: NodeId) -> Menu {
        let (dir, collected) = match self.item(id) {
            Some((_, dir, ..)) => (dir, self.collected.contains(&id)),
            None => (false, false),
        };
        Menu::new("")
            .item(if dir { "Open Folder" } else { "Open" }, "open", None)
            .item("Show in Tracker", "show-in-tracker", None)
            .item("Copy Path", "copy-path", None)
            .separator()
            .item("Add to Collector", "collect", None)
            .enabled(!collected && id != ROOT)
            .item("Move to Trash", "trash", None)
            .enabled(id != ROOT && self.busy.is_none())
            .item("Delete…", "delete", None)
            .enabled(id != ROOT && self.busy.is_none())
    }

    // ---- drawing ------------------------------------------------------

    fn volumes_ui(&mut self, cx: &mut Cx, ui: &mut Ui, area: Rect) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        style::label(painter, area.left(), area.top() + 8.0, Align::Min, "Choose what to scan:", style::bold_font(), c(p.panel_text));
        let row_h = 56.0;
        let mut y = area.top() + 24.0;
        let mut rows: Vec<(String, &str, String, Option<(u64, u64)>, PathBuf)> = Vec::new();
        rows.push(("Home".into(), "home", display_path(&home()), volumes::space(&home()), home()));
        for v in self.volumes.iter().flatten() {
            rows.push((v.name.clone(), "disk", v.mount.display().to_string(), Some((v.total, v.free)), v.mount.clone()));
        }
        let mut chosen = None;
        for (name, icon, place, space, path) in rows {
            let r = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), row_h));
            y += row_h + m::SPACING;
            style::bevel(ui.painter(), r, c(p.shine), c(mix(p.panel_background, p.panel_text, 0.25)));
            let icon_r = Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 16.0), vec2(32.0, 32.0));
            icons::paint(ui.painter(), icon_r, icon);
            let x = icon_r.right() + 10.0;
            let button = Rect::from_min_size(pos2(r.right() - 10.0 - 80.0, r.center().y - m::CONTROL_HEIGHT / 2.0), vec2(80.0, m::CONTROL_HEIGHT));
            // Name and where it is; then how full it is (a volume) or what
            // it holds (home).
            let named = style::label(ui.painter(), x, r.top() + 16.0, Align::Min, &name, style::bold_font(), c(p.panel_text));
            let dim = c(mix(p.panel_text, p.panel_background, 0.45));
            style::label(ui.painter(), named.right() + m::SPACING, r.top() + 16.0, Align::Min, &place, style::plain_font(), dim);
            let right = button.left() - 16.0;
            match (icon, space) {
                ("home", _) => {
                    style::label(ui.painter(), x, r.bottom() - 16.0, Align::Min, "Your own files and folders", style::plain_font(), dim);
                }
                (_, Some((total, free))) => {
                    let free_text = format!("{} free of {}", format_size(free), format_size(total));
                    style::label(ui.painter(), right, r.top() + 16.0, Align::Max, &free_text, style::plain_font(), dim);
                    let bar = Rect::from_min_max(pos2(x, r.bottom() - 22.0), pos2(right, r.bottom() - 10.0));
                    ui.put(bar, StatusBar::new(1.0 - free as f32 / total.max(1) as f32).bar_height(10.0));
                }
                _ => {}
            }
            if ui.put(button, Button::new("Scan")).clicked() {
                chosen = Some(path);
            }
        }
        if self.volumes.is_none() {
            style::label(ui.painter(), area.left(), y + 8.0, Align::Min, "Looking for volumes…", style::plain_font(), c(p.panel_text));
        }
        let folder = Rect::from_min_size(pos2(area.left(), area.bottom() - m::CONTROL_HEIGHT), vec2(140.0, m::CONTROL_HEIGHT));
        if ui.put(folder, Button::new("Scan a Folder…").min_width(140.0)).clicked() {
            self.command(cx, "scan-folder");
        }
        if let Some(path) = chosen {
            self.start_scan(cx, path);
        }
    }

    /// The folder's contents for the list, sorted as asked.
    fn rows(&mut self) -> Vec<NodeId> {
        let Some(scan) = &self.scan else { return Vec::new() };
        let t = scan.tree.read().unwrap();
        let key = (self.folder, t.generation, self.list.sort);
        if (self.rows.0, self.rows.1, self.rows.2) == key {
            return self.rows.3.clone();
        }
        let mut rows = t.children(self.folder, 0);
        if let Some((col, asc)) = self.list.sort {
            let n = |i: &NodeId| &t.nodes[*i as usize];
            match col {
                0 => rows.sort_by(|a, b| n(a).name.to_lowercase().cmp(&n(b).name.to_lowercase())),
                3 => rows.sort_by(|a, b| n(b).files.cmp(&n(a).files)),
                _ => {}
            }
            if asc != (col == 0) {
                rows.reverse();
            }
        }
        self.rows = (key.0, key.1, key.2, rows.clone());
        rows
    }

    fn scan_ui(&mut self, cx: &mut Cx, ui: &mut Ui, area: Rect) {
        let p = style::palette(ui.ctx());
        let Some(scan) = &self.scan else { return };
        let running = scan.running();
        let (tree, progress) = (scan.tree.clone(), scan.progress.clone());

        // The bar along the top: back to the volumes, up, where, and
        // Rescan or Stop.
        let top = Rect::from_min_size(area.min, vec2(area.width(), m::CONTROL_HEIGHT));
        let bw = m::BUTTON_MIN_WIDTH;
        let at = |i: f32, w: f32| Rect::from_min_size(pos2(top.left() + i, top.top()), vec2(w, m::CONTROL_HEIGHT));
        if ui.put(at(0.0, bw), Button::new("Volumes")).clicked() {
            return self.command(cx, "volumes");
        }
        if ui.add_enabled_ui(self.folder != ROOT, |ui| ui.put(at(bw + m::SPACING, bw), Button::new("Up"))).inner.clicked() {
            self.up();
        }
        let action = Rect::from_min_size(pos2(top.right() - bw, top.top()), vec2(bw, m::CONTROL_HEIGHT));
        if ui.put(action, Button::new(if running { "Stop" } else { "Rescan" })).clicked() {
            return self.command(cx, if running { "stop" } else { "rescan" });
        }
        let folder_path = tree.read().unwrap().path(self.folder);
        let where_x = at(2.0 * (bw + m::SPACING) + 4.0, 0.0).left();
        let where_r = Rect::from_min_max(pos2(where_x, top.top()), pos2(action.left() - m::SPACING, top.bottom()));
        let font = style::bold_font();
        let shown = ellipsize(ui.ctx(), &display_path(&folder_path), font.clone(), where_r.width());
        style::label(ui.painter(), where_r.left(), where_r.center().y, Align::Min, &shown, font, c(p.panel_text));

        // The Collector along the bottom, the status line above it.
        let collector = Rect::from_min_max(pos2(area.left(), area.bottom() - COLLECTOR_HEIGHT), area.max);
        let status = Rect::from_min_max(pos2(area.left(), collector.top() - m::SPACING - 2.0 * style::PLAIN_SIZE - 6.0), pos2(area.right(), collector.top() - m::SPACING));
        let body = Rect::from_min_max(pos2(area.left(), top.bottom() + m::SPACING), pos2(area.right(), status.top() - m::SPACING));

        // The chart at the left, as large as fits; the list beside it.
        let side = body.height().min(body.width() * 0.56).floor();
        let chart_r = Rect::from_min_size(body.min, vec2(side, body.height()));
        let list_r = Rect::from_min_max(pos2(chart_r.right() + m::SPACING, body.top()), body.max);

        let geometry = chart::Geometry::fit(chart_r.center(), side / 2.0 - 2.0);
        let slices = chart::layout(&tree.read().unwrap(), self.folder, &p);
        let resp = ui.interact(chart_r, Id::unique("disk-usage-chart"), Sense::click_and_drag());
        let hover = resp.hover_pos().and_then(|pos| chart::hit(&slices, &geometry, pos));
        let on_hub = resp.hover_pos().is_some_and(|pos| geometry.at(pos).is_some_and(|(ring, _)| ring == 0));
        {
            let painter = ui.painter().with_clip_rect(chart_r);
            let line = c(p.panel_background);
            chart::paint(&painter, &geometry, &slices, hover, self.selected, line);
            let hub_fill = if on_hub && self.folder != ROOT { mix(p.document_background, p.list_selected_background, 0.3) } else { p.document_background };
            chart::paint_hub(&painter, &geometry, c(hub_fill), c(mix(p.panel_text, p.panel_background, 0.5)));
            let t = tree.read().unwrap();
            let n = &t.nodes[self.folder as usize];
            let w = geometry.hub * 1.7;
            let title = if self.folder == ROOT && t.root == home() { "Home".to_owned() } else { n.name.to_string() };
            let name = ellipsize(ui.ctx(), &title, style::bold_font(), w);
            style::label(&painter, geometry.center.x, geometry.center.y - 8.0, Align::Center, &name, style::bold_font(), c(p.document_text));
            style::label(&painter, geometry.center.x, geometry.center.y + 8.0, Align::Center, &format_size(n.size), style::plain_font(), c(p.document_text));
            style::describe_item(ui.ctx(), &format!("{title} {}", format_size(n.size)), Rect::from_center_size(geometry.center, vec2(geometry.hub, geometry.hub)), false);
        }
        for s in slices.iter().filter(|s| s.ring == 1) {
            // Agents (and hints) can pick the first ring's slices by name.
            let mid = chart::to_pos(&geometry, 1, (s.start + s.end) / 2.0);
            if s.end - s.start > 0.15 {
                let t = tree.read().unwrap();
                style::describe_item(ui.ctx(), &t.nodes[s.node as usize].name, Rect::from_center_size(mid, vec2(8.0, 8.0)), self.selected == Some(s.node));
            }
        }
        let hovered = hover.map(|i| slices[i].node);
        if resp.clicked() {
            match hovered {
                Some(id) => {
                    self.selected = Some(id);
                    self.enter(id);
                }
                None if on_hub => self.up(),
                None => {}
            }
        }
        if resp.secondary_clicked()
            && let (Some(id), Some(pos)) = (hovered, resp.interact_pointer_pos())
        {
            self.context = Some(id);
            self.selected = Some(id);
            widgets::popup_menu_at(ui.ctx(), Id::unique("disk-usage-menu"), pos, self.item_menu(id));
        }
        if resp.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && let Some(i) = chart::hit(&slices, &geometry, origin)
        {
            self.drag = Some(slices[i].node);
        }

        // The list.
        let rows = self.rows();
        let columns = [Column::new("Name", (list_r.width() - 206.0).max(120.0)), Column::new("Size", 76.0).right(), Column::new("", 50.0), Column::new("Files", 76.0).right()];
        self.list.selected = self.selected.and_then(|s| rows.iter().position(|&r| r == s)).into_iter().collect();
        let (names, total): (Vec<(String, u64, bool, u64, &'static str)>, u64) = {
            let t = tree.read().unwrap();
            let names = rows
                .iter()
                .map(|&r| {
                    let n = &t.nodes[r as usize];
                    let icon = if n.dir { "folder" } else { kind_for(Path::new(&*n.name), false, 0).icon() };
                    (n.name.to_string(), n.size, n.dir, n.files, icon)
                })
                .collect();
            (names, t.nodes[self.folder as usize].size.max(1))
        };
        // Each row's bar in its slice's colour, tying the list to the rings.
        let hues: HashMap<NodeId, egui::Color32> = slices.iter().filter(|s| s.ring == 1).map(|s| (s.node, s.color)).collect();
        let bar_other = c(mix(p.document_text, p.document_background, 0.5));
        let bar_fills: Vec<egui::Color32> = rows.iter().map(|r| hues.get(r).copied().unwrap_or(bar_other)).collect();
        let bar_back = c(mix(p.document_background, p.document_text, 0.12));
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new("disk-usage-list", &columns, rows.len(), &mut self.list)
                    .draggable()
                    .empty_text(if running { "Scanning…" } else { "Empty" })
                    .show(ui, |cell: &Cell| {
                        let (name, size, dir, files, icon) = &names[cell.row];
                        match cell.column {
                            0 => {
                                let icon_r = Rect::from_min_size(pos2(cell.rect.left() + m::CELL_INSET, cell.rect.center().y - 8.0), vec2(16.0, 16.0));
                                icons::paint(cell.painter, icon_r, icon);
                                let r = Rect::from_min_max(pos2(icon_r.right() + 2.0, cell.rect.top()), cell.rect.max);
                                Cell { painter: cell.painter, rect: r, row: cell.row, column: cell.column, selected: cell.selected, text_color: cell.text_color, align: cell.align }.text(name);
                            }
                            1 => cell.text(&format_size(*size)),
                            2 => {
                                // The share of the folder, as a bar.
                                let r = Rect::from_min_size(pos2(cell.rect.left() + 4.0, cell.rect.center().y - 4.0), vec2(cell.rect.width() - 8.0, 8.0));
                                cell.painter.rect_filled(r, 0.0, bar_back);
                                // Whole device pixels long, at least one for
                                // anything at all.
                                let ppp = cell.painter.ctx().pixels_per_point();
                                let w = ((r.width() * (*size as f32 / total as f32) * ppp).round().max(if *size > 0 { 1.0 } else { 0.0 })) / ppp;
                                cell.painter.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), 0.0, bar_fills[cell.row]);
                            }
                            _ => cell.text(&if *dir { count(*files) } else { String::new() }),
                        }
                    })
            })
            .inner;
        if out.selection_changed {
            self.selected = self.list.single().and_then(|r| rows.get(r).copied());
        }
        if let Some(r) = out.activated
            && let Some(&id) = rows.get(r)
        {
            self.open_item(id);
        }
        if let Some((r, pos)) = out.context
            && let Some(&id) = rows.get(r)
        {
            self.selected = Some(id);
            self.context = Some(id);
            widgets::popup_menu_at(ui.ctx(), Id::unique("disk-usage-menu"), pos, self.item_menu(id));
        }
        if let Some((r, _)) = out.drag_started {
            self.drag = rows.get(r).copied();
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("disk-usage-menu")) {
            self.command(cx, &cmd);
        }

        // What the pointer is over, or how the scan is going.
        let dim = c(mix(p.panel_text, p.panel_background, 0.4));
        let (line1, line2) = if let Some((what, stop)) = &self.busy {
            let _ = stop;
            (what.clone(), String::new())
        } else if let Some(id) = hovered.or(self.selected) {
            let t = tree.read().unwrap();
            let n = &t.nodes[id as usize];
            let share = n.size as f64 / total as f64 * 100.0;
            let kind = if n.dir { format!(" — {}", plural(n.files, "file", "files")) } else { String::new() };
            let floor = if n.unreadable { " (some of it couldn't be read)" } else { "" };
            (format!("{} — {} ({share:.1}% of this folder){kind}{floor}", n.name, format_size(n.size)), display_path(&t.path(id)))
        } else if running {
            let files = progress.files.load(Ordering::Relaxed);
            let bytes = progress.bytes.load(Ordering::Relaxed);
            let current = display_path(&progress.current.lock().unwrap());
            (format!("Scanning… {}, {}", plural(files, "file", "files"), format_size(bytes)), current)
        } else {
            let t = tree.read().unwrap();
            let unreadable = progress.unreadable.load(Ordering::Relaxed);
            let n = &t.nodes[self.folder as usize];
            let note = if unreadable > 0 { format!(" — {} couldn't be read", plural(unreadable, "folder", "folders")) } else { String::new() };
            (format!("{} in {}{note}", format_size(n.size), plural(n.files, "file", "files")), "Click a ring to go into a folder, the middle to go back up; drag anything you don't need into the Collector.".into())
        };
        let font = style::plain_font();
        let l1 = ellipsize(ui.ctx(), &line1, font.clone(), status.width());
        let l2 = ellipsize(ui.ctx(), &line2, font.clone(), status.width());
        style::label(ui.painter(), status.left(), status.top() + style::PLAIN_SIZE / 2.0 + 1.0, Align::Min, &l1, font.clone(), c(p.panel_text));
        style::label(ui.painter(), status.left(), status.bottom() - style::PLAIN_SIZE / 2.0 - 1.0, Align::Min, &l2, font, dim);

        self.collector_ui(cx, ui, collector);
        self.drag_ui(ui, collector);
    }

    fn collector_ui(&mut self, cx: &mut Cx, ui: &mut Ui, r: Rect) {
        let p = style::palette(ui.ctx());
        let bw = 112.0;
        let buttons = Rect::from_min_max(pos2(r.right() - bw, r.top()), r.max);
        let well = Rect::from_min_max(r.min, pos2(buttons.left() - m::SPACING, r.bottom()));
        let target = self.drag.is_some() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| well.contains(pos));
        let fill = if target { mix(p.document_background, p.list_selected_background, 0.25) } else { p.document_background };
        ui.painter().rect_filled(well.shrink(2.0), 0.0, c(fill));
        style::sunken(ui.painter(), well, &p, target);
        let inner = well.shrink(6.0);
        let total = self.collected_size();
        let heading = if self.collected.is_empty() {
            "Collector: drag files and folders here".to_owned()
        } else {
            format!("Collector: {} — {}", plural(self.collected.len() as u64, "item", "items"), format_size(total))
        };
        style::label(ui.painter(), inner.left(), inner.top() + 6.0, Align::Min, &heading, style::bold_font(), c(p.document_text));
        // The items, as chips; those that don't fit are counted.
        let mut x = inner.left();
        let y = inner.bottom() - CHIP_HEIGHT;
        let mut dropped = None;
        let items: Vec<(NodeId, String, u64, bool)> = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                self.collected.iter().map(|&i| (i, t.nodes[i as usize].name.to_string(), t.nodes[i as usize].size, t.nodes[i as usize].dir)).collect()
            }
            None => Vec::new(),
        };
        let font = style::small_font();
        for (k, (id, name, size, dir)) in items.iter().enumerate() {
            let text = format!("{name}  {}", format_size(*size));
            let w = (style::label_width(ui.ctx(), &text, font.clone()) + 16.0 + 6.0 + 14.0 + 8.0).min(220.0);
            let left = items.len() - k;
            if x + w > inner.right() - if left > 1 { 60.0 } else { 0.0 } {
                style::label(ui.painter(), x + 2.0, y + CHIP_HEIGHT / 2.0, Align::Min, &format!("+{left} more"), font.clone(), c(p.document_text));
                break;
            }
            let chip = Rect::from_min_size(pos2(x, y), vec2(w, CHIP_HEIGHT));
            ui.painter().rect_filled(chip, 3.0, c(mix(p.panel_background, p.document_background, 0.4)));
            ui.painter().rect_stroke(chip, 3.0, egui::Stroke::new(1.0, c(mix(p.panel_text, p.panel_background, 0.6))), egui::StrokeKind::Inside);
            icons::paint(ui.painter(), Rect::from_min_size(pos2(chip.left() + 3.0, chip.center().y - 8.0), vec2(16.0, 16.0)), if *dir { "folder" } else { "document" });
            let close = Rect::from_center_size(pos2(chip.right() - 9.0, chip.center().y), vec2(12.0, 12.0));
            let text_w = close.left() - chip.left() - 24.0;
            let shown = ellipsize(ui.ctx(), &text, font.clone(), text_w);
            style::label(ui.painter(), chip.left() + 22.0, chip.center().y, Align::Min, &shown, font.clone(), c(p.document_text));
            let x_resp = ui.interact(close, Id::unique(("uncollect", *id)), Sense::click());
            let xc = if x_resp.hovered() { p.panel_text } else { mix(p.panel_text, p.panel_background, 0.4) };
            let stroke = egui::Stroke::new(1.5, c(xc));
            ui.painter().line_segment([close.left_top() + vec2(3.0, 3.0), close.right_bottom() - vec2(3.0, 3.0)], stroke);
            ui.painter().line_segment([close.right_top() + vec2(-3.0, 3.0), close.left_bottom() + vec2(3.0, -3.0)], stroke);
            style::describe_item(ui.ctx(), &format!("Remove {name} from the Collector"), close, false);
            if x_resp.clicked() {
                dropped = Some(*id);
            }
            x += w + 6.0;
        }
        if let Some(id) = dropped {
            self.collected.retain(|&c| c != id);
        }
        let busy = self.busy.is_some();
        let b = |i: f32| Rect::from_min_size(pos2(buttons.left(), buttons.top() + i * (m::CONTROL_HEIGHT + 6.0) + 2.0), vec2(bw, m::CONTROL_HEIGHT));
        if busy {
            if ui.put(b(0.0), Button::new("Stop").min_width(bw)).clicked() {
                self.command(cx, "stop-removing");
            }
            return;
        }
        let any = !self.collected.is_empty();
        if ui.add_enabled_ui(any, |ui| ui.put(b(0.0), Button::new("Move to Trash").min_width(bw))).inner.clicked() {
            self.command(cx, "collector-trash");
        }
        if ui.add_enabled_ui(any, |ui| ui.put(b(1.0), Button::new("Delete…").min_width(bw))).inner.clicked() {
            self.command(cx, "collector-delete");
        }
    }

    /// While something is dragged: its name follows the pointer, and
    /// letting go over the Collector collects it.
    fn drag_ui(&mut self, ui: &mut Ui, collector: Rect) {
        let Some(id) = self.drag else { return };
        let (down, pos) = ui.input(|i| (i.pointer.primary_down() || i.pointer.secondary_down(), i.pointer.hover_pos()));
        if !down {
            self.drag = None;
            if pos.is_some_and(|p| collector.contains(p)) {
                self.collect(id);
            }
            return;
        }
        let (Some(pos), Some((_, dir, name, size))) = (pos, self.item(id)) else { return };
        let p = style::palette(ui.ctx());
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("disk-usage-drag")));
        let text = format!("{name}  {}", format_size(size));
        let w = style::label_width(ui.ctx(), &text, style::plain_font()) + 30.0;
        let r = Rect::from_min_size(pos + vec2(12.0, 6.0), vec2(w, CHIP_HEIGHT + 2.0));
        painter.rect_filled(r, 3.0, c(p.list_selected_background).gamma_multiply(0.92));
        icons::paint(&painter, Rect::from_min_size(pos2(r.left() + 4.0, r.center().y - 8.0), vec2(16.0, 16.0)), if dir { "folder" } else { "document" });
        style::label(&painter, r.left() + 24.0, r.center().y, Align::Min, &text, style::plain_font(), c(p.list_selected_item_text));
        ui.ctx().request_repaint();
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        let area = ui.max_rect().shrink(m::WINDOW_INSET);
        if self.scan.is_some() {
            self.scan_ui(cx, ui, area);
        } else {
            self.volumes_ui(cx, ui, area);
        }
        // Keys the list doesn't take.
        let (back, collect) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, Key::ArrowUp) || i.consume_key(egui::Modifiers::NONE, Key::Backspace),
                i.consume_key(egui::Modifiers::COMMAND, Key::D),
            )
        });
        if back {
            self.up();
        }
        if collect && let Some(id) = self.selected {
            self.collect(id);
        }
    }

    /// Back to where things were before a reload, once the scan has got
    /// that far.
    fn restore(&mut self) {
        let Some(scan) = &self.scan else { return };
        if scan.running() {
            return;
        }
        let Some(saved) = self.restore.take() else { return };
        let t = scan.tree.read().unwrap();
        let folder = saved.folder.as_deref().and_then(|f| t.find(f));
        let collected: Vec<NodeId> = saved.collected.iter().filter_map(|c| t.find(c)).collect();
        drop(t);
        if let Some(f) = folder {
            self.folder = f;
        }
        for c in collected {
            self.collect(c);
        }
    }
}

impl App for DiskUsage {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        match self.windows.get_mut(&window) {
            Some(Win::Main) => self.main_ui(cx, ui),
            Some(Win::Panel(panel)) => {
                let Some(result) = panel.ui(ui) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if let PanelResult::Chosen(path) = result {
                    self.start_scan(cx, path);
                }
            }
            Some(Win::ConfirmDelete { items, message }) => {
                let (items, message) = (items.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Delete"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.remove(cx, items, false);
                }
            }
            Some(Win::Error { message }) => {
                let message = message.clone();
                if widgets::alert(ui, "stop", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if !matches!(self.windows.get(&window), Some(Win::Main)) {
            return None;
        }
        let scanning = self.scan.as_ref().is_some_and(Scan::running);
        let scanned = self.scan.is_some();
        let mut volumes = Menu::new("Scan Volume");
        for v in self.volumes.iter().flatten() {
            volumes = volumes.item(v.name.clone(), format!("scan:{}", v.mount.display()), None);
        }
        let item = self.selected.is_some_and(|s| s != ROOT);
        let busy = self.busy.is_some();
        let any = !self.collected.is_empty();
        Some(vec![
            Menu::new("File")
                .item("Scan Home", "scan-home", Some(Key::H))
                .item("Scan a Folder…", "scan-folder", Some(Key::O))
                .submenu(volumes)
                .separator()
                .item("Rescan", "rescan", Some(Key::R))
                .enabled(scanned && !scanning)
                .item("Stop Scanning", "stop", Some(Key::Period))
                .enabled(scanning)
                .separator()
                .item("Close", "close", Some(Key::W))
                .item("Quit", "quit", Some(Key::Q)),
            Menu::new("Go")
                .item("Enclosing Folder", "up", None)
                .enabled(scanned && self.folder != ROOT)
                .item("Volumes", "volumes", None)
                .enabled(scanned),
            Menu::new("Item")
                .item("Open", "open", None)
                .enabled(item)
                .item("Show in Tracker", "show-in-tracker", None)
                .enabled(item)
                .item("Copy Path", "copy-path", None)
                .enabled(item)
                .separator()
                .item("Add to Collector", "collect", Some(Key::D))
                .enabled(item)
                .item("Move to Trash", "trash", Some(Key::Backspace))
                .enabled(item && !busy)
                .item("Delete…", "delete", None)
                .enabled(item && !busy),
            Menu::new("Collector")
                .item("Move All to Trash", "collector-trash", None)
                .enabled(any && !busy)
                .item("Delete All…", "collector-delete", None)
                .enabled(any && !busy)
                .separator()
                .item("Empty the Collector", "collector-clear", None)
                .enabled(any),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        DiskUsage::command(self, cx, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if matches!(self.windows.get(&window), Some(Win::Main)) {
            return cx.quit();
        }
        self.windows.remove(&window);
        cx.close_window(window);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => {
                self.open(cx);
                if let Some(path) = launch.args.iter().find(|a| !a.starts_with('-')) {
                    let path = launch.cwd.unwrap_or_default().join(path);
                    self.start_scan(cx, path);
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Tick => self.restore(),
            Msg::Volumes(v) => self.volumes = Some(v),
            Msg::Removed { gone, failed } => {
                self.busy = None;
                if let Some(scan) = &self.scan {
                    let mut t = scan.tree.write().unwrap();
                    for &id in &gone {
                        t.remove(id);
                    }
                    let folder_gone = !t.nodes[self.folder as usize].dir || t.ancestry(self.folder).iter().any(|&a| t.nodes[a as usize].gone);
                    drop(t);
                    if folder_gone {
                        self.folder = ROOT;
                    }
                }
                self.collected.retain(|c| !gone.contains(c));
                if self.selected.is_some_and(|s| gone.contains(&s)) {
                    self.selected = None;
                }
                if !failed.is_empty() {
                    let lines: Vec<String> = failed.iter().take(5).map(|(name, why)| format!("“{name}”: {why}.")).collect();
                    let more = if failed.len() > 5 { format!("\n…and {} more.", failed.len() - 5) } else { String::new() };
                    let message = format!("Some items couldn't be removed.\n{}{more}", lines.join("\n"));
                    self.alert(cx, Win::Error { message: message.clone() }, &message);
                }
                self.read_volumes(cx);
            }
        }
        self.redraw(cx);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let saved = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                Saved { root: Some(t.root.clone()), folder: Some(t.path(self.folder)), collected: self.collected.iter().map(|&c| t.path(c)).collect() }
            }
            None => Saved::default(),
        };
        serde_json::to_string(&saved).ok()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut app = DiskUsage {
            windows: HashMap::new(),
            main: None,
            volumes: None,
            scan: None,
            folder: ROOT,
            selected: None,
            list: ListState::default(),
            rows: (ROOT, u64::MAX, None, Vec::new()),
            collected: Vec::new(),
            drag: None,
            context: None,
            busy: None,
            restore: None,
        };
        app.open(cx);
        let saved: Option<Saved> = cx.restored_state().and_then(|s| serde_json::from_str(&s).ok());
        if let Some(saved) = saved
            && let Some(root) = saved.root.clone()
        {
            app.start_scan(cx, root);
            app.restore = Some(saved);
        } else if let Some(path) = args.iter().find(|a| !a.starts_with('-')) {
            app.start_scan(cx, std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path)));
        }
        app
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_show_home_as_tilde() {
        let h = home();
        assert_eq!(display_path(&h), "~");
        assert_eq!(display_path(&h.join("Music")), "~/Music");
        assert_eq!(display_path(Path::new("/usr")), "/usr");
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(count(199_111), "199,111");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(count(999), "999");
    }
}
