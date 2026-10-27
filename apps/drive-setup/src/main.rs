//! DriveSetup: the disks, their partitions and what is on them, after
//! BeOS's DriveSetup and Haiku's (a map of the chosen disk over a list of
//! every disk and volume). Mount and unmount, unlock and lock, format,
//! make, delete, resize and rename partitions, give a disk a new
//! partition table, write a disk image onto it, eject it, check its health
//! and measure how fast it reads. Everything talks to UDisks2
//! (`olo_system::disks`) on worker threads; UDisks asks polkit before
//! privileged steps. Every destructive step is behind an alert naming the
//! disk, and the disk holding the running system is never offered for one.
//!
//! For agents: `--list` prints every disk as JSON; `--mount DEVICE`,
//! `--unmount DEVICE` and `--eject DEVICE` do the safe steps.

mod map;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, Proxy, WindowId, WindowOptions,
    egui::{self, Align, Id, Key, Rect, Shape, Stroke, Ui, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    files::format_size,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, CheckBox, Column, ColumnListView, ListState, MenuField, Slider, StatusBar, TextControl},
};
use olo_system::disks::{self, Disk, Segment, Space, Status, Volume};
use olo_theme::mix;

const APP_ID: &str = "olo.drive-setup";
const MIB: u64 = 1 << 20;
const MAP_H: f32 = 52.0;
const FORM_W: u32 = 440;
const DIVIDER: f32 = 92.0;
const BENCH_SAMPLES: usize = 48;
/// The name column's indent per level, and its icon.
const INDENT: f32 = 14.0;
const ICON: f32 = 16.0;
/// The Free column's bar of how full a volume is.
const USAGE_W: f32 = 44.0;
const USAGE_H: f32 = 10.0;
/// Re-read this often while the window is open: writing files sends no
/// UDisks signal, and the free space should stay current.
const SPACE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Something in the list or on the map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Item {
    Disk(String),
    Volume(String),
    Free { disk: String, offset: u64, size: u64 },
}

struct Row {
    item: Item,
    depth: usize,
    icon: Option<&'static str>,
    cells: [String; 6],
    space: Option<Space>,
}

fn table_name(d: &Disk) -> String {
    match d.table.as_deref() {
        Some("gpt") => "GUID partitions".into(),
        Some("dos") => "MBR partitions".into(),
        Some(t) => format!("{t} partitions"),
        None if d.volumes.is_empty() => "Empty".into(),
        None => "No partitions".into(),
    }
}

/// A device without "/dev/".
fn short(device: &str) -> String {
    device.strip_prefix("/dev/").unwrap_or(device).to_owned()
}

fn disk_icon(d: &Disk) -> &'static str {
    match &d.drive {
        Some(dr) if dr.connection == "usb" => "usb",
        _ => "disk",
    }
}

fn push_volume(out: &mut Vec<Row>, v: &Volume, depth: usize) {
    let icon = if v.encrypted {
        Some("vault")
    } else if v.mounted() {
        Some("folder")
    } else {
        None
    };
    out.push(Row {
        item: Item::Volume(v.path.clone()),
        depth,
        icon,
        cells: [v.title(), short(&v.device), v.content_name(), format_size(v.size), v.space.map(|s| format_size(s.free)).unwrap_or_default(), v.mount_points.join(", ")],
        space: v.space,
    });
    if let Some(inner) = &v.cleartext {
        push_volume(out, inner, depth + 1);
    }
}

/// The list: each disk, then its partitions (logical ones under their
/// container, unlocked contents under what holds them) and free space.
fn rows(status: &Status) -> Vec<Row> {
    let mut out = Vec::new();
    for d in &status.disks {
        out.push(Row { item: Item::Disk(d.path.clone()), depth: 0, icon: Some(disk_icon(d)), cells: [d.name(), short(&d.device), table_name(d), format_size(d.size), String::new(), String::new()], space: None });
        for s in d.segments() {
            match s {
                Segment::Volume(i) => {
                    let v = &d.volumes[i];
                    push_volume(&mut out, v, 1);
                    if v.container {
                        for inner in d.volumes.iter().filter(|w| w.contained) {
                            push_volume(&mut out, inner, 2);
                        }
                    }
                }
                Segment::Free { offset, size } => out.push(Row {
                    item: Item::Free { disk: d.path.clone(), offset, size },
                    depth: 1,
                    icon: None,
                    cells: ["Free space".into(), String::new(), String::new(), format_size(size), String::new(), String::new()],
                    space: None,
                }),
            }
        }
    }
    out
}

/// Under a tenth left: shown as a warning.
fn nearly_full(space: &Space) -> bool {
    space.fraction() > 0.9
}

/// The longest name a file system takes.
fn label_limit(kind: &str) -> usize {
    match kind {
        "vfat" => 11,
        "xfs" => 12,
        "exfat" => 15,
        "ext2" | "ext3" | "ext4" => 16,
        "ntfs" => 128,
        _ => 255,
    }
}

/// `widget` at its own size at the left of `r` (`put` would centre it).
fn left(ui: &mut Ui, r: Rect, widget: impl egui::Widget) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(Align::Center)), |ui| ui.add(widget)).inner
}

/// The name, file system and encryption of a new file system (Format and
/// Create share them).
#[derive(Default)]
struct FsFields {
    name: String,
    kind: usize,
    kinds: Vec<(&'static str, &'static str)>,
    encrypt: bool,
    pass: String,
    again: String,
    erase: bool,
}

impl FsFields {
    fn new(kinds: Vec<(&'static str, &'static str)>, name: String) -> FsFields {
        FsFields { name, kinds, ..Default::default() }
    }

    fn kind(&self) -> &'static str {
        self.kinds.get(self.kind).map_or("ext4", |k| k.0)
    }

    fn format(&self) -> disks::Format {
        disks::Format { kind: self.kind().into(), label: self.name.trim().into(), erase: self.erase, passphrase: self.encrypt.then(|| self.pass.clone()) }
    }

    /// What stops it being made, in words.
    fn problem(&self) -> Option<String> {
        let limit = label_limit(self.kind());
        if self.name.trim().len() > limit {
            return Some(format!("The name is too long for this file system ({limit} letters at most)."));
        }
        if self.encrypt && !self.again.is_empty() && self.pass != self.again {
            return Some("The passphrases don't match.".into());
        }
        None
    }

    fn ready(&self) -> bool {
        self.problem().is_none() && !self.kinds.is_empty() && (!self.encrypt || (!self.pass.is_empty() && self.pass == self.again))
    }

    /// The fields from `y` down; where they end.
    fn ui(&mut self, ui: &mut Ui, col: Rect, mut y: f32, erase: bool) -> f32 {
        let row = |y: f32| Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::CONTROL_HEIGHT));
        ui.put(row(y), TextControl::new(&mut self.name).label("Name:").divider(DIVIDER).autofocus(true));
        y += m::CONTROL_HEIGHT + m::TIGHT;
        let names: Vec<&str> = self.kinds.iter().map(|k| k.1).collect();
        left(ui, row(y), MenuField::new("drive-setup-kind", &mut self.kind, &names).label("File system:").divider(DIVIDER));
        y += m::CONTROL_HEIGHT + m::TIGHT;
        left(ui, row(y), CheckBox::new(&mut self.encrypt, "Encrypt it with a passphrase"));
        y += m::CONTROL_HEIGHT;
        if self.encrypt {
            ui.put(row(y), TextControl::new(&mut self.pass).label("Passphrase:").divider(DIVIDER).password(true));
            y += m::CONTROL_HEIGHT + m::TIGHT;
            ui.put(row(y), TextControl::new(&mut self.again).label("Again:").divider(DIVIDER).password(true));
            y += m::CONTROL_HEIGHT + m::TIGHT;
        }
        if erase {
            left(ui, row(y), CheckBox::new(&mut self.erase, "Overwrite the old data with zeros (slow)"));
            y += m::CONTROL_HEIGHT;
        }
        if let Some(problem) = self.problem() {
            let p = style::palette(ui.ctx());
            style::label(ui.painter(), col.left(), y + m::LABEL_ROW / 2.0, Align::Min, problem, style::plain_font(), c(p.failure));
            y += m::LABEL_ROW;
        }
        y
    }
}

/// What a confirmed alert goes on to do.
enum Action {
    Delete { part: String, release: Option<Volume>, title: String },
    WriteImage { disk: Disk, image: PathBuf },
}

enum Win {
    Main,
    Confirm { message: String, button: &'static str, action: Option<Action> },
    Format { path: String, release: Option<Volume>, title: String, message: String, fields: Box<FsFields> },
    Create { disk: String, table: String, offset: u64, max: u64, size: f32, message: String, fields: Box<FsFields> },
    Resize { path: String, title: String, current: u64, max: u64, size: f32, has_fs: bool, can: Option<Result<(bool, bool), String>> },
    Rename { path: String, title: String, fs: bool, name: String },
    Unlock { path: String, title: String, pass: String },
    Table { disk: Box<Disk>, kind: usize, erase: bool },
    Image { disk: Box<Disk>, panel: Box<FilePanel> },
    Bench(Box<Bench>),
}

struct Bench {
    path: String,
    title: String,
    size: u64,
    samples: Vec<f64>,
    result: Option<disks::Benchmark>,
    error: Option<String>,
    stop: Option<Arc<AtomicBool>>,
}

enum BenchMsg {
    Sample(f64),
    Done(Result<disks::Benchmark, String>),
}

enum Msg {
    Status(Result<Status, String>),
    Changed,
    /// Time to look at the free space again.
    Tick,
    Progress(Option<f32>, String),
    Done { fail: String, result: Result<Option<Item>, String> },
    CanResize(WindowId, Result<(bool, bool), String>),
    Bench(WindowId, BenchMsg),
}

/// The step under way.
struct Working {
    what: String,
    stop: Option<Arc<AtomicBool>>,
    fraction: Option<f32>,
    detail: String,
}

/// What the selection allows.
#[derive(Default)]
struct Can {
    mount: bool,
    unmount: bool,
    unlock: bool,
    lock: bool,
    format: bool,
    create: bool,
    delete: bool,
    resize: bool,
    rename: bool,
    tracker: bool,
    eject: bool,
    table: bool,
    image: bool,
    bench: bool,
    health: bool,
}

/// The selection, found in the status.
struct Sel<'a> {
    disk: &'a Disk,
    volume: Option<&'a Volume>,
    /// What an unlocked volume is unlocked from.
    outer: Option<&'a Volume>,
    free: Option<(u64, u64)>,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    selected: Option<Item>,
}

struct DriveSetup {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    proxy: Proxy,
    status: Status,
    loaded: bool,
    error: Option<String>,
    loading: bool,
    again: bool,
    selected: Option<Item>,
    /// The selection's disk, to fall back to when the selection goes.
    sel_disk: Option<String>,
    /// To select once the list has it (a partition just made).
    pending: Option<Item>,
    reveal: bool,
    list: ListState,
    working: Option<Working>,
}

impl DriveSetup {
    fn open(&mut self, cx: &mut Cx) {
        if let Some(id) = self.main {
            return cx.activate(id);
        }
        let id = cx.open_window(WindowOptions::new("DriveSetup", 800, 480).min_size(480, 340));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.redraw(id);
        }
    }

    /// Read everything again on a worker (once more after, if asked
    /// while reading).
    fn refresh(&mut self) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Status(disks::status().map_err(|e| disks::error_text(&e)))));
    }

    fn select(&mut self, item: Item) {
        self.sel_disk = match &item {
            Item::Disk(d) | Item::Free { disk: d, .. } => Some(d.clone()),
            Item::Volume(v) => self.status.volume(v).map(|(d, _)| d.path.clone()),
        };
        self.selected = Some(item);
        self.reveal = true;
    }

    /// Keep the selection on something that still exists.
    fn settle(&mut self) {
        let rows = rows(&self.status);
        if let Some(p) = &self.pending
            && rows.iter().any(|r| &r.item == p)
        {
            let p = self.pending.take().unwrap();
            return self.select(p);
        }
        if self.selected.as_ref().is_some_and(|s| rows.iter().any(|r| &r.item == s)) {
            if let Some(s) = self.selected.clone() {
                self.select(s);
            }
            return;
        }
        let fallback = self.sel_disk.clone().filter(|d| self.status.disk(d).is_some()).map(Item::Disk).or_else(|| rows.first().map(|r| r.item.clone()));
        self.selected = None;
        if let Some(item) = fallback {
            self.select(item);
        }
    }

    fn sel(&self) -> Option<Sel<'_>> {
        match self.selected.as_ref()? {
            Item::Disk(d) => Some(Sel { disk: self.status.disk(d)?, volume: None, outer: None, free: None }),
            Item::Free { disk, offset, size } => Some(Sel { disk: self.status.disk(disk)?, volume: None, outer: None, free: Some((*offset, *size)) }),
            Item::Volume(path) => {
                let (disk, volume) = self.status.volume(path)?;
                let outer = disk.volumes.iter().find(|v| v.cleartext.as_ref().is_some_and(|c| &c.path == path));
                Some(Sel { disk, volume: Some(volume), outer, free: None })
            }
        }
    }

    fn can(&self) -> Can {
        let Some(s) = self.sel() else { return Can::default() };
        let idle = self.working.is_none();
        let d = s.disk;
        let writable = idle && !d.read_only;
        let drive = d.drive.as_ref();
        let mut can = Can {
            eject: idle && !d.system && drive.is_some_and(|dr| dr.removable || dr.ejectable || dr.can_power_off),
            table: writable && !d.system,
            image: writable && !d.system,
            bench: idle,
            health: idle && drive.is_some_and(|dr| dr.health.is_some()),
            create: writable && s.free.is_some() && d.table.is_some(),
            ..Can::default()
        };
        if let Some(v) = s.volume {
            let system = v.system || s.outer.is_some_and(|o| o.system);
            let part = s.outer.unwrap_or(v);
            can.mount = idle && v.mountable && !v.mounted();
            can.unmount = idle && v.mounted() && !system;
            can.unlock = idle && v.encrypted && v.cleartext.is_none();
            can.lock = idle && !system && ((v.encrypted && v.cleartext.is_some()) || s.outer.is_some());
            can.format = writable && !system;
            can.delete = writable && !system && part.number.is_some();
            can.resize = writable && !system && s.outer.is_none() && v.number.is_some() && !v.encrypted && !v.container;
            can.rename = writable && (v.mountable || (v.number.is_some() && d.table.as_deref() == Some("gpt")));
            can.tracker = v.mounted();
        }
        can
    }

    fn alert(&mut self, cx: &mut Cx, message: String) {
        let id = cx.open_window(WindowOptions::alert("DriveSetup", &message).parent_opt(self.main));
        self.windows.insert(id, Win::Confirm { message, button: "OK", action: None });
    }

    fn confirm(&mut self, cx: &mut Cx, message: String, button: &'static str, action: Action) {
        let id = cx.open_window(WindowOptions::alert("DriveSetup", &message).parent_opt(self.main));
        self.windows.insert(id, Win::Confirm { message, button, action: Some(action) });
    }

    fn form(&mut self, cx: &mut Cx, title: &str, height: u32, win: Win) -> WindowId {
        let id = cx.open_window(WindowOptions::new(title, FORM_W, height).min_size(FORM_W, 120).parent_opt(self.main));
        self.windows.insert(id, win);
        id
    }

    /// Do `work` on a worker as the step under way: it can report
    /// progress, and says what to select after. `fail` begins the alert
    /// if it fails.
    fn run(
        &mut self,
        cx: &mut Cx,
        what: String,
        fail: String,
        stop: Option<Arc<AtomicBool>>,
        work: impl FnOnce(&dyn Fn(Option<f32>, String)) -> Result<Option<Item>, String> + Send + 'static,
    ) {
        if self.working.is_some() {
            return;
        }
        self.working = Some(Working { what, stop, fraction: None, detail: String::new() });
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let report = |f: Option<f32>, detail: String| proxy.send(Msg::Progress(f, detail));
            let result = work(&report);
            proxy.send(Msg::Done { fail, result });
        });
        self.redraw(cx);
    }

    fn command(&mut self, cx: &mut Cx, command: &str) {
        let can = self.can();
        let Some(s) = self.sel() else {
            if command == "quit" {
                cx.quit();
            }
            return;
        };
        let disk = s.disk.clone();
        let volume = s.volume.cloned();
        let outer = s.outer.cloned();
        let free = s.free;
        let dname = disk.name();
        let e = |r: olo_system::Result<()>| r.map(|_| None).map_err(|e| disks::error_text(&e));
        match command {
            "mount" if can.mount => {
                let v = volume.unwrap();
                let t = v.title();
                self.run(cx, format!("Mounting “{t}”…"), format!("“{t}” couldn't be mounted."), None, move |_| {
                    disks::mount(&v.path).map(|_| None).map_err(|e| disks::error_text(&e))
                });
            }
            "unmount" if can.unmount => {
                let v = volume.unwrap();
                let t = v.title();
                self.run(cx, format!("Unmounting “{t}”…"), format!("“{t}” couldn't be unmounted."), None, move |_| e(disks::unmount(&v.path)));
            }
            "unlock" if can.unlock => {
                let v = volume.unwrap();
                let title = v.title();
                let h = 150;
                self.form(cx, "Unlock", h, Win::Unlock { path: v.path, title, pass: String::new() });
            }
            "lock" if can.lock => {
                let v = volume.unwrap();
                // The encrypted volume itself, or what this is unlocked from.
                let (o, t) = match outer {
                    Some(o) => (o.clone(), o.title()),
                    None => (v.clone(), v.title()),
                };
                self.run(cx, format!("Locking “{t}”…"), format!("“{t}” couldn't be locked."), None, move |_| e(disks::release(&o)));
            }
            "format" if can.format => {
                let v = volume.unwrap();
                let title = v.title();
                let where_ = if outer.is_some() { format!("inside the encrypted volume on “{dname}”") } else { format!("on “{dname}”") };
                let message = format!("Formatting erases everything on “{title}” ({}, {}) {where_}.", v.device, format_size(v.size));
                let fields = FsFields::new(disks::formats(&self.status.filesystems), if v.label.is_empty() { String::new() } else { v.label.clone() });
                let release = Some(v.clone()).filter(Volume::in_use);
                self.form(cx, "Format", 260, Win::Format { path: v.path, release, title, message, fields: Box::new(fields) });
            }
            "create" if can.create => {
                let (offset, size) = free.unwrap();
                let max = size / MIB * MIB;
                let message = format!("Make a partition in the {} of free space on “{dname}”.", format_size(size));
                let fields = FsFields::new(disks::formats(&self.status.filesystems), String::new());
                let table = disk.table.clone().unwrap_or_default();
                self.form(cx, "Create Partition", 300, Win::Create { disk: disk.path, table, offset, max, size: (max / MIB) as f32, message, fields: Box::new(fields) });
            }
            "delete" if can.delete => {
                let v = volume.unwrap();
                let part = outer.unwrap_or(v);
                let title = part.title();
                let message = format!(
                    "Delete “{title}” ({}, {}) from “{dname}”? Everything on it will be lost.",
                    part.device,
                    format_size(part.size)
                );
                let release = Some(part.clone()).filter(Volume::in_use);
                self.confirm(cx, message, "Delete", Action::Delete { part: part.path, release, title });
            }
            "resize" if can.resize => {
                let v = volume.unwrap();
                // Up to the free space right after it.
                let end = v.offset + v.size;
                let after = disk.segments().into_iter().find_map(|s| match s {
                    Segment::Free { offset, size } if offset >= end && offset < end + MIB => Some(offset + size - end),
                    _ => None,
                });
                let max = (v.size + after.unwrap_or(0)) / MIB * MIB;
                let has_fs = !v.content.is_empty();
                let id = self.form(
                    cx,
                    "Resize",
                    210,
                    Win::Resize { path: v.path.clone(), title: v.title(), current: v.size, max, size: (v.size / MIB) as f32, has_fs, can: (!has_fs).then_some(Ok((true, true))) },
                );
                if has_fs {
                    let (proxy, kind) = (self.proxy.clone(), v.content.clone());
                    std::thread::spawn(move || proxy.send(Msg::CanResize(id, disks::can_resize(&kind))));
                }
            }
            "rename" if can.rename => {
                let v = volume.unwrap();
                let name = if v.mountable { v.label.clone() } else { v.name.clone() };
                self.form(cx, "Rename", 150, Win::Rename { path: v.path.clone(), title: v.title(), fs: v.mountable, name });
            }
            "show-in-tracker" if can.tracker => {
                let dir = volume.unwrap().mount_points[0].clone();
                if let Some(tracker) = olo_kit::find_program("olo-tracker")
                    && let Err(e) = olo_kit::open::spawn(&[tracker.into_os_string(), dir.into()])
                {
                    tracing::warn!("couldn't open Tracker: {e}");
                }
            }
            "eject" if can.eject => {
                self.run(cx, format!("Ejecting “{dname}”…"), format!("“{dname}” couldn't be ejected."), None, move |_| e(disks::eject(&disk)));
            }
            "new-table" if can.table => {
                self.form(cx, "New Partition Table", 200, Win::Table { disk: Box::new(disk), kind: 0, erase: false });
            }
            "write-image" if can.image => {
                let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
                let panel = FilePanel::open(home).types(&["iso", "img", "raw", "bin"]);
                let id = cx.open_window(WindowOptions::new("Write Disk Image", PANEL_SIZE.0, PANEL_SIZE.1).min_size(420, 300).parent_opt(self.main));
                self.windows.insert(id, Win::Image { disk: Box::new(disk), panel: Box::new(panel) });
            }
            "benchmark" if can.bench => {
                let bench = Bench { path: disk.path.clone(), title: dname.clone(), size: disk.size, samples: Vec::new(), result: None, error: None, stop: None };
                let id = cx.open_window(WindowOptions::new(format!("Benchmark: {dname}"), 480, 320).min_size(400, 260));
                self.windows.insert(id, Win::Bench(Box::new(bench)));
            }
            "check-health" if can.health => {
                let dr = disk.drive.clone().unwrap();
                let nvme = disk.device.starts_with("/dev/nvme");
                self.run(cx, format!("Checking “{dname}”…"), format!("“{dname}” couldn't be checked."), None, move |_| e(disks::smart_update(&dr.path, nvme)));
            }
            "quit" => cx.quit(),
            _ => {}
        }
    }

    /// The context menu of the selection.
    fn item_menu(&self) -> Menu {
        let can = self.can();
        match &self.selected {
            Some(Item::Volume(_)) => Menu::new("")
                .item("Mount", "mount", None)
                .enabled(can.mount)
                .item("Unmount", "unmount", None)
                .enabled(can.unmount)
                .item("Show in Tracker", "show-in-tracker", None)
                .enabled(can.tracker)
                .separator()
                .item("Unlock…", "unlock", None)
                .enabled(can.unlock)
                .item("Lock", "lock", None)
                .enabled(can.lock)
                .separator()
                .item("Format…", "format", None)
                .enabled(can.format)
                .item("Rename…", "rename", None)
                .enabled(can.rename)
                .item("Resize…", "resize", None)
                .enabled(can.resize)
                .item("Delete…", "delete", None)
                .enabled(can.delete),
            Some(Item::Free { .. }) => Menu::new("").item("Create Partition…", "create", None).enabled(can.create),
            _ => self.disk_menu(Menu::new(""), &can),
        }
    }

    fn disk_menu(&self, menu: Menu, can: &Can) -> Menu {
        menu.item("Eject", "eject", Some(Key::E))
            .enabled(can.eject)
            .separator()
            .item("Check Health", "check-health", None)
            .enabled(can.health)
            .item("Benchmark…", "benchmark", None)
            .enabled(can.bench)
            .separator()
            .item("Write Disk Image…", "write-image", None)
            .enabled(can.image)
            .item("New Partition Table…", "new-table", None)
            .enabled(can.table)
    }

    /// The status line: the step under way (with Stop), UDisks's own
    /// jobs, or what is selected.
    fn status_ui(&mut self, ui: &mut Ui, row: Rect) {
        let p = style::palette(ui.ctx());
        let job = self.status.jobs.first();
        let busy = self.working.as_ref().map(|w| (w.what.clone(), w.fraction, w.detail.clone(), w.stop.is_some()));
        let busy = busy.or_else(|| job.map(|j| (format!("{}…", j.title()), j.progress.map(|f| f as f32), String::new(), false)));
        if let Some((what, fraction, detail, stoppable)) = busy {
            let job_fraction = job.and_then(|j| j.progress).map(|f| f as f32);
            let fraction = fraction.or(job_fraction);
            let cancel = job.filter(|j| j.cancelable).map(|j| j.path.clone());
            let bw = 80.0;
            let can_stop = stoppable || cancel.is_some();
            let bar_r = Rect::from_min_max(row.min, pos2(row.right() - if can_stop { bw + m::SPACING } else { 0.0 }, row.bottom()));
            let trailing = match (fraction, detail.is_empty()) {
                (_, false) => detail,
                (Some(f), true) => format!("{:.0}%", f * 100.0),
                (None, true) => String::new(),
            };
            let t = ui.input(|i| i.time);
            // Without a measure, a bar that sweeps.
            let shown = fraction.unwrap_or(((t * 0.6).fract()) as f32);
            ui.put(bar_r, StatusBar::new(shown).label(&what).trailing(&trailing));
            if can_stop && ui.put(Rect::from_min_max(pos2(row.right() - bw, row.top()), row.max), Button::new("Stop")).clicked() {
                if let Some(stop) = self.working.as_ref().and_then(|w| w.stop.clone()) {
                    stop.store(true, Ordering::Relaxed);
                } else if let Some(job) = cancel {
                    std::thread::spawn(move || {
                        if let Err(e) = disks::cancel(&job) {
                            tracing::warn!("couldn't stop the job: {e}");
                        }
                    });
                }
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let (text, warn) = self.describe();
        let color = if warn { c(p.failure) } else { c(p.panel_text) };
        let clip = ui.painter().with_clip_rect(row);
        style::label(&clip, row.left(), row.center().y, Align::Min, text, style::plain_font(), color);
    }

    /// The selection in words, and whether it is a warning.
    fn describe(&self) -> (String, bool) {
        if let Some(e) = &self.error {
            return (format!("Can't see the disks: {e}"), true);
        }
        let Some(s) = self.sel() else { return (String::new(), false) };
        if let Some((_, size)) = s.free {
            return (format!("{} free. Choose Create Partition… to use it.", format_size(size)), false);
        }
        if let Some(v) = s.volume {
            let mut t = format!("{}, {}", v.content_name(), format_size(v.size));
            if v.system || s.outer.is_some_and(|o| o.system) {
                t.push_str(", holds the running system");
            } else if v.mounted() {
                t.push_str(&format!(", mounted at {}", v.mount_points.join(", ")));
            } else if v.encrypted && v.cleartext.is_none() {
                t.push_str(", locked");
            } else if v.mountable {
                t.push_str(", not mounted");
            }
            let space = v.space.or_else(|| v.cleartext.as_ref().and_then(|c| c.space));
            if let Some(sp) = space {
                t.push_str(&format!("; {} free of {}", format_size(sp.free), format_size(sp.used + sp.free)));
                if nearly_full(&sp) {
                    t.push_str(", nearly full");
                    return (t, true);
                }
            }
            return (t, false);
        }
        let d = s.disk;
        let mut t = format!("{}, {}", d.kind(), format_size(d.size));
        let mut warn = false;
        if let Some(h) = d.drive.as_ref().and_then(|dr| dr.health.as_ref()) {
            if h.ok() {
                t.push_str(", healthy");
            } else {
                warn = true;
                t.push_str(", failing! Copy what matters off it");
            }
            if let Some(temp) = h.temperature {
                t.push_str(&format!(", {temp:.0} °C"));
            }
            if let Some(hours) = h.power_on_hours {
                t.push_str(&format!(", {hours} hours of use"));
            }
        }
        (t, warn)
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        if !self.loaded {
            if let Some(e) = self.error.clone() {
                style::label(ui.painter(), full.center().x, full.center().y, Align::Center, format!("Can't see the disks: {e}"), style::plain_font(), c(p.failure));
            }
            return;
        }
        // The chosen disk's map, under its name.
        let head = Rect::from_min_size(full.min, vec2(full.width(), m::LABEL_ROW));
        let map_r = Rect::from_min_size(pos2(full.left(), head.bottom() + m::TIGHT), vec2(full.width(), MAP_H));
        let status_r = Rect::from_min_max(pos2(full.left(), full.bottom() - m::CONTROL_HEIGHT), full.max);
        let list_r = Rect::from_min_max(pos2(full.left(), map_r.bottom() + m::SPACING), pos2(full.right(), status_r.top() - m::SPACING));
        let mut menu_at = None;
        if let Some(disk) = self.sel().map(|s| s.disk.clone()) {
            let what = format!("{} ({}), {}, {}", disk.name(), disk.device, format_size(disk.size), table_name(&disk));
            style::label(&ui.painter().with_clip_rect(head), head.left(), head.center().y, Align::Min, what, style::bold_font(), c(p.panel_text));
            let out = map::show(ui, map_r, &disk, self.selected.as_ref());
            if let Some(item) = out.clicked {
                self.select(item);
            }
            if let Some((item, pos)) = out.context {
                self.select(item);
                menu_at = Some(pos);
            }
        }

        let rows = rows(&self.status);
        let columns = [
            Column::new("Name", 200.0),
            Column::new("Device", 84.0),
            Column::new("Contents", 120.0),
            Column::new("Size", 72.0).right(),
            Column::new("Free", 130.0),
            Column::new("Mounted At", 140.0),
        ];
        let at = self.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.item == s));
        self.list.selected = at.into_iter().collect();
        let reveal = std::mem::take(&mut self.reveal).then_some(at).flatten();
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new("drive-setup-list", &columns, rows.len(), &mut self.list).reveal(reveal).empty_text("No disks.").show(ui, |cell: &Cell| {
                    let row = &rows[cell.row];
                    if cell.column == 4
                        && let Some(space) = row.space
                    {
                        let left = cell.rect.left() + m::CELL_INSET;
                        let bar = Rect::from_min_size(pos2(left, cell.rect.center().y - USAGE_H / 2.0), vec2(USAGE_W, USAGE_H));
                        let bar = style::snap(cell.painter.ctx(), bar);
                        let p = style::palette(cell.painter.ctx());
                        // On panel grey, so its whole bevel shows on the list's white.
                        cell.painter.rect_filled(bar.expand(1.0), 0.0, c(p.panel_background));
                        widgets::level_bar(cell.painter, bar, space.fraction(), nearly_full(&space).then(|| c(p.failure)));
                        return Cell { rect: cell.rect.with_min_x(bar.right()), ..*cell }.text(&row.cells[4]);
                    }
                    if cell.column != 0 {
                        return cell.text(&row.cells[cell.column]);
                    }
                    let x = cell.rect.left() + m::CELL_INSET + row.depth as f32 * INDENT;
                    if let Some(icon) = row.icon {
                        let r = Rect::from_center_size(pos2(x + ICON / 2.0, cell.rect.center().y), vec2(ICON, ICON));
                        olo_kit::icons::paint(cell.painter, r, icon);
                    }
                    let shifted = Cell { rect: cell.rect.with_min_x(x + ICON), ..*cell };
                    let free = matches!(row.item, Item::Free { .. });
                    let p = style::palette(cell.painter.ctx());
                    let color = if free && !cell.selected { c(mix(p.list_item_text, p.list_background, 0.45)) } else { cell.text_color };
                    let font = if row.depth == 0 { style::bold_font() } else { style::plain_font() };
                    shifted.text_with(&row.cells[0], font, color);
                })
            })
            .inner;
        if out.selection_changed
            && let Some(r) = self.list.single().and_then(|i| rows.get(i))
        {
            self.select(r.item.clone());
        }
        if let Some((i, pos)) = out.context
            && let Some(r) = rows.get(i)
        {
            self.select(r.item.clone());
            menu_at = Some(pos);
        }
        if let Some(i) = out.activated
            && let Some(r) = rows.get(i)
        {
            self.select(r.item.clone());
            let can = self.can();
            let cmd = if can.tracker {
                "show-in-tracker"
            } else if can.mount {
                "mount"
            } else if can.unlock {
                "unlock"
            } else if can.create {
                "create"
            } else {
                ""
            };
            self.command(cx, cmd);
        }
        if let Some(pos) = menu_at {
            widgets::popup_menu_at(ui.ctx(), Id::unique("drive-setup-menu"), pos, self.item_menu());
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("drive-setup-menu")) {
            self.command(cx, &cmd);
        }
        self.status_ui(ui, status_r);
    }

    /// A form's window: grows or shrinks to what it shows (fields come
    /// and go).
    fn fit(cx: &mut Cx, window: WindowId, full: Rect, bottom: f32) {
        let want = (bottom - full.top() + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil();
        if (want - full.height()).abs() > 1.0 {
            cx.resize(window, (FORM_W, want as u32));
        }
    }

    fn close(&mut self, cx: &mut Cx, window: WindowId) -> Option<Win> {
        cx.close_window(window);
        self.windows.remove(&window)
    }

    fn form_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        let full = ui.max_rect();
        let Some(win) = self.windows.get_mut(&window) else { return };
        // The button chosen, and where the fields end.
        let (choice, bottom) = match win {
            Win::Format { message, fields, .. } => {
                let col = widgets::alert_frame(ui, "warning");
                let y = widgets::alert_text(ui, col, col.top(), message) + m::SPACING;
                let bottom = fields.ui(ui, col, y, true);
                (widgets::alert_buttons(ui, col, &["Cancel", "Format"], fields.ready()), bottom)
            }
            Win::Create { message, fields, max, size, .. } => {
                let col = widgets::alert_frame(ui, "drive-setup");
                let mut y = widgets::alert_text(ui, col, col.top(), message) + m::SPACING;
                let max_mib = (*max / MIB).max(1) as f32;
                let r = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::LABEL_ROW * 2.0 + 24.0));
                let text = format_size((*size as u64) * MIB);
                ui.put(r, Slider::new(size, 1.0..=max_mib).step(1.0).label("Size:").value_text(text).limits("1 MB", &format_size(*max)));
                y = r.bottom() + m::SPACING;
                let bottom = fields.ui(ui, col, y, false);
                (widgets::alert_buttons(ui, col, &["Cancel", "Create"], fields.ready() && *size >= 1.0), bottom)
            }
            Win::Resize { title, current, max, size, can, .. } => {
                let col = widgets::alert_frame(ui, "drive-setup");
                let mut y = widgets::alert_text(ui, col, col.top(), &format!("Resize “{title}”, now {}.", format_size(*current))) + m::SPACING;
                let (shrink, grow) = match can {
                    Some(Ok(sg)) => *sg,
                    _ => (false, false),
                };
                let cur = (*current / MIB) as f32;
                let lo = if shrink { 1.0 } else { cur };
                let hi = if grow { (*max / MIB) as f32 } else { cur };
                match can {
                    None => {
                        y = widgets::alert_text(ui, col, y, "Finding out what this file system allows…");
                    }
                    Some(Err(e)) => {
                        let p = style::palette(ui.ctx());
                        style::label(ui.painter(), col.left(), y + m::LABEL_ROW / 2.0, Align::Min, e.clone(), style::plain_font(), c(p.failure));
                        y += m::LABEL_ROW;
                    }
                    Some(Ok(_)) if hi <= lo => {
                        y = widgets::alert_text(ui, col, y, "There is no room to grow it, and its file system can't shrink.");
                    }
                    Some(Ok(_)) => {
                        let r = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::LABEL_ROW * 2.0 + 24.0));
                        let text = format_size((*size as u64) * MIB);
                        ui.put(r, Slider::new(size, lo..=hi).step(1.0).label("Size:").value_text(text).limits(&format_size((lo as u64) * MIB), &format_size((hi as u64) * MIB)));
                        y = r.bottom();
                    }
                }
                let changed = (*size as u64) != cur as u64;
                (widgets::alert_buttons(ui, col, &["Cancel", "Resize"], matches!(can, Some(Ok(_))) && changed), y)
            }
            Win::Rename { title, name, .. } => {
                let col = widgets::alert_frame(ui, "drive-setup");
                let y = widgets::alert_text(ui, col, col.top(), &format!("A new name for “{title}”:")) + m::SPACING;
                let r = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::CONTROL_HEIGHT));
                ui.put(r, TextControl::new(name).autofocus(true));
                (widgets::alert_buttons(ui, col, &["Cancel", "Rename"], true), r.bottom())
            }
            Win::Unlock { title, pass, .. } => {
                let col = widgets::alert_frame(ui, "vault");
                let y = widgets::alert_text(ui, col, col.top(), &format!("“{title}” is encrypted. Its passphrase:")) + m::SPACING;
                let r = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::CONTROL_HEIGHT));
                ui.put(r, TextControl::new(pass).password(true).autofocus(true));
                (widgets::alert_buttons(ui, col, &["Cancel", "Unlock"], !pass.is_empty()), r.bottom())
            }
            Win::Table { disk, kind, erase } => {
                let col = widgets::alert_frame(ui, "warning");
                let message = format!(
                    "A new partition table erases everything on “{}” ({}, {}): every partition on it goes.",
                    disk.name(),
                    disk.device,
                    format_size(disk.size)
                );
                let mut y = widgets::alert_text(ui, col, col.top(), &message) + m::SPACING;
                let row = |y: f32| Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), m::CONTROL_HEIGHT));
                left(ui, row(y), MenuField::new("drive-setup-table", kind, &["GUID (GPT, for anything recent)", "MBR (for older devices)"]).label("Type:").divider(DIVIDER));
                y += m::CONTROL_HEIGHT + m::TIGHT;
                left(ui, row(y), CheckBox::new(erase, "Overwrite the old data with zeros (slow)"));
                y += m::CONTROL_HEIGHT;
                (widgets::alert_buttons(ui, col, &["Cancel", "Erase"], true), y)
            }
            _ => return,
        };
        Self::fit(cx, window, full, bottom);
        match choice {
            None => {}
            Some(0) => {
                self.close(cx, window);
            }
            Some(_) => {
                let Some(win) = self.close(cx, window) else { return };
                self.submit(cx, win);
            }
        }
    }

    /// A form's default button.
    fn submit(&mut self, cx: &mut Cx, win: Win) {
        let e = |r: olo_system::Result<()>| r.map(|_| None).map_err(|e| disks::error_text(&e));
        match win {
            Win::Format { path, release, title, fields, .. } => {
                let f = fields.format();
                self.run(cx, format!("Formatting “{title}”…"), format!("“{title}” couldn't be formatted."), None, move |_| {
                    if let Some(v) = &release {
                        disks::release(v).map_err(|e| disks::error_text(&e))?;
                    }
                    e(disks::format(&path, &f))
                });
            }
            Win::Create { disk, table, offset, size, fields, .. } => {
                let f = fields.format();
                let name = fields.name.trim().to_owned();
                self.run(cx, "Creating a partition…".into(), "The partition couldn't be made.".into(), None, move |_| {
                    disks::create_partition(&disk, &table, offset, (size as u64) * MIB, &name, &f).map(|p| Some(Item::Volume(p))).map_err(|e| disks::error_text(&e))
                });
            }
            Win::Resize { path, title, current, size, has_fs, .. } => {
                self.run(cx, format!("Resizing “{title}”…"), format!("“{title}” couldn't be resized."), None, move |_| {
                    e(disks::resize(&path, current, (size as u64) * MIB, has_fs))
                });
            }
            Win::Rename { path, title, fs, name } => {
                let name = name.trim().to_owned();
                self.run(cx, format!("Renaming “{title}”…"), format!("“{title}” couldn't be renamed."), None, move |_| {
                    e(if fs { disks::set_label(&path, &name) } else { disks::set_partition_name(&path, &name) })
                });
            }
            Win::Unlock { path, title, pass } => {
                self.run(cx, format!("Unlocking “{title}”…"), format!("“{title}” couldn't be unlocked."), None, move |_| {
                    disks::unlock(&path, &pass).map(|p| Some(Item::Volume(p))).map_err(|e| disks::error_text(&e))
                });
            }
            Win::Table { disk, kind, erase } => {
                let name = disk.name();
                self.run(cx, format!("Erasing “{name}”…"), format!("“{name}” couldn't be erased."), None, move |_| {
                    for v in &disk.volumes {
                        disks::release(v).map_err(|e| disks::error_text(&e))?;
                    }
                    disks::new_table(&disk.path, if kind == 0 { "gpt" } else { "dos" }, erase).map_err(|e| disks::error_text(&e))?;
                    Ok(Some(Item::Disk(disk.path.clone())))
                });
            }
            _ => {}
        }
    }

    fn act(&mut self, cx: &mut Cx, action: Action) {
        match action {
            Action::Delete { part, release, title } => {
                self.run(cx, format!("Deleting “{title}”…"), format!("“{title}” couldn't be deleted."), None, move |_| {
                    if let Some(v) = &release {
                        disks::release(v).map_err(|e| disks::error_text(&e))?;
                    }
                    disks::delete_partition(&part).map(|_| None).map_err(|e| disks::error_text(&e))
                });
            }
            Action::WriteImage { disk, image } => {
                let name = disk.name();
                let stop = Arc::new(AtomicBool::new(false));
                let flag = stop.clone();
                self.run(cx, format!("Writing to “{name}”…"), format!("The image couldn't be written to “{name}”."), Some(stop), move |report| {
                    for v in &disk.volumes {
                        disks::release(v).map_err(|e| disks::error_text(&e))?;
                    }
                    disks::write_image(&disk.path, &image, &flag, &mut |p| {
                        let f = p.done as f32 / p.total.max(1) as f32;
                        report(Some(f), format!("{} of {}, {}/s", format_size(p.done), format_size(p.total), format_size(p.rate as u64)));
                    })?;
                    Ok(Some(Item::Disk(disk.path.clone())))
                });
            }
        }
    }

    fn image_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        let Some(Win::Image { panel, .. }) = self.windows.get_mut(&window) else { return };
        let Some(result) = panel.ui(ui) else { return };
        let Some(Win::Image { disk, .. }) = self.close(cx, window) else { return };
        let PanelResult::Chosen(image) = result else { return };
        let name = disk.name();
        let file = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let len = std::fs::metadata(&image).map(|m| m.len()).unwrap_or(0);
        if len > disk.size {
            return self.alert(cx, format!("“{file}” ({}) is bigger than “{name}” ({}).", format_size(len), format_size(disk.size)));
        }
        let message = format!("Write “{file}” ({}) to “{name}” ({}, {})? Everything on the disk will be erased.", format_size(len), disk.device, format_size(disk.size));
        self.confirm(cx, message, "Write", Action::WriteImage { disk: *disk, image });
    }

    fn bench_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        let p = style::palette(ui.ctx());
        let proxy = self.proxy.clone();
        let Some(Win::Bench(b)) = self.windows.get_mut(&window) else { return };
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let mut y = widgets::alert_text(ui, Rect::from_min_max(full.min, pos2(full.right(), full.top() + 40.0)), full.top(), &format!(
            "How fast “{}” reads, from its start to its end, and how long it takes to find a block. Nothing is written.",
            b.title
        )) + m::SPACING;
        let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - m::CONTROL_HEIGHT), full.max);
        let result_h = m::LABEL_ROW;
        let graph = Rect::from_min_max(pos2(full.left(), y), pos2(full.right(), bottom.top() - m::SPACING - result_h - m::TIGHT));
        y = graph.bottom() + m::TIGHT;

        // The rate along the disk.
        let inner = graph.shrink(2.0);
        ui.painter().rect_filled(inner, 0.0, c(p.document_background));
        style::sunken(ui.painter(), graph, &p, false);
        let grid = c(mix(p.document_text, p.document_background, 0.9));
        for k in 1..4 {
            let gy = (inner.top() + inner.height() * k as f32 / 4.0).round() + 0.5;
            style::hline(ui.painter(), inner.left(), inner.right(), gy, grid);
        }
        let top = nice_ceiling(b.samples.iter().copied().fold(0.0, f64::max));
        let small = style::small_font();
        let faint = c(mix(p.document_text, p.document_background, 0.4));
        if !b.samples.is_empty() {
            style::label(ui.painter(), inner.left() + 4.0, inner.top() + 8.0, Align::Min, rate_text(top), small, faint);
        }
        if b.samples.len() > 1 {
            let step = inner.width() / (BENCH_SAMPLES - 1) as f32;
            let pts: Vec<egui::Pos2> = b
                .samples
                .iter()
                .enumerate()
                .map(|(i, &v)| pos2(inner.left() + i as f32 * step, inner.bottom() - 2.0 - (v / top) as f32 * (inner.height() - 4.0)))
                .collect();
            ui.painter().with_clip_rect(inner).add(Shape::line(pts, Stroke::new(1.5, c(p.keyboard_navigation))));
        }

        let text = match (&b.result, &b.error) {
            (_, Some(e)) => e.clone(),
            (Some(r), _) => format!(
                "Average {}, slowest {}, fastest {}; a block found in {}.",
                rate_text(r.average),
                rate_text(r.min),
                rate_text(r.max),
                if r.access_ms < 1.0 { format!("{:.0} µs", r.access_ms * 1000.0) } else { format!("{:.1} ms", r.access_ms) }
            ),
            (None, _) if b.stop.is_some() => format!("Reading… {} of {BENCH_SAMPLES}", b.samples.len()),
            (None, _) => "Choose Start to measure.".into(),
        };
        let color = if b.error.is_some() { c(p.failure) } else { c(p.panel_text) };
        style::label(&ui.painter().with_clip_rect(full), full.left(), y + result_h / 2.0, Align::Min, text, style::plain_font(), color);

        let running = b.stop.is_some();
        let bw = 80.0;
        let r = Rect::from_min_max(pos2(bottom.right() - bw, bottom.top()), bottom.max);
        if ui.put(r, Button::new(if running { "Stop" } else { "Start" }).default(!running)).clicked() {
            if let Some(stop) = &b.stop {
                stop.store(true, Ordering::Relaxed);
            } else {
                let stop = Arc::new(AtomicBool::new(false));
                b.stop = Some(stop.clone());
                b.samples.clear();
                b.result = None;
                b.error = None;
                let (path, size) = (b.path.clone(), b.size);
                std::thread::spawn(move || {
                    let r = disks::benchmark(&path, size, BENCH_SAMPLES, &stop, &mut |_, rate| proxy.send(Msg::Bench(window, BenchMsg::Sample(rate))));
                    proxy.send(Msg::Bench(window, BenchMsg::Done(r)));
                });
            }
        }
        let _ = cx;
    }
}

/// MB/s in words (GB/s when fast).
fn rate_text(mb: f64) -> String {
    if mb >= 1000.0 { format!("{:.1} GB/s", mb / 1000.0) } else if mb >= 100.0 { format!("{mb:.0} MB/s") } else { format!("{mb:.1} MB/s") }
}

/// The next "nice" number at or above `v` (1, 2, 5 × 10ⁿ).
fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * p).find(|&n| n >= v).unwrap_or(10.0 * p)
}


impl App for DriveSetup {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        match self.windows.get(&window) {
            Some(Win::Main) => self.main_ui(cx, ui),
            Some(Win::Confirm { message, button, .. }) => {
                let (message, button) = (message.clone(), *button);
                let (icon, buttons): (&str, Vec<&str>) = if button == "OK" { ("stop", vec!["OK"]) } else { ("warning", vec!["Cancel", button]) };
                let Some(choice) = widgets::alert(ui, icon, &message, &buttons) else { return };
                if let Some(Win::Confirm { action: Some(action), .. }) = self.close(cx, window)
                    && choice == 1
                {
                    self.act(cx, action);
                }
            }
            Some(Win::Image { .. }) => self.image_ui(cx, window, ui),
            Some(Win::Bench(_)) => self.bench_ui(cx, window, ui),
            Some(_) => self.form_ui(cx, window, ui),
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if !matches!(self.windows.get(&window), Some(Win::Main)) {
            return None;
        }
        let can = self.can();
        Some(vec![
            self.disk_menu(Menu::new("Disk"), &can).separator().item("Quit", "quit", Some(Key::Q)),
            Menu::new("Partition")
                .item("Mount", "mount", None)
                .enabled(can.mount)
                .item("Unmount", "unmount", None)
                .enabled(can.unmount)
                .item("Show in Tracker", "show-in-tracker", None)
                .enabled(can.tracker)
                .separator()
                .item("Unlock…", "unlock", None)
                .enabled(can.unlock)
                .item("Lock", "lock", None)
                .enabled(can.lock)
                .separator()
                .item("Create…", "create", None)
                .enabled(can.create)
                .item("Format…", "format", None)
                .enabled(can.format)
                .item("Rename…", "rename", None)
                .enabled(can.rename)
                .item("Resize…", "resize", None)
                .enabled(can.resize)
                .item("Delete…", "delete", None)
                .enabled(can.delete),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        DriveSetup::command(self, cx, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if matches!(self.windows.get(&window), Some(Win::Main)) {
            return cx.quit();
        }
        if let Some(Win::Bench(b)) = self.close(cx, window)
            && let Some(stop) = b.stop
        {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Launch>() {
            return self.open(cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Status(r) => {
                self.loading = false;
                match r {
                    Ok(s) => {
                        self.status = s;
                        self.loaded = true;
                        self.error = None;
                        self.settle();
                    }
                    Err(e) => self.error = Some(e),
                }
                if std::mem::take(&mut self.again) {
                    self.refresh();
                }
            }
            Msg::Changed => self.refresh(),
            Msg::Tick => {
                if self.main.is_some() {
                    self.refresh();
                }
            }
            Msg::Progress(f, detail) => {
                if let Some(w) = &mut self.working {
                    w.fraction = f;
                    w.detail = detail;
                }
            }
            Msg::Done { fail, result } => {
                // Stopped on purpose: say what that left, not that it failed.
                let stopped = self.working.take().and_then(|w| w.stop).is_some_and(|s| s.load(Ordering::Relaxed));
                match result {
                    Ok(item) => self.pending = item,
                    Err(e) if stopped => self.alert(cx, e),
                    Err(e) => self.alert(cx, format!("{fail} {e}")),
                }
                self.refresh();
            }
            Msg::CanResize(window, r) => {
                if let Some(Win::Resize { can, .. }) = self.windows.get_mut(&window) {
                    *can = Some(r);
                    cx.redraw(window);
                }
            }
            Msg::Bench(window, m) => {
                if let Some(Win::Bench(b)) = self.windows.get_mut(&window) {
                    match m {
                        BenchMsg::Sample(rate) => b.samples.push(rate),
                        BenchMsg::Done(r) => {
                            b.stop = None;
                            match r {
                                Ok(r) => {
                                    b.samples = r.samples.clone();
                                    b.result = Some(r);
                                }
                                Err(e) => b.error = Some(e),
                            }
                        }
                    }
                    cx.redraw(window);
                }
            }
        }
        self.redraw(cx);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        serde_json::to_string(&Saved { selected: self.selected.clone() }).ok()
    }
}

/// The disk or volume named by `device` (/dev/sda1, sda1, or a UDisks
/// object path).
fn find<'a>(status: &'a Status, device: &str) -> Option<(&'a Disk, Option<&'a Volume>)> {
    let dev = if device.starts_with('/') { device.to_owned() } else { format!("/dev/{device}") };
    let is = |d: &str, p: &str| d == dev || p == device;
    status.disks.iter().find_map(|d| {
        if is(&d.device, &d.path) {
            return Some((d, None));
        }
        d.volumes.iter().flat_map(|v| std::iter::once(v).chain(v.cleartext.as_deref())).find(|v| is(&v.device, &v.path)).map(|v| (d, Some(v)))
    })
}

/// The command line, for agents: the exit code, or None to open the app.
fn cli(args: &[String]) -> Option<i32> {
    let flag = args.first()?.as_str();
    if !matches!(flag, "--list" | "--mount" | "--unmount" | "--eject") {
        return None;
    }
    let fail = |e: String| {
        eprintln!("olo-drive-setup: {e}");
        Some(1)
    };
    let status = match disks::status() {
        Ok(s) => s,
        Err(e) => return fail(disks::error_text(&e)),
    };
    if flag == "--list" {
        println!("{}", serde_json::to_string_pretty(&status).unwrap_or_default());
        return Some(0);
    }
    let Some(device) = args.get(1) else { return fail(format!("{flag} needs a device, such as sdb1")) };
    let Some((disk, volume)) = find(&status, device) else { return fail(format!("there's no {device}")) };
    let r = match (flag, volume) {
        ("--mount", Some(v)) => disks::mount(&v.path).map(|at| println!("{at}")),
        ("--unmount", Some(v)) => disks::unmount(&v.path),
        ("--eject", _) if disk.system => return fail(format!("{} holds the running system", disk.device)),
        ("--eject", _) => disks::eject(disk),
        _ => return fail(format!("{device} is a disk; name a volume on it")),
    };
    match r {
        Ok(()) => Some(0),
        Err(e) => fail(disks::error_text(&e)),
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let saved: Saved = cx.restored_state().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let proxy = cx.proxy();
        let watcher = proxy.clone();
        std::thread::spawn(move || {
            let tell = watcher.clone();
            if let Err(e) = disks::watch(move || tell.send(Msg::Changed)) {
                tracing::warn!("can't watch UDisks: {e}");
            }
        });
        let ticker = proxy.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(SPACE_EVERY);
                ticker.send(Msg::Tick);
            }
        });
        let mut app = DriveSetup {
            windows: HashMap::new(),
            main: None,
            proxy,
            status: Status::default(),
            loaded: false,
            error: None,
            loading: false,
            again: false,
            selected: None,
            sel_disk: None,
            pending: saved.selected,
            reveal: false,
            list: ListState::default(),
            working: None,
        };
        app.refresh();
        app.open(cx);
        app
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(path: &str, offset: u64, size: u64, content: &str) -> Volume {
        Volume { path: path.into(), device: format!("/dev/{path}"), offset, size, number: Some(1), content: content.into(), usage: "filesystem".into(), ..Default::default() }
    }

    #[test]
    fn rows_show_disks_volumes_and_free_space() {
        let mut luks = volume("sdb2", 600 * MIB, 300 * MIB, "crypto_LUKS");
        luks.encrypted = true;
        luks.cleartext = Some(Box::new(volume("dm-0", 0, 298 * MIB, "ext4")));
        let disk = Disk { path: "sdb".into(), device: "/dev/sdb".into(), size: 1000 * MIB, table: Some("gpt".into()), volumes: vec![volume("sdb1", MIB, 499 * MIB, "exfat"), luks], ..Default::default() };
        let status = Status { disks: vec![disk], ..Default::default() };
        let r = rows(&status);
        let items: Vec<(Item, usize)> = r.iter().map(|r| (r.item.clone(), r.depth)).collect();
        assert_eq!(
            items,
            vec![
                (Item::Disk("sdb".into()), 0),
                (Item::Volume("sdb1".into()), 1),
                (Item::Free { disk: "sdb".into(), offset: 500 * MIB, size: 100 * MIB }, 1),
                (Item::Volume("sdb2".into()), 1),
                (Item::Volume("dm-0".into()), 2),
                (Item::Free { disk: "sdb".into(), offset: 900 * MIB, size: 99 * MIB }, 1),
            ]
        );
        assert_eq!(find(&status, "dm-0").map(|(d, v)| (d.device.as_str(), v.map(|v| v.path.as_str()))), Some(("/dev/sdb", Some("dm-0"))));
        assert_eq!(find(&status, "/dev/sdb").map(|(_, v)| v.is_none()), Some(true));
    }

    #[test]
    fn names_fit_their_file_system() {
        let mut f = FsFields::new(vec![("vfat", "FAT")], "TWELVE CHARS".into());
        assert!(f.problem().is_some());
        f.name = "STICK".into();
        assert!(f.ready());
        f.encrypt = true;
        assert!(!f.ready());
        f.pass = "secret".into();
        f.again = "secreT".into();
        assert!(f.problem().is_some());
        f.again = "secret".into();
        assert!(f.ready());
        assert_eq!(f.format().passphrase.as_deref(), Some("secret"));
    }
}
