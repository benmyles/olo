//! DiskUsage: where the space on a disk has gone, and getting it back.
//! Haiku's DiskUsage (a ring chart of the folder, rings outwards for
//! what's inside; click into a folder, click the middle to go back up)
//! beside a list of the folder's contents by size, with a Collector
//! along the bottom, as DaisyDisk has: drag what you don't need into it
//! (or ⌘D), see how much it adds up to, then move it all to the Trash or
//! delete it. Scanning runs on a thread (scan.rs) and the chart grows as
//! it goes. On Olo's own kit (olo-kit): one view on its window's looper,
//! told how the scan is going and what was removed by the threads that
//! do them.

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
    BorderStyle, Bounds, ClickEvent, ClipboardItem, Cx, Element, FontWeight, Hsla, InteractiveElement as _, KeyDownEvent, Menu, MenuBar, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Painter, ParentElement as _, Pixels, Point, SharedString, Size, Styled as _, View, WindowOptions, clipboard, div,
    file_panel::{self, FilePanel, PanelResult},
    files::{format_size, kind_for},
    icons,
    instance::{self, Launch},
    menu, metrics as m, point, px, size, style, theme, tracker,
    undo::{History, Purpose},
    widgets::{Button, Column, ColumnListView, ListModel, SCROLL_BAR, StringView, level_bar, open_alert},
};
use olo_theme::mix;
use scan::{NodeId, ROOT, Scan};
use serde::{Deserialize, Serialize};
use volumes::Volume;

const APP_ID: &str = "olo.disk-usage";
const COLLECTOR_HEIGHT: f32 = 58.0;
const CHIP_HEIGHT: f32 = 20.0;

/// What the window is told: by the scan, its workers, a launch, and
/// what's in it.
enum Msg {
    Tick,
    Volumes(Vec<Volume>),
    /// Moved to the Trash or deleted: what went (and, moved to the Trash,
    /// where it is there), and what couldn't (name, why).
    Removed {
        gone: Vec<NodeId>,
        trashed: Vec<(NodeId, PathBuf)>,
        failed: Vec<(String, String)>,
    },
    /// Put back from the Trash (Undo): each item and where it went, and
    /// what couldn't be.
    PutBack {
        purpose: Purpose,
        back: Vec<(NodeId, PathBuf)>,
        failed: Vec<(String, String)>,
    },
    /// Scan this folder (a launch's, a button's, the panel's).
    Scan(PathBuf),
    /// Deleting these for good was confirmed.
    Delete(Vec<NodeId>),
    Command(String),
    /// The list's selection changed.
    Selected,
    Open(usize),
    Context(usize, Point<Pixels>),
    /// A row dragged off.
    Drag(usize),
}

/// What Edit ▸ Undo takes back: items moved to the Trash (each and where
/// it is there), or put back from it (Redo moves them again). Deleting
/// for good says it can't be undone, and isn't.
#[derive(Debug, Clone)]
enum Step {
    Trashed(Vec<(NodeId, PathBuf)>),
    PutBack(Vec<NodeId>),
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

/// A row as the list draws it: its name, size, whether a folder, its
/// files, its icon and its bar's colour.
struct Shown {
    name: String,
    size: u64,
    dir: bool,
    files: u64,
    icon: &'static str,
    bar: Hsla,
}

struct DiskUsage {
    volumes: Option<Vec<Volume>>,
    scan: Option<Scan>,
    /// The folder shown.
    folder: NodeId,
    selected: Option<NodeId>,
    list: ListModel,
    /// The list's rows: (folder, tree generation, sort) they were made for.
    rows: (NodeId, u64, Option<(usize, bool)>, Vec<NodeId>),
    /// What `body` last laid out, for the drawing and the pointer: the
    /// chart's slices, the list's rows as drawn, the folder's size.
    slices: Arc<Vec<chart::Slice>>,
    shown: Vec<Shown>,
    total: u64,
    /// Counts what the chart and the Collector show changing (their
    /// drawings' keys).
    rev: u64,
    collected: Vec<NodeId>,
    /// Being dragged towards the Collector, and where the pointer is.
    drag: Option<NodeId>,
    drag_at: Point<Pixels>,
    /// Where the Collector's well was drawn last (a drop's target), and
    /// where the pointer is in it.
    collector: Bounds<Pixels>,
    well_at: Option<Point<Pixels>>,
    pointer: Pointer,
    /// A slice right-clicked, for its menu.
    context: Option<NodeId>,
    /// Moving to the Trash or deleting: what's said meanwhile, and the
    /// flag that stops it.
    busy: Option<(String, Arc<AtomicBool>)>,
    /// To go back to once the scan has found them (after a reload).
    restore: Option<Saved>,
    /// Edit ▸ Undo and Redo (this scan's: a new one forgets them).
    history: History<Step>,
    /// Why the move to the Trash under way was made (Redo's, or done).
    removing_as: Option<Purpose>,
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

/// A chip in the Collector's well: what it is (None: "+N more"), where,
/// its words, whether a folder, and its "×".
type Chip = (Option<NodeId>, Bounds<Pixels>, String, bool, Option<Bounds<Pixels>>);

impl DiskUsage {
    fn new(start: Option<(PathBuf, Option<Saved>)>, cx: &mut Cx<Self>) -> DiskUsage {
        let mut d = DiskUsage {
            volumes: None,
            scan: None,
            folder: ROOT,
            selected: None,
            list: ListModel::new(false),
            rows: (ROOT, u64::MAX, None, Vec::new()),
            slices: Arc::default(),
            shown: Vec::new(),
            total: 1,
            rev: 0,
            collected: Vec::new(),
            drag: None,
            drag_at: Point::default(),
            collector: Bounds::default(),
            well_at: None,
            pointer: Pointer::default(),
            context: None,
            busy: None,
            restore: None,
            history: History::default(),
            removing_as: None,
        };
        d.read_volumes(cx);
        if let Some((root, saved)) = start {
            d.start_scan(root, cx);
            d.restore = saved;
        }
        d
    }

    fn read_volumes(&self, cx: &mut Cx<Self>) {
        cx.work(volumes::volumes, Msg::Volumes);
    }

    fn start_scan(&mut self, root: PathBuf, cx: &mut Cx<Self>) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        let to = cx.messenger();
        self.scan = Some(scan::start(root.clone(), move || {
            to.send(Msg::Tick);
        }));
        self.folder = ROOT;
        self.selected = None;
        self.collected.clear();
        // (Its steps name the last scan's items.)
        self.history.clear();
        *self.list.state_mut() = Default::default();
        self.rows = (ROOT, u64::MAX, None, Vec::new());
        cx.set_window_title(format!("DiskUsage: {}", display_path(&root)));
        cx.invalidate();
    }

    fn show_volumes(&mut self, cx: &mut Cx<Self>) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        self.collected.clear();
        cx.set_window_title("DiskUsage");
        self.read_volumes(cx);
        cx.invalidate();
    }

    /// What `id` is: its path, whether a folder, its name and size.
    fn item(&self, id: NodeId) -> Option<(PathBuf, bool, String, u64)> {
        let t = self.scan.as_ref()?.tree.read().ok()?;
        let n = t.nodes.get(id as usize)?;
        Some((t.path(id), n.dir, n.name.to_string(), n.size))
    }

    fn enter(&mut self, id: NodeId, _: &mut Cx<Self>) {
        let Some(scan) = &self.scan else { return };
        let t = scan.tree.read().unwrap();
        if t.nodes[id as usize].dir && !t.nodes[id as usize].gone {
            drop(t);
            self.folder = id;
            self.selected = None;
            self.list.state_mut().selected.clear();
        }
    }

    fn up(&mut self, _: &mut Cx<Self>) {
        let Some(scan) = &self.scan else { return };
        if self.folder == ROOT {
            return;
        }
        let from = self.folder;
        self.folder = scan.tree.read().unwrap().nodes[from as usize].parent;
        self.selected = Some(from);
        self.list.state_mut().selected.clear();
    }

    /// Opens a file with its application, or goes into a folder.
    fn open_item(&mut self, id: NodeId, cx: &mut Cx<Self>) {
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
    fn remove(&mut self, items: Vec<NodeId>, trash: bool, cx: &mut Cx<Self>) {
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
        let to = cx.messenger();
        std::thread::spawn(move || {
            let (mut gone, mut trashed, mut failed) = (Vec::new(), Vec::new(), Vec::new());
            for (id, path, name) in paths {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let result = if trash {
                    olo_xdg::trash::move_to_trash(&path).map(|at| trashed.push((id, at)))
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
            to.send(Msg::Removed { gone, trashed, failed });
        });
        cx.invalidate();
    }

    /// Edit ▸ Undo (`redo` false) or Redo: items moved to the Trash put
    /// back (on a thread), or moved there again.
    fn undo_redo(&mut self, redo: bool, cx: &mut Cx<Self>) {
        if self.busy.is_some() || self.scan.is_none() {
            return;
        }
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        match step.change {
            Step::Trashed(items) => {
                let stop = Arc::new(AtomicBool::new(false));
                let what = if items.len() == 1 { "“".to_owned() + &items[0].1.file_name().unwrap_or_default().to_string_lossy() + "”" } else { format!("{} items", items.len()) };
                self.busy = Some((format!("Putting {what} back…"), stop.clone()));
                let to = cx.messenger();
                std::thread::spawn(move || {
                    let (mut back, mut failed) = (Vec::new(), Vec::new());
                    for (id, at) in items {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        match olo_xdg::trash::put_back(&at) {
                            Ok(to) => back.push((id, to)),
                            Err(e) => failed.push((at.file_name().unwrap_or_default().to_string_lossy().into_owned(), e.to_string())),
                        }
                    }
                    to.send(Msg::PutBack { purpose, back, failed });
                });
            }
            Step::PutBack(ids) => {
                self.removing_as = Some(purpose);
                self.remove(ids, true, cx);
                if self.busy.is_none() {
                    self.removing_as = None;
                }
            }
        }
        cx.invalidate();
    }

    fn record(&mut self, purpose: Purpose, step: Step) {
        match purpose {
            Purpose::Do => self.history.done("Move to Trash", step),
            p => self.history.record(p, "Move to Trash", step),
        }
    }

    fn confirm_delete(&mut self, items: Vec<NodeId>, cx: &mut Cx<Self>) {
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
        let to = cx.messenger();
        open_alert(cx, "DiskUsage", "warning", &message, &["Cancel", "Delete"], move |choice, _| {
            if *choice == Some(1) {
                to.send(Msg::Delete(items.clone()));
            }
        });
    }

    fn do_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        if let Some(path) = command.strip_prefix("scan:") {
            return self.start_scan(PathBuf::from(path), cx);
        }
        if let Some(id) = command.strip_prefix("uncollect:").and_then(|i| i.parse::<NodeId>().ok()) {
            self.collected.retain(|&c| c != id);
            return cx.invalidate();
        }
        // Item commands act on the slice right-clicked, else the selection.
        let target = self.context.take().or(self.selected);
        match command {
            "scan-home" => self.start_scan(home(), cx),
            "undo" => self.undo_redo(false, cx),
            "redo" => self.undo_redo(true, cx),
            "scan-folder" => {
                // (The panel answers on its own looper: it tells this
                // window.)
                let to = cx.messenger();
                file_panel::show("Scan a Folder", FilePanel::folder(home()), move |a| {
                    if let PanelResult::Chosen(path) = a.result {
                        to.send(Msg::Scan(path));
                    }
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
            "open-item" => {
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
                    clipboard::write(ClipboardItem::new_string(path.display().to_string()));
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
            "close" | "quit" => olo_kit::quit(),
            _ => {}
        }
        cx.invalidate();
    }

    fn item_menu(&self, id: NodeId) -> Menu {
        let (dir, collected) = match self.item(id) {
            Some((_, dir, ..)) => (dir, self.collected.contains(&id)),
            None => (false, false),
        };
        Menu::new("")
            .item(if dir { "Open Folder" } else { "Open" }, "open-item")
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
                .app_items(&[])
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
                .close_quit(),
            self.history.edit_items(Menu::new("Edit")),
            Menu::new("Go").item("Enclosing Folder", "up").enabled(scanned && self.folder != ROOT).item("Volumes", "volumes").enabled(scanned),
            Menu::new("Item")
                .item("Open", "open-item")
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
    fn rows(&mut self) -> Vec<NodeId> {
        let Some(scan) = &self.scan else { return Vec::new() };
        let t = scan.tree.read().unwrap();
        let sort = self.list.state().sort;
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

    /// What couldn't be removed or put back, said in an alert.
    fn say_failed(&self, what: &str, failed: &[(String, String)], cx: &mut Cx<Self>) {
        if failed.is_empty() {
            return;
        }
        let lines: Vec<String> = failed.iter().take(5).map(|(name, why)| format!("“{name}”: {why}.")).collect();
        let more = if failed.len() > 5 { format!("\n…and {} more.", failed.len() - 5) } else { String::new() };
        let message = format!("Some items couldn't be {what}.\n{}{more}", lines.join("\n"));
        open_alert(cx, "DiskUsage", "stop", &message, &["OK"], |_, _| {});
    }

    // ---- drawing ----------------------------------------------------------

    fn volumes_view(&self, cx: &mut Cx<Self>) -> Element {
        let mut rows: Vec<(String, &'static str, String, Option<(u64, u64)>, PathBuf)> = vec![("Home".into(), "home", display_path(&home()), volumes::space(&home()), home())];
        for v in self.volumes.iter().flatten() {
            rows.push((v.name.clone(), "disk", v.mount.display().to_string(), Some((v.total, v.free)), v.mount.clone()));
        }
        let mut out = div().flex().flex_col().gap(px(m::SPACING)).size_full().child(div().h(px(16.0)).child(StringView::new("Choose what to scan:").bold(true).height(16.0)));
        for (i, (name, icon, place, space, path)) in rows.into_iter().enumerate() {
            let scan_it = Button::new("Scan").id(format!("scan-{i}")).on_click(cx.msg_with(move |_: &ClickEvent| Msg::Scan(path.clone())));
            let look = style::key((&name, icon, &place, space));
            let face = olo_kit::canvas(move |g, r| {
                let p = theme::palette();
                style::bevel(g, r, style::c(p.shine), style::c(mix(p.panel_background, p.panel_text, 0.25)));
                let icon_r = Bounds { origin: point(r.left() + px(10.0), r.center().y - px(16.0)), size: size(px(32.0), px(32.0)) };
                icons::paint(g, icon_r, icon);
                let x = icon_r.right() + px(10.0);
                let right = r.right() - px(10.0 + 80.0 + 16.0);
                // Name and where it is; then how full it is (a volume)
                // or what it holds (home).
                let named = style::label(g, x, r.top() + px(16.0), style::Align::Left, name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
                let dim = style::c(mix(p.panel_text, p.panel_background, 0.45));
                let plain = style::ui_font(FontWeight::NORMAL);
                style::label(g, named.right() + px(m::SPACING), r.top() + px(16.0), style::Align::Left, place.clone(), style::PLAIN_SIZE, plain.clone(), dim);
                match (icon, space) {
                    ("home", _) => {
                        style::label(g, x, r.bottom() - px(16.0), style::Align::Left, "Your own files and folders", style::PLAIN_SIZE, plain, dim);
                    }
                    (_, Some((total, free))) => {
                        let free_text = format!("{} free of {}", format_size(free), format_size(total));
                        style::label(g, right, r.top() + px(16.0), style::Align::Right, free_text, style::PLAIN_SIZE, plain, dim);
                        let bar = Bounds::from_corners(point(x, r.bottom() - px(22.0)), point(right, r.bottom() - px(10.0)));
                        level_bar(g, bar, 1.0 - free as f32 / total.max(1) as f32, None, &p);
                    }
                    _ => {}
                }
            })
            .paint_key(look)
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full();
            out = out.child(div().relative().w_full().h(px(56.0)).flex().items_center().justify_end().pr(px(10.0)).child(face).child(scan_it));
        }
        if self.volumes.is_none() {
            out = out.child(StringView::new("Looking for volumes…"));
        }
        out.child(div().flex_1()).child(Button::new("Scan a Folder…").min_width(140.0).on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("scan-folder".into()))))
    }

    /// The chart's place in its box.
    fn geometry(b: Bounds<Pixels>) -> chart::Geometry {
        let side = f32::from(b.size.width.min(b.size.height));
        chart::Geometry::fit(b.center(), side / 2.0 - 2.0)
    }

    fn scan_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let Some(scan) = &self.scan else { return div() };
        let running = scan.running();
        let (tree, progress) = (scan.tree.clone(), scan.progress.clone());

        // The bar along the top: back to the volumes, up, where, and
        // Rescan or Stop.
        let folder_path = tree.read().unwrap().path(self.folder);
        let where_text = display_path(&folder_path);
        let look = style::key(&where_text);
        let top = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .child(Button::new("Volumes").on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("volumes".into()))))
            .child(Button::new("Up").enabled(self.folder != ROOT).on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("up".into()))))
            .child(
                olo_kit::canvas(move |g, b| {
                    let p = theme::palette();
                    style::label_fit(
                        g,
                        b.left() + px(4.0),
                        b.center().y,
                        style::Align::Left,
                        where_text.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::BOLD),
                        style::c(p.panel_text),
                        b.size.width - px(4.0),
                    );
                })
                .paint_key(look)
                .flex_1()
                .h_full(),
            )
            .child(Button::new(if running { "Stop" } else { "Rescan" }).on_click(cx.msg_with(move |_: &ClickEvent| Msg::Command(if running { "stop" } else { "rescan" }.into()))));

        // The chart at the left, as large as fits; the list beside it.
        self.slices = Arc::new(chart::layout(&tree.read().unwrap(), self.folder, &p));
        self.rev += 1;
        let chart_el = cx
            .canvas(|d: &DiskUsage, g, b| d.paint_chart(g, b))
            .id("disk-usage-chart")
            .size_full()
            .on_mouse_move(cx.listener(|d: &mut Self, e: &MouseMoveEvent, cx| d.chart_moved(Some(e), cx)))
            .on_hover(cx.listener(|d: &mut Self, over: &bool, cx| {
                if !over {
                    d.chart_moved(None, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|d: &mut Self, e: &MouseDownEvent, cx| d.chart_pressed(e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|d: &mut Self, e: &MouseDownEvent, cx| d.chart_pressed(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|d: &mut Self, e: &MouseUpEvent, cx| d.chart_released(e, cx)));

        // The list.
        let rows = self.rows();
        let viewport = cx.viewport_size();
        let list_w = f32::from(viewport.width) - 2.0 * m::WINDOW_INSET;
        let body_h = f32::from(viewport.height) - menu::BAR_HEIGHT - 2.0 * m::WINDOW_INSET - m::CONTROL_HEIGHT - 3.0 * m::SPACING - (2.0 * style::PLAIN_SIZE + 6.0) - COLLECTOR_HEIGHT;
        // On whole device pixels, so the list beside it is too (a box a
        // quarter pixel in leaves layout rounding ties that put the list's
        // scroll bar a pixel off its rows).
        let scale = cx.scale_factor();
        let side = (body_h.min(list_w * 0.56) * scale).floor().max(0.0) / scale;
        let list_w = list_w - side - m::SPACING;
        // The name takes what the others leave (inside the list's border and
        // scroll bar), the last filling to the edge.
        let columns = [Column::new("Name", (list_w - 2.0 - SCROLL_BAR - 208.0).max(120.0)), Column::new("Size", 76.0).right(), Column::new("", 50.0), Column::new("Files", 76.0).right()];
        let want: Vec<usize> = self.selected.and_then(|s| rows.iter().position(|&r| r == s)).into_iter().collect();
        {
            let mut state = self.list.state_mut();
            if state.selected.iter().copied().collect::<Vec<_>>() != want {
                state.selected = want.into_iter().collect();
            }
        }
        // Each row's bar in its slice's colour, tying the list to the
        // rings.
        let hues: HashMap<NodeId, Hsla> = self.slices.iter().filter(|s| s.ring == 1).map(|s| (s.node, s.color)).collect();
        let bar_other = style::c(mix(p.document_text, p.document_background, 0.5));
        {
            let t = tree.read().unwrap();
            self.shown = rows
                .iter()
                .map(|&r| {
                    let n = &t.nodes[r as usize];
                    Shown { name: n.name.to_string(), size: n.size, dir: n.dir, files: n.files, icon: icon_for(&n.name, n.dir), bar: hues.get(&r).copied().unwrap_or(bar_other) }
                })
                .collect();
            self.total = t.nodes[self.folder as usize].size.max(1);
        }
        let total = self.total;
        let list = ColumnListView::new(cx, "disk-usage-list", &self.list, columns, rows.len(), |d: &DiskUsage, cell, g| {
            let Some(row) = d.shown.get(cell.row) else { return };
            match cell.column {
                0 => {
                    let icon_r = Bounds { origin: point(cell.bounds.left() + px(m::CELL_INSET), cell.bounds.center().y - px(8.0)), size: size(px(16.0), px(16.0)) };
                    icons::paint(g, style::snap(g, icon_r), row.icon);
                    cell.bounds = Bounds::from_corners(point(icon_r.right() + px(2.0), cell.bounds.top()), cell.bounds.bottom_right());
                    cell.text(g, row.name.clone());
                }
                1 => cell.text(g, format_size(row.size)),
                2 => {
                    // The share of the folder, as a bar.
                    let p = theme::palette();
                    let b = cell.bounds;
                    let r = style::snap(g, Bounds { origin: point(b.left() + px(4.0), b.center().y - px(4.0)), size: size(b.size.width - px(8.0), px(8.0)) });
                    style::fill_rect(g, r, style::c(mix(p.document_background, p.document_text, 0.12)));
                    // Whole device pixels long, at least one for anything
                    // at all.
                    let s = g.scale_factor();
                    let w = ((f32::from(r.size.width) * (row.size as f32 / d.total as f32) * s).round().max(if row.size > 0 { 1.0 } else { 0.0 })) / s;
                    style::fill_rect(g, Bounds { origin: r.origin, size: size(px(w), r.size.height) }, row.bar);
                }
                _ => cell.text(g, if row.dir { count(row.files) } else { String::new() }),
            }
        })
        .row_key(|d: &DiskUsage, row| d.shown.get(row).map_or(0, |r| style::key((&r.name, r.size, r.dir, r.files, d.total, [r.bar.h, r.bar.s, r.bar.l].map(f32::to_bits)))))
        .empty_text(if running { "Scanning…" } else { "Empty" })
        .on_select(cx.msg_with(|_: &()| Msg::Selected))
        .on_open(cx.msg_with(|r: &usize| Msg::Open(*r)))
        .on_context(cx.msg_with(|(r, at): &(usize, Point<Pixels>)| Msg::Context(*r, *at)))
        .on_drag(cx.msg_with(|r: &usize| Msg::Drag(*r)));

        // What the pointer is over, or how the scan is going.
        let hovered = self.pointer.at.and_then(|pos| self.hit(pos)).map(|i| self.slices[i].node);
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
        let look = style::key((&line1, &line2));
        let status = olo_kit::canvas(move |g, b| {
            let p = theme::palette();
            let font = style::ui_font(FontWeight::NORMAL);
            let dim = style::c(mix(p.panel_text, p.panel_background, 0.4));
            style::label_fit(g, b.left(), b.top() + px(style::PLAIN_SIZE / 2.0 + 1.0), style::Align::Left, line1.clone(), style::PLAIN_SIZE, font.clone(), style::c(p.panel_text), b.size.width);
            style::label_fit(g, b.left(), b.bottom() - px(style::PLAIN_SIZE / 2.0 + 1.0), style::Align::Left, line2.clone(), style::PLAIN_SIZE, font, dim, b.size.width);
        })
        .paint_key(look)
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

    /// The slice at `pos`, where the chart was last drawn.
    fn hit(&self, pos: Point<Pixels>) -> Option<usize> {
        chart::hit(&self.slices, &CHART.with(|c| c.get())?, pos)
    }

    fn paint_chart(&self, gx: &mut Painter, b: Bounds<Pixels>) {
        let Some(scan) = &self.scan else { return };
        let g = Self::geometry(b);
        CHART.with(|c| c.set(Some(g)));
        let (slices, folder, selected) = (&self.slices, self.folder, self.selected);
        let p = theme::palette();
        let hover = self.pointer.at.and_then(|pos| chart::hit(slices, &g, pos));
        let on_hub = self.pointer.at.is_some_and(|pos| g.at(pos).is_some_and(|(ring, _)| ring == 0));
        gx.with_content_mask(Some(olo_kit::ContentMask { bounds: b }), |gx| {
            chart::paint(gx, &g, slices, hover, selected, style::c(p.panel_background));
            let hub_fill = if on_hub && folder != ROOT { mix(p.document_background, p.list_selected_background, 0.3) } else { p.document_background };
            chart::paint_hub(gx, &g, style::c(hub_fill), style::c(mix(p.panel_text, p.panel_background, 0.5)));
            let t = scan.tree.read().unwrap();
            let n = &t.nodes[folder as usize];
            let title = if folder == ROOT && t.root == home() { "Home".to_owned() } else { n.name.to_string() };
            let w = px(g.hub * 1.7);
            style::label_fit(gx, g.center.x, g.center.y - px(8.0), style::Align::Center, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.document_text), w);
            style::label(gx, g.center.x, g.center.y + px(8.0), style::Align::Center, format_size(n.size), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.document_text));
            let hub = Bounds { origin: point(g.center.x - px(g.hub / 2.0), g.center.y - px(g.hub / 2.0)), size: size(px(g.hub), px(g.hub)) };
            olo_kit::agent::record(
                gx,
                olo_kit::agent::Node { role: "Button", name: Some(format!("{title} {}", format_size(n.size)).into()), bounds: hub, enabled: true, clickable: true, ..Default::default() },
            );
            // Agents (and hints) can pick the first ring's slices by
            // name.
            for s in slices.iter().filter(|s| s.ring == 1 && s.end - s.start > 0.15) {
                let mid = chart::to_pos(&g, 1, (s.start + s.end) / 2.0);
                let at = Bounds { origin: point(mid.x - px(4.0), mid.y - px(4.0)), size: size(px(8.0), px(8.0)) };
                olo_kit::agent::record(
                    gx,
                    olo_kit::agent::Node {
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
    }

    /// The pointer moved over the chart (None: off it): what's under it
    /// is lit, and a press on a slice that moves becomes a drag.
    fn chart_moved(&mut self, e: Option<&MouseMoveEvent>, cx: &mut Cx<Self>) {
        let g = CHART.with(|c| c.get());
        let state = |at: Option<Point<Pixels>>, d: &Self| {
            let hub = at.is_some_and(|pos| g.is_some_and(|g| g.at(pos).is_some_and(|(r, _)| r == 0)));
            (at.and_then(|pos| d.hit(pos)), hub)
        };
        let before = state(self.pointer.at, self);
        self.pointer.at = e.map(|e| e.position);
        if let Some(e) = e
            && let Some((from, Some(node))) = self.pointer.pressed
            && e.pressed_button == Some(MouseButton::Left)
            && f32::from(e.position.x - from.x).hypot(f32::from(e.position.y - from.y)) > 3.0
        {
            self.pointer.pressed = None;
            self.drag = Some(node);
            self.drag_at = e.position;
            cx.invalidate();
        }
        if state(self.pointer.at, self) != before {
            cx.invalidate();
        }
    }

    fn chart_pressed(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let hit = self.hit(e.position).map(|i| self.slices[i].node);
        match e.button {
            MouseButton::Left => self.pointer.pressed = Some((e.position, hit)),
            MouseButton::Right => {
                let Some(id) = hit else { return };
                self.context = Some(id);
                self.selected = Some(id);
                let chose = cx.msg_with(|command: &str| Msg::Command(command.to_owned()));
                menu::context_menu(cx, self.item_menu(id), e.position, chose);
                cx.invalidate();
            }
            _ => {}
        }
    }

    /// A click: into the folder clicked, or up from the middle.
    fn chart_released(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let Some((_, pressed)) = self.pointer.pressed.take() else { return };
        let hit = self.hit(e.position).map(|i| self.slices[i].node);
        let on_hub = CHART.with(|c| c.get()).is_some_and(|g| g.at(e.position).is_some_and(|(r, _)| r == 0));
        match hit.filter(|h| Some(*h) == pressed) {
            Some(id) => {
                self.selected = Some(id);
                self.enter(id, cx);
            }
            None if on_hub => self.up(cx),
            None => {}
        }
        cx.invalidate();
    }

    /// The Collector's chips in its well at `b`.
    fn chips(&self, b: Bounds<Pixels>) -> (Bounds<Pixels>, Vec<Chip>) {
        let items: Vec<(NodeId, String, u64, bool)> = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                self.collected.iter().map(|&i| (i, t.nodes[i as usize].name.to_string(), t.nodes[i as usize].size, t.nodes[i as usize].dir)).collect()
            }
            None => Vec::new(),
        };
        let font = style::ui_font(FontWeight::NORMAL);
        let inner = Bounds::from_corners(point(b.left() + px(6.0), b.top() + px(6.0)), point(b.right() - px(6.0), b.bottom() - px(6.0)));
        let mut x = inner.left();
        let y = inner.bottom() - px(CHIP_HEIGHT);
        let mut chips = Vec::new();
        for (k, (id, name, size, dir)) in items.iter().enumerate() {
            let text = format!("{name}  {}", format_size(*size));
            let w = (f32::from(style::text_width(text.clone(), style::DETAIL_SIZE, font.clone())) + 16.0 + 6.0 + 14.0 + 8.0).min(220.0);
            let left = items.len() - k;
            if x + px(w) > inner.right() - px(if left > 1 { 60.0 } else { 0.0 }) {
                chips.push((None, Bounds { origin: point(x, y), size: size_of(0.0, CHIP_HEIGHT) }, format!("+{left} more"), false, None));
                break;
            }
            let chip = Bounds { origin: point(x, y), size: size_of(w, CHIP_HEIGHT) };
            let close = Bounds { origin: point(chip.right() - px(15.0), chip.center().y - px(6.0)), size: size_of(12.0, 12.0) };
            chips.push((Some(*id), chip, text, *dir, Some(close)));
            x += px(w + 6.0);
        }
        (inner, chips)
    }

    fn paint_well(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let b = style::snap(g, b);
        let (inner, chips) = self.chips(b);
        let heading = if self.collected.is_empty() {
            "Collector: drag files and folders here".to_owned()
        } else {
            format!("Collector: {} — {}", plural(self.collected.len() as u64, "item", "items"), format_size(self.collected_size()))
        };
        let p = theme::palette();
        let target = self.drag.is_some() && b.contains(&self.drag_at);
        let fill = if target { mix(p.document_background, p.list_selected_background, 0.25) } else { p.document_background };
        style::fill_rect(g, style::inset(g, b, 2.0), style::c(fill));
        style::sunken(g, b, &p, target);
        // In line with the chips' icons.
        style::label(g, inner.left() + px(3.0), inner.top() + px(6.0), style::Align::Left, heading, style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.document_text));
        let font = style::ui_font(FontWeight::NORMAL);
        for (id, chip, text, dir, close) in &chips {
            if id.is_none() {
                style::label(g, chip.left() + px(2.0), chip.center().y, style::Align::Left, text.clone(), style::DETAIL_SIZE, font.clone(), style::c(p.document_text));
                continue;
            }
            let chip_r = style::snap(g, *chip);
            g.paint_quad(olo_kit::fill(chip_r, style::c(mix(p.panel_background, p.document_background, 0.4))).corner_radii(px(3.0)));
            g.paint_quad(olo_kit::outline(chip_r, style::c(mix(p.panel_text, p.panel_background, 0.6)), BorderStyle::Solid).corner_radii(px(3.0)));
            let name = text.split("  ").next().unwrap_or_default();
            let icon_r = style::snap(g, Bounds { origin: point(chip.left() + px(3.0), chip.center().y - px(8.0)), size: size_of(16.0, 16.0) });
            icons::paint(g, icon_r, icon_for(name, *dir));
            if let Some(close) = close {
                style::label_fit(
                    g,
                    chip.left() + px(22.0),
                    chip.center().y,
                    style::Align::Left,
                    text.clone(),
                    style::DETAIL_SIZE,
                    font.clone(),
                    style::c(p.document_text),
                    close.left() - chip.left() - px(24.0),
                );
                let over = self.well_at.is_some_and(|at| close.contains(&at));
                let xc = if over { p.panel_text } else { mix(p.panel_text, p.panel_background, 0.4) };
                let (a, z) = (point(close.left() + px(3.0), close.top() + px(3.0)), point(close.right() - px(3.0), close.bottom() - px(3.0)));
                g.paint_segment(a, z, px(1.5), true, style::c(xc));
                g.paint_segment(point(z.x, a.y), point(a.x, z.y), px(1.5), true, style::c(xc));
                olo_kit::agent::record(
                    g,
                    olo_kit::agent::Node { role: "Button", name: Some(format!("Remove {name} from the Collector").into()), bounds: *close, enabled: true, clickable: true, ..Default::default() },
                );
            }
        }
    }

    /// The chip's "×" at `at` in the well, if one is there.
    fn close_at(&self, at: Point<Pixels>) -> Option<NodeId> {
        let (_, chips) = self.chips(self.collector);
        chips.into_iter().find_map(|(id, _, _, _, close)| id.filter(|_| close.is_some_and(|c| c.contains(&at))))
    }

    fn collector_view(&self, cx: &mut Cx<Self>) -> Element {
        let well = cx
            .canvas(|d: &DiskUsage, g, b| d.paint_well(g, b))
            .id("disk-usage-collector")
            .flex_1()
            .h_full()
            // (Where a drop lands, and where its chips are.)
            .on_bounds_changed(cx.listener(|d: &mut Self, b: &Bounds<Pixels>, _| d.collector = *b))
            .on_mouse_move(cx.listener(|d: &mut Self, e: &MouseMoveEvent, cx| {
                let before = d.well_at.and_then(|at| d.close_at(at));
                d.well_at = Some(e.position);
                if d.close_at(e.position) != before {
                    cx.invalidate();
                }
            }))
            .on_hover(cx.listener(|d: &mut Self, over: &bool, cx| {
                if !over && d.well_at.take().is_some() {
                    cx.invalidate();
                }
            }))
            // A chip's "×": out of the Collector.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|d: &mut Self, e: &MouseDownEvent, cx| {
                    if let Some(id) = d.close_at(e.position) {
                        d.collected.retain(|&c| c != id);
                        cx.invalidate();
                    }
                }),
            );
        let busy = self.busy.is_some();
        let any = !self.collected.is_empty();
        let bw = 112.0;
        let mut buttons = div().w(px(bw)).flex().flex_col().gap(px(6.0)).pt(px(2.0));
        if busy {
            buttons = buttons.child(Button::new("Stop").min_width(bw).on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("stop-removing".into()))));
        } else {
            buttons = buttons
                .child(Button::new("Move to Trash").min_width(bw).enabled(any).on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("collector-trash".into()))))
                .child(Button::new("Delete…").min_width(bw).enabled(any).on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("collector-delete".into()))));
        }
        div().flex().gap(px(m::SPACING)).h(px(COLLECTOR_HEIGHT)).flex_shrink_0().child(well).child(buttons)
    }

    /// What's dragged, by the pointer.
    fn paint_drag(&self, g: &mut Painter) {
        let Some((_, dir, name, size)) = self.drag.and_then(|id| self.item(id)) else { return };
        let p = theme::palette();
        let pos = self.drag_at;
        let text = format!("{name}  {}", format_size(size));
        let font = style::ui_font(FontWeight::NORMAL);
        let w = f32::from(style::text_width(text.clone(), style::PLAIN_SIZE, font.clone())) + 30.0;
        let r = style::snap(g, Bounds { origin: point(pos.x + px(12.0), pos.y + px(6.0)), size: size_of(w, CHIP_HEIGHT + 2.0) });
        g.paint_quad(olo_kit::fill(r, style::c(p.list_selected_background).opacity(0.92)).corner_radii(px(3.0)));
        let icon_r = style::snap(g, Bounds { origin: point(r.left() + px(4.0), r.center().y - px(8.0)), size: size_of(16.0, 16.0) });
        icons::paint(g, icon_r, icon_for(&name, dir));
        style::label(g, r.left() + px(24.0), r.center().y, style::Align::Left, text, style::PLAIN_SIZE, font, style::c(p.list_selected_item_text));
    }
}

thread_local! {
    /// The chart's place, last drawn (the window's looper's own: its
    /// drawing and its pointer are both there).
    static CHART: std::cell::Cell<Option<chart::Geometry>> = const { std::cell::Cell::new(None) };
}

/// A file's icon by its kind, as Tracker shows it.
fn icon_for(name: &str, dir: bool) -> &'static str {
    if dir { "folder" } else { kind_for(Path::new(name), false, 0).icon() }
}

fn size_of(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}

impl View for DiskUsage {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Tick => self.restore(),
            Msg::Volumes(v) => self.volumes = Some(v),
            Msg::Removed { gone, trashed, failed } => {
                self.busy = None;
                let purpose = self.removing_as.take().unwrap_or(Purpose::Do);
                if !trashed.is_empty() {
                    self.record(purpose, Step::Trashed(trashed));
                }
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
                self.say_failed("removed", &failed, cx);
                self.read_volumes(cx);
            }
            Msg::PutBack { purpose, back, failed } => {
                self.busy = None;
                let mut ids = Vec::new();
                if let Some(scan) = &self.scan {
                    let mut t = scan.tree.write().unwrap();
                    for (id, to) in &back {
                        // Back in its folder (under a new number if
                        // something new had its name): shown there again.
                        if to.parent() == t.path(*id).parent() {
                            if let Some(name) = to.file_name() {
                                t.nodes[*id as usize].name = name.to_string_lossy().into();
                            }
                            t.put_back(*id);
                            ids.push(*id);
                        }
                    }
                }
                if !ids.is_empty() {
                    self.record(purpose, Step::PutBack(ids));
                }
                self.say_failed("put back", &failed, cx);
                self.read_volumes(cx);
            }
            Msg::Scan(path) => return self.start_scan(path, cx),
            Msg::Delete(items) => self.remove(items, false, cx),
            Msg::Command(command) => return self.do_command(&command, cx),
            Msg::Selected => {
                let row = self.list.state().single();
                self.selected = row.and_then(|r| self.rows.3.get(r).copied());
            }
            Msg::Open(row) => {
                if let Some(&id) = self.rows.3.get(row) {
                    self.open_item(id, cx);
                }
            }
            Msg::Context(row, at) => {
                let Some(&id) = self.rows.3.get(row) else { return };
                self.selected = Some(id);
                self.context = Some(id);
                let chose = cx.msg_with(|command: &str| Msg::Command(command.to_owned()));
                menu::context_menu(cx, self.item_menu(id), at, chose);
            }
            Msg::Drag(row) => {
                if let Some(&id) = self.rows.3.get(row) {
                    self.drag = Some(id);
                    self.drag_at = cx.mouse_position();
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.do_command(command, cx);
        true
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        let saved = match &self.scan {
            Some(scan) => {
                let t = scan.tree.read().unwrap();
                Saved { root: Some(t.root.clone()), folder: Some(t.path(self.folder)), collected: self.collected.iter().map(|&c| t.path(c)).collect() }
            }
            None => Saved::default(),
        };
        serde_json::to_string(&saved).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let bar = MenuBar::new(cx, self.menus());
        let content = if self.scan.is_some() { self.scan_view(cx) } else { self.volumes_view(cx) };
        let mut root = div().size_full().flex().flex_col().child(bar).child(div().flex_1().min_h_0().p(px(m::WINDOW_INSET)).child(content));
        // While something is dragged: its name follows the pointer, and
        // letting go over the Collector collects it.
        if self.drag.is_some() {
            let chip = cx.canvas(|d: &DiskUsage, g, _| d.paint_drag(g)).absolute().left(px(0.0)).top(px(0.0)).size_full();
            root = root
                .relative()
                .child(chip)
                .on_drag_move(cx.listener(|d: &mut Self, e: &MouseMoveEvent, cx| {
                    d.drag_at = e.position;
                    cx.invalidate();
                }))
                .on_window_mouse_up(cx.listener(|d: &mut Self, e: &MouseUpEvent, cx| {
                    if let Some(id) = d.drag.take()
                        && d.collector.contains(&e.position)
                    {
                        d.collect(id);
                    }
                    cx.invalidate();
                }));
        }
        // Keys the list doesn't take: up a folder, collect.
        root.on_key_down(cx.listener(|d: &mut Self, e: &KeyDownEvent, cx| {
            let k = &e.keystroke;
            let up = k.key == "backspace" && !k.modifiers.modified() || olo_kit_base::keys::shortcut("up").is_some_and(|s| olo_kit::keys::pressed(&s, k));
            if up {
                d.do_command("up", cx);
                cx.stop_propagation();
            }
        }))
    }
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let saved: Option<Saved> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
        // What to scan to begin with: where it was (after a reload), or
        // the folder named.
        let start = match saved {
            Some(saved) if saved.root.is_some() => saved.root.clone().map(|root| (root, Some(saved))),
            _ => args.iter().find(|a| !a.starts_with('-')).map(|path| (std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path)), None)),
        };
        let window = olo_kit::open_window(WindowOptions::new("DiskUsage", 780.0, 560.0).min_size(600.0, 440.0).keep_size("main"), move |cx| DiskUsage::new(start, cx));
        // Launched again: the one there comes forward, and scans the
        // folder named.
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(launch) = message.downcast::<Launch>() {
                olo_kit::raise(Some(&window));
                if let Some(path) = launch.args.iter().find(|a| !a.starts_with('-')) {
                    window.messenger().send(Msg::Scan(launch.cwd.clone().unwrap_or_default().join(path)));
                }
            }
        });
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
