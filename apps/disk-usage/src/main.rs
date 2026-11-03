//! DiskUsage: where the space on a disk has gone, and getting it back.
//! Haiku's DiskUsage (a ring chart of the folder, rings outwards for
//! what's inside; click into a folder, click the middle to go back up)
//! beside a list of the folder's contents by size, with a Collector
//! along the bottom, as DaisyDisk has: drag what you don't need into it
//! (or ⌘D), see how much it adds up to, then move it all to the Trash or
//! delete it. Scanning runs on a thread (scan.rs) and the chart grows as
//! it goes. On the GPUI kit (olo-ui).

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

use olo_theme::mix;
use olo_ui::{
    Frame, Menu, MenuBar, WindowOptions,
    file_panel::{self, FilePanel, PanelResult},
    files::{format_size, kind_for},
    gpui::{
        self, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, SharedString, Styled as _, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons,
    instance::{self, Launch},
    menu, metrics as m, style, theme, tracker,
    widgets::{Button, Column, ColumnListView, ListModel, SCROLL_BAR, open_alert},
};
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
    Removed {
        gone: Vec<NodeId>,
        failed: Vec<(String, String)>,
    },
}

/// What comes back after an upgrade or reload.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    root: Option<PathBuf>,
    folder: Option<PathBuf>,
    collected: Vec<PathBuf>,
}

/// The chart's pointer: where it is, and a press that may become a drag.
#[derive(Default, Clone, Copy)]
struct Pointer {
    at: Option<Point<Pixels>>,
    pressed: Option<(Point<Pixels>, Option<NodeId>)>,
}

struct DiskUsage {
    window: Option<WindowHandle<Frame>>,
    volumes: Option<Vec<Volume>>,
    scan: Option<Scan>,
    /// The folder shown.
    folder: NodeId,
    selected: Option<NodeId>,
    list: Entity<ListModel>,
    /// The list's rows: (folder, tree generation, sort) they were made for.
    rows: (NodeId, u64, Option<(usize, bool)>, Vec<NodeId>),
    collected: Vec<NodeId>,
    /// Being dragged towards the Collector.
    drag: Option<NodeId>,
    /// Where the Collector's well was drawn last (a drop's target).
    collector: Bounds<Pixels>,
    pointer: Pointer,
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
    fn proxy(cx: &App) -> olo_ui::Proxy {
        olo_ui::proxy(cx)
    }

    fn read_volumes(&self, cx: &App) {
        let proxy = Self::proxy(cx);
        std::thread::spawn(move || proxy.send(Msg::Volumes(volumes::volumes())));
    }

    /// Retitles the window (deferred: called from the app's updates).
    fn set_title(&self, title: String, cx: &mut Context<Self>) {
        let Some(w) = self.window else { return };
        cx.defer(move |cx| {
            let _ = w.update(cx, |f, window, _| f.set_title(title, window));
        });
    }

    fn start_scan(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        let proxy = Self::proxy(cx);
        self.scan = Some(scan::start(root.clone(), move || proxy.send(Msg::Tick)));
        self.folder = ROOT;
        self.selected = None;
        self.collected.clear();
        self.list.update(cx, |l, _| l.state = Default::default());
        self.rows = (ROOT, u64::MAX, None, Vec::new());
        self.set_title(format!("DiskUsage: {}", display_path(&root)), cx);
        cx.notify();
    }

    fn show_volumes(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        self.collected.clear();
        self.set_title("DiskUsage".into(), cx);
        self.read_volumes(cx);
        cx.notify();
    }

    /// What `id` is: its path, whether a folder, its name and size.
    fn item(&self, id: NodeId) -> Option<(PathBuf, bool, String, u64)> {
        let t = self.scan.as_ref()?.tree.read().ok()?;
        let n = t.nodes.get(id as usize)?;
        Some((t.path(id), n.dir, n.name.to_string(), n.size))
    }

    fn enter(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(scan) = &self.scan else { return };
        let t = scan.tree.read().unwrap();
        if t.nodes[id as usize].dir && !t.nodes[id as usize].gone {
            drop(t);
            self.folder = id;
            self.selected = None;
            self.list.update(cx, |l, _| l.state.selected.clear());
        }
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = &self.scan else { return };
        if self.folder == ROOT {
            return;
        }
        let from = self.folder;
        self.folder = scan.tree.read().unwrap().nodes[from as usize].parent;
        self.selected = Some(from);
        self.list.update(cx, |l, _| l.state.selected.clear());
    }

    /// Opens a file with its application, or goes into a folder.
    fn open_item(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some((path, dir, _, _)) = self.item(id) else { return };
        if dir {
            return self.enter(id, cx);
        }
        if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Path(path)) {
            tracing::warn!("couldn't open: {e}");
        }
    }

    /// A folder in its window, a file selected in its folder's.
    fn show_in_tracker(&self, id: NodeId) {
        let Some((path, _, _, _)) = self.item(id) else { return };
        if let Err(e) = tracker::open(&path) {
            tracing::warn!("couldn't open Tracker: {e}");
        }
    }

    /// Puts `id` in the Collector (not the scan's own folder, and not
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

    /// Moves `items` to the Trash (or deletes them) on a thread.
    fn remove(&mut self, items: Vec<NodeId>, trash: bool, cx: &mut Context<Self>) {
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
        let proxy = Self::proxy(cx);
        std::thread::spawn(move || {
            let (mut gone, mut failed) = (Vec::new(), Vec::new());
            for (id, path, name) in paths {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let result = if trash {
                    olo_xdg::trash::move_to_trash(&path).map(|_| ())
                } else if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                match result {
                    Ok(()) => gone.push(id),
                    Err(e) if e.raw_os_error() == Some(libc::EXDEV) => failed.push((name, "it is on a different volume from the Trash, so it can only be deleted".into())),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => failed.push((name, "you don't have permission".into())),
                    Err(e) => failed.push((name, e.to_string())),
                }
            }
            proxy.send(Msg::Removed { gone, failed });
        });
        cx.notify();
    }

    fn confirm_delete(&mut self, items: Vec<NodeId>, cx: &mut Context<Self>) {
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
        let me = cx.entity();
        cx.defer(move |cx| {
            open_alert(cx, "DiskUsage", "warning", &message, &["Cancel", "Delete"], move |choice, cx| {
                if choice == Some(1) {
                    me.update(cx, |d, cx| d.remove(items.clone(), false, cx));
                }
            });
        });
    }

    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = command.strip_prefix("scan:") {
            return self.start_scan(PathBuf::from(path), cx);
        }
        if let Some(id) = command.strip_prefix("uncollect:").and_then(|i| i.parse::<NodeId>().ok()) {
            self.collected.retain(|&c| c != id);
            return cx.notify();
        }
        // Item commands act on the slice right-clicked, else the selection.
        let target = self.context.take().or(self.selected);
        match command {
            "scan-home" => self.start_scan(home(), cx),
            "scan-folder" => {
                let me = cx.entity();
                cx.defer(move |cx| {
                    file_panel::show(cx, "Scan a Folder", FilePanel::folder(home()), move |a, cx| {
                        if let PanelResult::Chosen(path) = a.result {
                            me.update(cx, |d, cx| d.start_scan(path, cx));
                        }
                    });
                });
            }
            "rescan" => {
                if let Some(root) = self.scan.as_ref().map(|s| s.tree.read().unwrap().root.clone()) {
                    let folder = self.item(self.folder).map(|(p, ..)| p);
                    self.start_scan(root, cx);
                    self.restore = Some(Saved { root: None, folder, collected: Vec::new() });
                }
            }
            "stop" => {
                if let Some(s) = &self.scan {
                    s.cancel();
                }
            }
            "volumes" => self.show_volumes(cx),
            "up" => self.up(cx),
            "open" => {
                if let Some(id) = target {
                    self.open_item(id, cx);
                }
            }
            tracker::COMMAND => {
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
                    self.remove(vec![id], true, cx);
                }
            }
            "delete" => {
                if let Some(id) = target.filter(|&i| i != ROOT) {
                    self.confirm_delete(vec![id], cx);
                }
            }
            "copy-path" => {
                if let Some((path, ..)) = target.and_then(|id| self.item(id)) {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                }
            }
            "collector-trash" => self.remove(self.collected.clone(), true, cx),
            "collector-delete" => self.confirm_delete(self.collected.clone(), cx),
            "collector-clear" => self.collected.clear(),
            "stop-removing" => {
                if let Some((_, stop)) = &self.busy {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            "close" | "quit" => {
                let _ = window;
                cx.quit();
            }
            _ => {}
        }
        cx.notify();
    }

    fn item_menu(&self, id: NodeId) -> Menu {
        let (dir, collected) = match self.item(id) {
            Some((_, dir, ..)) => (dir, self.collected.contains(&id)),
            None => (false, false),
        };
        Menu::new("")
            .item(if dir { "Open Folder" } else { "Open" }, "open")
            .item(tracker::LABEL, tracker::COMMAND)
            .item("Copy Path", "copy-path")
            .separator()
            .item("Add to Collector", "collect")
            .enabled(!collected && id != ROOT)
            .item("Move to Trash", "trash")
            .enabled(id != ROOT && self.busy.is_none())
            .item("Delete…", "delete")
            .enabled(id != ROOT && self.busy.is_none())
    }

    fn menus(&self) -> Vec<Menu> {
        let scanning = self.scan.as_ref().is_some_and(Scan::running);
        let scanned = self.scan.is_some();
        let mut volumes = Menu::new("Scan Volume");
        for v in self.volumes.iter().flatten() {
            volumes = volumes.item(v.name.clone(), format!("scan:{}", v.mount.display()));
        }
        let item = self.selected.is_some_and(|s| s != ROOT);
        let busy = self.busy.is_some();
        let any = !self.collected.is_empty();
        vec![
            Menu::new("File")
                .item("Scan Home", "scan-home")
                .key("h")
                .item("Scan a Folder…", "scan-folder")
                .key("o")
                .submenu(volumes)
                .separator()
                .item("Rescan", "rescan")
                .key("r")
                .enabled(scanned && !scanning)
                .item("Stop Scanning", "stop")
                .key(".")
                .enabled(scanning)
                .separator()
                .item("Close", "close")
                .key("w")
                .item("Quit", "quit")
                .key("q"),
            Menu::new("Go").item("Enclosing Folder", "up").enabled(scanned && self.folder != ROOT).item("Volumes", "volumes").enabled(scanned),
            Menu::new("Item")
                .item("Open", "open")
                .enabled(item)
                .item(tracker::LABEL, tracker::COMMAND)
                .enabled(item)
                .item("Copy Path", "copy-path")
                .enabled(item)
                .separator()
                .item("Add to Collector", "collect")
                .key("d")
                .enabled(item)
                .item("Move to Trash", "trash")
                .key("backspace")
                .enabled(item && !busy)
                .item("Delete…", "delete")
                .enabled(item && !busy),
            Menu::new("Collector")
                .item("Move All to Trash", "collector-trash")
                .enabled(any && !busy)
                .item("Delete All…", "collector-delete")
                .enabled(any && !busy)
                .separator()
                .item("Empty the Collector", "collector-clear")
                .enabled(any),
        ]
    }

    /// The folder's contents for the list, sorted as asked.
    fn rows(&mut self, cx: &App) -> Vec<NodeId> {
        let Some(scan) = &self.scan else { return Vec::new() };
        let t = scan.tree.read().unwrap();
        let sort = self.list.read(cx).state.sort;
        let key = (self.folder, t.generation, sort);
        if (self.rows.0, self.rows.1, self.rows.2) == key {
            return self.rows.3.clone();
        }
        let mut rows = t.children(self.folder, 0);
        if let Some((col, asc)) = sort {
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

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => {
                if let Some(w) = self.window {
                    let _ = w.update(cx, |_, window, _| window.activate_window());
                }
                if let Some(path) = launch.args.iter().find(|a| !a.starts_with('-')) {
                    let path = launch.cwd.unwrap_or_default().join(path);
                    self.start_scan(path, cx);
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
                    cx.defer(move |cx| {
                        open_alert(cx, "DiskUsage", "stop", &message, &["OK"], |_, _| {});
                    });
                }
                self.read_volumes(cx);
            }
        }
        cx.notify();
    }

    fn save_state(&self) -> Option<String> {
        let saved = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                Saved { root: Some(t.root.clone()), folder: Some(t.path(self.folder)), collected: self.collected.iter().map(|&c| t.path(c)).collect() }
            }
            None => Saved::default(),
        };
        serde_json::to_string(&saved).ok()
    }

    // ---- drawing ----------------------------------------------------------

    fn volumes_view(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut rows: Vec<(String, &'static str, String, Option<(u64, u64)>, PathBuf)> = vec![("Home".into(), "home", display_path(&home()), volumes::space(&home()), home())];
        for v in self.volumes.iter().flatten() {
            rows.push((v.name.clone(), "disk", v.mount.display().to_string(), Some((v.total, v.free)), v.mount.clone()));
        }
        let mut out = div().flex().flex_col().gap(px(m::SPACING)).size_full().child(div().h(px(16.0)).child(olo_ui::widgets::StringView::new("Choose what to scan:").bold(true).height(16.0)));
        for (i, (name, icon, place, space, path)) in rows.into_iter().enumerate() {
            let me = cx.entity();
            let scan_it = Button::new("Scan").id(format!("scan-{i}")).on_click(move |_, cx| {
                let path = path.clone();
                me.update(cx, |d, cx| d.start_scan(path, cx));
            });
            let face = canvas(
                |_, _, _| {},
                move |r, _, window, cx| {
                    let p = theme::palette(cx);
                    style::bevel(window, r, style::c(p.shine), style::c(mix(p.panel_background, p.panel_text, 0.25)));
                    let icon_r = Bounds { origin: point(r.left() + px(10.0), r.center().y - px(16.0)), size: size(px(32.0), px(32.0)) };
                    icons::paint(window, icon_r, icon);
                    let x = icon_r.right() + px(10.0);
                    let right = r.right() - px(10.0 + 80.0 + 16.0);
                    // Name and where it is; then how full it is (a volume)
                    // or what it holds (home).
                    let named = style::label(window, cx, x, r.top() + px(16.0), style::Align::Left, name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
                    let dim = style::c(mix(p.panel_text, p.panel_background, 0.45));
                    let plain = style::ui_font(FontWeight::NORMAL);
                    style::label(window, cx, named.right() + px(m::SPACING), r.top() + px(16.0), style::Align::Left, place.clone(), style::PLAIN_SIZE, plain.clone(), dim);
                    match (icon, space) {
                        ("home", _) => {
                            style::label(window, cx, x, r.bottom() - px(16.0), style::Align::Left, "Your own files and folders", style::PLAIN_SIZE, plain, dim);
                        }
                        (_, Some((total, free))) => {
                            let free_text = format!("{} free of {}", format_size(free), format_size(total));
                            style::label(window, cx, right, r.top() + px(16.0), style::Align::Right, free_text, style::PLAIN_SIZE, plain, dim);
                            let bar = Bounds::from_corners(point(x, r.bottom() - px(22.0)), point(right, r.bottom() - px(10.0)));
                            olo_ui::widgets::level_bar(window, bar, 1.0 - free as f32 / total.max(1) as f32, None, &p);
                        }
                        _ => {}
                    }
                },
            )
            .absolute()
            .inset_0();
            out = out.child(div().relative().w_full().h(px(56.0)).flex().items_center().justify_end().pr(px(10.0)).child(face).child(scan_it));
        }
        if self.volumes.is_none() {
            out = out.child(olo_ui::widgets::StringView::new("Looking for volumes…"));
        }
        let me = cx.entity();
        out.child(div().flex_1()).child(Button::new("Scan a Folder…").min_width(140.0).on_click(move |window, cx| me.update(cx, |d, cx| d.command("scan-folder", window, cx))))
    }

    fn scan_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity();
        let p = theme::palette(cx);
        let Some(scan) = &self.scan else { return div() };
        let running = scan.running();
        let (tree, progress) = (scan.tree.clone(), scan.progress.clone());

        // The bar along the top: back to the volumes, up, where, and
        // Rescan or Stop.
        let folder_path = tree.read().unwrap().path(self.folder);
        let (vol, upb, act) = (me.clone(), me.clone(), me.clone());
        let where_text = display_path(&folder_path);
        let top = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .child(Button::new("Volumes").on_click(move |window, cx| vol.update(cx, |d, cx| d.command("volumes", window, cx))))
            .child(Button::new("Up").enabled(self.folder != ROOT).on_click(move |_, cx| {
                upb.update(cx, |d, cx| {
                    d.up(cx);
                    cx.notify();
                })
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        style::label_fit(
                            window,
                            cx,
                            b.left() + px(4.0),
                            b.center().y,
                            style::Align::Left,
                            where_text.clone(),
                            style::PLAIN_SIZE,
                            style::ui_font(FontWeight::BOLD),
                            style::c(p.panel_text),
                            b.size.width - px(4.0),
                        );
                    },
                )
                .flex_1()
                .h_full(),
            )
            .child(Button::new(if running { "Stop" } else { "Rescan" }).on_click(move |window, cx| act.update(cx, |d, cx| d.command(if running { "stop" } else { "rescan" }, window, cx))));

        // The chart at the left, as large as fits; the list beside it.
        let slices = Arc::new(chart::layout(&tree.read().unwrap(), self.folder, &p));
        let folder = self.folder;
        let selected = self.selected;
        let hovered = {
            let at = self.pointer.at;
            at.and_then(|pos| self.chart_geometry(window).and_then(|g| chart::hit(&slices, &g, pos))).map(|i| slices[i].node)
        };
        let chart_el = self.chart(slices.clone(), tree.clone(), folder, selected, cx);

        // The list.
        let rows = self.rows(cx);
        let list_w = f32::from(window.viewport_size().width) - 2.0 * m::WINDOW_INSET;
        let body_h =
            f32::from(window.viewport_size().height) - olo_ui::menu::BAR_HEIGHT - 2.0 * m::WINDOW_INSET - m::CONTROL_HEIGHT - 3.0 * m::SPACING - (2.0 * style::PLAIN_SIZE + 6.0) - COLLECTOR_HEIGHT;
        // On whole device pixels, so the list beside it is too (a box a
        // quarter pixel in leaves layout rounding ties that put the list's
        // scroll bar a pixel off its rows).
        let scale = window.scale_factor();
        let side = (body_h.min(list_w * 0.56) * scale).floor().max(0.0) / scale;
        let list_w = list_w - side - m::SPACING;
        // The name takes what the others leave (inside the list's border and
        // scroll bar), the last filling to the edge.
        let columns = [Column::new("Name", (list_w - 2.0 - SCROLL_BAR - 208.0).max(120.0)), Column::new("Size", 76.0).right(), Column::new("", 50.0), Column::new("Files", 76.0).right()];
        let want: Vec<usize> = self.selected.and_then(|s| rows.iter().position(|&r| r == s)).into_iter().collect();
        self.list.update(cx, |l, _| {
            if l.state.selected.iter().copied().collect::<Vec<_>>() != want {
                l.state.selected = want.into_iter().collect();
            }
        });
        let (names, total): (Vec<(String, u64, bool, u64, &'static str)>, u64) = {
            let t = tree.read().unwrap();
            let names = rows
                .iter()
                .map(|&r| {
                    let n = &t.nodes[r as usize];
                    let icon = icon_for(&n.name, n.dir);
                    (n.name.to_string(), n.size, n.dir, n.files, icon)
                })
                .collect();
            (names, t.nodes[self.folder as usize].size.max(1))
        };
        // Each row's bar in its slice's colour, tying the list to the
        // rings.
        let hues: HashMap<NodeId, Hsla> = slices.iter().filter(|s| s.ring == 1).map(|s| (s.node, s.color)).collect();
        let bar_other = style::c(mix(p.document_text, p.document_background, 0.5));
        let bar_fills: Vec<Hsla> = rows.iter().map(|r| hues.get(r).copied().unwrap_or(bar_other)).collect();
        let bar_back = style::c(mix(p.document_background, p.document_text, 0.12));
        let rows = Arc::new(rows);
        let (sel_rows, open_rows, ctx_rows, drag_rows) = (rows.clone(), rows.clone(), rows.clone(), rows.clone());
        let (sel, open, ctx, drag) = (me.clone(), me.clone(), me.clone(), me.clone());
        let list = ColumnListView::new("disk-usage-list", &self.list, columns, rows.len(), move |cell, window, cx| {
            let (name, size, dir, files, icon) = &names[cell.row];
            match cell.column {
                0 => {
                    let icon_r = Bounds { origin: point(cell.bounds.left() + px(m::CELL_INSET), cell.bounds.center().y - px(8.0)), size: gpui::size(px(16.0), px(16.0)) };
                    icons::paint(window, style::snap(window, icon_r), icon);
                    cell.bounds = Bounds::from_corners(point(icon_r.right() + px(2.0), cell.bounds.top()), cell.bounds.bottom_right());
                    cell.text(window, cx, name.clone());
                }
                1 => cell.text(window, cx, format_size(*size)),
                2 => {
                    // The share of the folder, as a bar.
                    let b = cell.bounds;
                    let r = style::snap(window, Bounds { origin: point(b.left() + px(4.0), b.center().y - px(4.0)), size: gpui::size(b.size.width - px(8.0), px(8.0)) });
                    style::fill_rect(window, r, bar_back);
                    // Whole device pixels long, at least one for anything
                    // at all.
                    let s = window.scale_factor();
                    let w = ((f32::from(r.size.width) * (*size as f32 / total as f32) * s).round().max(if *size > 0 { 1.0 } else { 0.0 })) / s;
                    style::fill_rect(window, Bounds { origin: r.origin, size: gpui::size(px(w), r.size.height) }, bar_fills[cell.row]);
                }
                _ => cell.text(window, cx, if *dir { count(*files) } else { String::new() }),
            }
        })
        .empty_text(if running { "Scanning…" } else { "Empty" })
        .on_select(move |_, cx| {
            let r = sel.read(cx).list.read(cx).state.single();
            sel.update(cx, |d, cx| {
                d.selected = r.and_then(|r| sel_rows.get(r).copied());
                cx.notify();
            })
        })
        .on_open(move |r, _, cx| {
            if let Some(&id) = open_rows.get(r) {
                open.update(cx, |d, cx| {
                    d.open_item(id, cx);
                    cx.notify();
                })
            }
        })
        .on_context(move |r, at, window, cx| {
            let Some(&id) = ctx_rows.get(r) else { return };
            let menu = ctx.update(cx, |d, _| {
                d.selected = Some(id);
                d.context = Some(id);
                d.item_menu(id)
            });
            let to = ctx.clone();
            menu::context_menu(window, cx, menu, at, move |cmd, window, cx| to.update(cx, |d, cx| d.command(cmd, window, cx)));
        })
        .on_drag(move |r, _, cx| {
            if let Some(&id) = drag_rows.get(r) {
                drag.update(cx, |d, cx| {
                    d.drag = Some(id);
                    cx.notify();
                })
            }
        });

        // What the pointer is over, or how the scan is going.
        let (line1, line2) = if let Some((what, _)) = &self.busy {
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
            (
                format!("{} in {}{note}", format_size(n.size), plural(n.files, "file", "files")),
                "Click a ring to go into a folder, the middle to go back up; drag anything you don't need into the Collector.".into(),
            )
        };
        let status = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let font = style::ui_font(FontWeight::NORMAL);
                let dim = style::c(mix(p.panel_text, p.panel_background, 0.4));
                style::label_fit(
                    window,
                    cx,
                    b.left(),
                    b.top() + px(style::PLAIN_SIZE / 2.0 + 1.0),
                    style::Align::Left,
                    line1.clone(),
                    style::PLAIN_SIZE,
                    font.clone(),
                    style::c(p.panel_text),
                    b.size.width,
                );
                style::label_fit(window, cx, b.left(), b.bottom() - px(style::PLAIN_SIZE / 2.0 + 1.0), style::Align::Left, line2.clone(), style::PLAIN_SIZE, font, dim, b.size.width);
            },
        )
        .w_full()
        .h(px(2.0 * style::PLAIN_SIZE + 6.0))
        .flex_shrink_0();

        let collector = self.collector_view(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(top)
            .child(div().flex_1().min_h_0().flex().gap(px(m::SPACING)).child(div().w(px(side)).h_full().child(chart_el)).child(div().flex_1().min_w_0().child(list)))
            .child(status)
            .child(collector)
    }

    /// Where the chart is, last laid out (its box is the chart's).
    fn chart_geometry(&self, _: &Window) -> Option<chart::Geometry> {
        CHART.with(|c| c.get())
    }

    fn chart(&self, slices: Arc<Vec<chart::Slice>>, tree: Arc<olo_sync::RwLock<scan::Tree>>, folder: NodeId, selected: Option<NodeId>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let me = cx.entity();
        let pointer = self.pointer;
        canvas(
            move |b, window, _| {
                let side = f32::from(b.size.width.min(b.size.height));
                let g = chart::Geometry::fit(b.center(), side / 2.0 - 2.0);
                CHART.with(|c| c.set(Some(g)));
                (g, window.insert_hitbox(b, HitboxBehavior::Normal))
            },
            move |b, (g, hitbox): (chart::Geometry, Hitbox), window, cx| {
                let p = theme::palette(cx);
                let hover = pointer.at.filter(|_| hitbox.is_hovered(window)).and_then(|pos| chart::hit(&slices, &g, pos));
                let on_hub = pointer.at.is_some_and(|pos| hitbox.is_hovered(window) && g.at(pos).is_some_and(|(ring, _)| ring == 0));
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    chart::paint(window, &g, &slices, hover, selected, style::c(p.panel_background));
                    let hub_fill = if on_hub && folder != ROOT { mix(p.document_background, p.list_selected_background, 0.3) } else { p.document_background };
                    chart::paint_hub(window, &g, style::c(hub_fill), style::c(mix(p.panel_text, p.panel_background, 0.5)));
                    let t = tree.read().unwrap();
                    let n = &t.nodes[folder as usize];
                    let title = if folder == ROOT && t.root == home() { "Home".to_owned() } else { n.name.to_string() };
                    let w = px(g.hub * 1.7);
                    style::label_fit(
                        window,
                        cx,
                        g.center.x,
                        g.center.y - px(8.0),
                        style::Align::Center,
                        title.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::BOLD),
                        style::c(p.document_text),
                        w,
                    );
                    style::label(
                        window,
                        cx,
                        g.center.x,
                        g.center.y + px(8.0),
                        style::Align::Center,
                        format_size(n.size),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::c(p.document_text),
                    );
                    let hub = Bounds { origin: point(g.center.x - px(g.hub / 2.0), g.center.y - px(g.hub / 2.0)), size: size(px(g.hub), px(g.hub)) };
                    olo_ui::agent::record(
                        window,
                        cx,
                        olo_ui::agent::Node { role: "Button", name: Some(format!("{title} {}", format_size(n.size)).into()), bounds: hub, enabled: true, clickable: true, ..Default::default() },
                    );
                    // Agents (and hints) can pick the first ring's slices by
                    // name.
                    for s in slices.iter().filter(|s| s.ring == 1 && s.end - s.start > 0.15) {
                        let mid = chart::to_pos(&g, 1, (s.start + s.end) / 2.0);
                        let at = Bounds { origin: point(mid.x - px(4.0), mid.y - px(4.0)), size: size(px(8.0), px(8.0)) };
                        olo_ui::agent::record(
                            window,
                            cx,
                            olo_ui::agent::Node {
                                role: "Button",
                                name: Some(SharedString::from(t.nodes[s.node as usize].name.to_string())),
                                bounds: at,
                                enabled: true,
                                clickable: true,
                                selected: Some(selected == Some(s.node)),
                                ..Default::default()
                            },
                        );
                    }
                });

                let (m, sl, hb) = (me.clone(), slices.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let over = hb.is_hovered(window);
                    let at = over.then_some(e.position);
                    let hit_now = at.and_then(|pos| chart::hit(&sl, &g, pos));
                    let hub_now = at.is_some_and(|pos| g.at(pos).is_some_and(|(r, _)| r == 0));
                    m.update(cx, |d, cx| {
                        let before = d.pointer.at;
                        let hit_before = before.and_then(|pos| chart::hit(&sl, &g, pos));
                        let hub_before = before.is_some_and(|pos| g.at(pos).is_some_and(|(r, _)| r == 0));
                        d.pointer.at = at;
                        // A press on a slice that moves: dragged.
                        if let Some((from, Some(node))) = d.pointer.pressed
                            && e.dragging()
                            && f32::from(e.position.x - from.x).hypot(f32::from(e.position.y - from.y)) > 3.0
                        {
                            d.pointer.pressed = None;
                            d.drag = Some(node);
                            cx.notify();
                        }
                        if hit_now != hit_before || hub_now != hub_before {
                            cx.notify();
                        }
                    });
                });
                let (m, sl, hb) = (me.clone(), slices.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !hb.is_hovered(window) {
                        return;
                    }
                    let hit = chart::hit(&sl, &g, e.position).map(|i| sl[i].node);
                    match e.button {
                        MouseButton::Left => m.update(cx, |d, _| d.pointer.pressed = Some((e.position, hit))),
                        MouseButton::Right => {
                            let Some(id) = hit else { return };
                            let menu = m.update(cx, |d, cx| {
                                d.context = Some(id);
                                d.selected = Some(id);
                                cx.notify();
                                d.item_menu(id)
                            });
                            let to = m.clone();
                            menu::context_menu(window, cx, menu, e.position, move |cmd, window, cx| to.update(cx, |d, cx| d.command(cmd, window, cx)));
                        }
                        _ => {}
                    }
                });
                let (m, sl) = (me.clone(), slices.clone());
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left {
                        return;
                    }
                    let Some((_, pressed)) = m.read(cx).pointer.pressed else { return };
                    m.update(cx, |d, cx| {
                        d.pointer.pressed = None;
                        // A click: into the folder clicked, or up from the
                        // middle.
                        let hit = chart::hit(&sl, &g, e.position).map(|i| sl[i].node);
                        match hit.filter(|h| Some(*h) == pressed) {
                            Some(id) => {
                                d.selected = Some(id);
                                d.enter(id, cx);
                            }
                            None if g.at(e.position).is_some_and(|(r, _)| r == 0) => d.up(cx),
                            None => {}
                        }
                        cx.notify();
                    });
                });
            },
        )
        .size_full()
    }

    fn collector_view(&self, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity();
        let total = self.collected_size();
        let heading = if self.collected.is_empty() {
            "Collector: drag files and folders here".to_owned()
        } else {
            format!("Collector: {} — {}", plural(self.collected.len() as u64, "item", "items"), format_size(total))
        };
        let items: Vec<(NodeId, String, u64, bool)> = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                self.collected.iter().map(|&i| (i, t.nodes[i as usize].name.to_string(), t.nodes[i as usize].size, t.nodes[i as usize].dir)).collect()
            }
            None => Vec::new(),
        };
        let dragging = self.drag.is_some();
        let well_me = me.clone();
        let chip_me = me.clone();
        let well = canvas(
            move |b, window, cx| {
                // (Where a drop lands, for the window's drag.)
                well_me.update(cx, |d, _| d.collector = b);
                // Each chip's "×", to take it out.
                let font = style::ui_font(FontWeight::NORMAL);
                let inner = style::inset(window, b, 0.0);
                let inner = Bounds::from_corners(point(inner.left() + px(6.0), inner.top() + px(6.0)), point(inner.right() - px(6.0), inner.bottom() - px(6.0)));
                let mut x = inner.left();
                let y = inner.bottom() - px(CHIP_HEIGHT);
                let mut chips = Vec::new();
                for (k, (id, name, size, dir)) in items.iter().enumerate() {
                    let text = format!("{name}  {}", format_size(*size));
                    let w = (f32::from(style::text_width(window, text.clone(), style::DETAIL_SIZE, font.clone())) + 16.0 + 6.0 + 14.0 + 8.0).min(220.0);
                    let left = items.len() - k;
                    if x + px(w) > inner.right() - px(if left > 1 { 60.0 } else { 0.0 }) {
                        chips.push((None, Bounds { origin: point(x, y), size: size_of(0.0, CHIP_HEIGHT) }, format!("+{left} more"), false, None));
                        break;
                    }
                    let chip = Bounds { origin: point(x, y), size: size_of(w, CHIP_HEIGHT) };
                    let close = Bounds { origin: point(chip.right() - px(15.0), chip.center().y - px(6.0)), size: size_of(12.0, 12.0) };
                    chips.push((Some(*id), chip, text, *dir, Some((close, window.insert_hitbox(close, HitboxBehavior::Normal)))));
                    x += px(w + 6.0);
                }
                (inner, chips)
            },
            move |b, (inner, chips), window, cx| {
                let p = theme::palette(cx);
                let pointer = window.mouse_position();
                let target = dragging && b.contains(&pointer);
                let fill = if target { mix(p.document_background, p.list_selected_background, 0.25) } else { p.document_background };
                style::fill_rect(window, style::inset(window, b, 2.0), style::c(fill));
                style::sunken(window, b, &p, target);
                // In line with the chips' icons.
                style::label(
                    window,
                    cx,
                    inner.left() + px(3.0),
                    inner.top() + px(6.0),
                    style::Align::Left,
                    heading.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::BOLD),
                    style::c(p.document_text),
                );
                let font = style::ui_font(FontWeight::NORMAL);
                for (id, chip, text, dir, close) in &chips {
                    let Some(id) = id else {
                        style::label(window, cx, chip.left() + px(2.0), chip.center().y, style::Align::Left, text.clone(), style::DETAIL_SIZE, font.clone(), style::c(p.document_text));
                        continue;
                    };
                    let chip_r = style::snap(window, *chip);
                    window.paint_quad(gpui::fill(chip_r, style::c(mix(p.panel_background, p.document_background, 0.4))).corner_radii(px(3.0)));
                    window.paint_quad(gpui::outline(chip_r, style::c(mix(p.panel_text, p.panel_background, 0.6)), gpui::BorderStyle::Solid).corner_radii(px(3.0)));
                    icons::paint(
                        window,
                        style::snap(window, Bounds { origin: point(chip.left() + px(3.0), chip.center().y - px(8.0)), size: size_of(16.0, 16.0) }),
                        icon_for(text.split("  ").next().unwrap_or_default(), *dir),
                    );
                    if let Some((close, hit)) = close {
                        style::label_fit(
                            window,
                            cx,
                            chip.left() + px(22.0),
                            chip.center().y,
                            style::Align::Left,
                            text.clone(),
                            style::DETAIL_SIZE,
                            font.clone(),
                            style::c(p.document_text),
                            close.left() - chip.left() - px(24.0),
                        );
                        let xc = if hit.is_hovered(window) { p.panel_text } else { mix(p.panel_text, p.panel_background, 0.4) };
                        let (a, z) = (point(close.left() + px(3.0), close.top() + px(3.0)), point(close.right() - px(3.0), close.bottom() - px(3.0)));
                        window.paint_segment(a, z, px(1.5), true, style::c(xc));
                        window.paint_segment(point(z.x, a.y), point(a.x, z.y), px(1.5), true, style::c(xc));
                        let name = text.split("  ").next().unwrap_or_default().to_owned();
                        olo_ui::agent::record(
                            window,
                            cx,
                            olo_ui::agent::Node {
                                role: "Button",
                                name: Some(format!("Remove {name} from the Collector").into()),
                                bounds: *close,
                                enabled: true,
                                clickable: true,
                                ..Default::default()
                            },
                        );
                        let (hit, id, m) = (hit.clone(), *id, chip_me.clone());
                        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                            if phase.bubble() && e.button == MouseButton::Left && hit.is_hovered(window) {
                                m.update(cx, |d, cx| {
                                    d.collected.retain(|&c| c != id);
                                    cx.notify();
                                });
                            }
                        });
                    }
                }
            },
        )
        .flex_1()
        .h_full();
        let busy = self.busy.is_some();
        let any = !self.collected.is_empty();
        let bw = 112.0;
        let mut buttons = div().w(px(bw)).flex().flex_col().gap(px(6.0)).pt(px(2.0));
        if busy {
            let stop = me.clone();
            buttons = buttons.child(Button::new("Stop").min_width(bw).on_click(move |window, cx| stop.update(cx, |d, cx| d.command("stop-removing", window, cx))));
        } else {
            let (t, d) = (me.clone(), me.clone());
            buttons = buttons
                .child(Button::new("Move to Trash").min_width(bw).enabled(any).on_click(move |window, cx| t.update(cx, |du, cx| du.command("collector-trash", window, cx))))
                .child(Button::new("Delete…").min_width(bw).enabled(any).on_click(move |window, cx| d.update(cx, |du, cx| du.command("collector-delete", window, cx))));
        }
        div().flex().gap(px(m::SPACING)).h(px(COLLECTOR_HEIGHT)).flex_shrink_0().child(well).child(buttons)
    }
}

thread_local! {
    /// The chart's place, last laid out.
    static CHART: std::cell::Cell<Option<chart::Geometry>> = const { std::cell::Cell::new(None) };
}

/// A file's icon by its kind, as Tracker shows it.
fn icon_for(name: &str, dir: bool) -> &'static str {
    if dir { "folder" } else { kind_for(Path::new(name), false, 0).icon() }
}

fn size_of(w: f32, h: f32) -> gpui::Size<Pixels> {
    size(px(w), px(h))
}

impl Render for DiskUsage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let bar_me = me.clone();
        let bar = MenuBar::new(self.menus(), move |cmd, window, cx| bar_me.update(cx, |d, cx| d.command(cmd, window, cx)));
        let content = if self.scan.is_some() { self.scan_view(window, cx) } else { self.volumes_view(cx) };
        // While something is dragged: its name follows the pointer, and
        // letting go over the Collector collects it.
        let mut root = div().size_full().flex().flex_col().child(bar).child(div().flex_1().min_h_0().p(px(m::WINDOW_INSET)).child(content));
        if let Some(id) = self.drag
            && let Some((_, dir, name, size)) = self.item(id)
        {
            let m = me.clone();
            let chip = canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let p = theme::palette(cx);
                    let pos = window.mouse_position();
                    let text = format!("{name}  {}", format_size(size));
                    let font = style::ui_font(FontWeight::NORMAL);
                    let w = f32::from(style::text_width(window, text.clone(), style::PLAIN_SIZE, font.clone())) + 30.0;
                    let r = style::snap(window, Bounds { origin: point(pos.x + px(12.0), pos.y + px(6.0)), size: size_of(w, CHIP_HEIGHT + 2.0) });
                    window.paint_quad(gpui::fill(r, style::c(p.list_selected_background).opacity(0.92)).corner_radii(px(3.0)));
                    icons::paint(window, style::snap(window, Bounds { origin: point(r.left() + px(4.0), r.center().y - px(8.0)), size: size_of(16.0, 16.0) }), icon_for(&name, dir));
                    style::label(window, cx, r.left() + px(24.0), r.center().y, style::Align::Left, text, style::PLAIN_SIZE, font, style::c(p.list_selected_item_text));
                    let m2 = m.clone();
                    window.on_mouse_event(move |_: &MouseMoveEvent, phase, _, cx| {
                        if phase.capture() {
                            m2.update(cx, |_, cx| cx.notify());
                        }
                    });
                    let m3 = m.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        m3.update(cx, |d, cx| {
                            if let Some(id) = d.drag.take()
                                && d.collector.contains(&e.position)
                            {
                                d.collect(id);
                            }
                            cx.notify();
                        });
                    });
                },
            )
            .absolute()
            .inset_0();
            root = root.relative().child(chip);
        }
        // Keys the list doesn't take: up a folder, collect.
        root.on_key_down(move |e: &KeyDownEvent, window, cx| {
            let k = &e.keystroke;
            let up = k.key == "backspace" && k.modifiers.number_of_modifiers() == 0 || olo_kit_base::keys::shortcut("up").is_some_and(|s| olo_ui::keys::pressed(&s, k));
            if up {
                me.update(cx, |d, cx| d.command("up", window, cx));
                cx.stop_propagation();
            }
        })
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let list = ListModel::new(cx, false);
        let saved: Option<Saved> = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok());
        let app = cx.new(|_| DiskUsage {
            window: None,
            volumes: None,
            scan: None,
            folder: ROOT,
            selected: None,
            list,
            rows: (ROOT, u64::MAX, None, Vec::new()),
            collected: Vec::new(),
            drag: None,
            collector: Bounds::default(),
            pointer: Pointer::default(),
            context: None,
            busy: None,
            restore: None,
        });
        let view = app.clone();
        let window = olo_ui::open_window(cx, WindowOptions::new("DiskUsage", 780.0, 560.0).min_size(600.0, 440.0), move |_, _| view);
        app.update(cx, |d, cx| {
            d.window = window;
            d.read_volumes(cx);
            if let Some(saved) = saved
                && let Some(root) = saved.root.clone()
            {
                d.start_scan(root, cx);
                d.restore = Some(saved);
            } else if let Some(path) = args.iter().find(|a| !a.starts_with('-')) {
                d.start_scan(std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path)), cx);
            }
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |d, cx| d.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state());
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
