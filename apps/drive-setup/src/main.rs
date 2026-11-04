//! DriveSetup: the disks, their partitions and what is on them, after
//! BeOS's DriveSetup and Haiku's (a map of the chosen disk over a list of
//! every disk and volume). Mount and unmount, unlock and lock, format,
//! make, delete, resize and rename partitions, give a disk a new
//! partition table, write a disk image onto it, eject it, check its health
//! and measure how fast it reads. Everything talks to UDisks2
//! (`olo_system::disks`) on worker threads; UDisks asks polkit before
//! privileged steps. Every destructive step is behind an alert naming the
//! disk, and the disk holding the running system is never offered for one.
//! On the GPUI kit (olo-ui).
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
    time::Duration,
};

use olo_system::disks::{self, Disk, Segment, Space, Status, Volume};
use olo_theme::mix;
use olo_ui::{
    Frame, Menu, MenuBar, WindowOptions,
    file_panel::{self, FilePanel, PanelResult},
    files::format_size,
    gpui::{
        self, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable as _, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
        Styled as _, Task, WeakEntity, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons,
    instance::{self, Launch},
    menu, metrics as m,
    style::{self, c},
    theme, tracker,
    widgets::{
        ALERT_TEXT_X, AlertFrame, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, Slider, StatusBar, TextControl, TextField, alert, button_row, level_bar, open_alert, paragraph,
        paragraph_height, text_field,
    },
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.drive-setup";
const MIB: u64 = 1 << 20;
const MAP_H: f32 = 52.0;
const FORM_W: f32 = 440.0;
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
const SPACE_EVERY: Duration = Duration::from_secs(5);
/// A slider's height with its label above and its limits under it.
const SLIDER_H: f32 = m::LABEL_ROW * 2.0 + 24.0;

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
        out.push(Row {
            item: Item::Disk(d.path.clone()),
            depth: 0,
            icon: Some(disk_icon(d)),
            cells: [d.name(), short(&d.device), table_name(d), format_size(d.size), String::new(), String::new()],
            space: None,
        });
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

/// The name, file system and encryption of a new file system (Format and
/// Create share them), as typed.
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
    #[cfg(test)]
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
}

/// A new file system's fields in a form.
struct FsForm {
    name: Entity<TextField>,
    pass: Entity<TextField>,
    again: Entity<TextField>,
    kind: usize,
    kinds: Vec<(&'static str, &'static str)>,
    encrypt: bool,
    erase: bool,
}

impl FsForm {
    fn new(kinds: Vec<(&'static str, &'static str)>, name: String, cx: &mut App) -> FsForm {
        let field = text_field(cx, |f| f.name("Name"));
        field.update(cx, |f, cx| f.set_text(name, cx));
        FsForm { name: field, pass: text_field(cx, |f| f.name("Passphrase").password(true)), again: text_field(cx, |f| f.name("Again").password(true)), kind: 0, kinds, encrypt: false, erase: false }
    }

    fn values(&self, cx: &App) -> FsFields {
        FsFields {
            name: self.name.read(cx).text().to_owned(),
            kind: self.kind,
            kinds: self.kinds.clone(),
            encrypt: self.encrypt,
            pass: self.pass.read(cx).text().to_owned(),
            again: self.again.read(cx).text().to_owned(),
            erase: self.erase,
        }
    }

    /// The fields' height (`erase`: with the checkbox for zeros).
    fn height(&self, erase: bool, cx: &App) -> f32 {
        let mut h = 2.0 * (m::CONTROL_HEIGHT + m::TIGHT) + m::CONTROL_HEIGHT;
        if self.encrypt {
            h += 2.0 * (m::CONTROL_HEIGHT + m::TIGHT);
        }
        if erase {
            h += m::CONTROL_HEIGHT;
        }
        if self.values(cx).problem().is_some() {
            h += m::LABEL_ROW;
        }
        h
    }
}

/// What a form is for.
enum FormKind {
    Format { path: String, release: Option<Volume>, title: String, message: String, fs: FsForm },
    Create { disk: String, table: String, offset: u64, max: u64, size: f32, message: String, fs: FsForm },
    Resize { path: String, title: String, current: u64, max: u64, size: f32, has_fs: bool, can: Option<Result<(bool, bool), String>> },
    Rename { path: String, title: String, fs: bool, name: Entity<TextField> },
    Unlock { path: String, title: String, pass: Entity<TextField> },
    Table { disk: Box<Disk>, kind: usize, erase: bool },
}

/// A form's window: an alert with fields, sized to what it shows.
struct Form {
    app: WeakEntity<DriveSetup>,
    kind: FormKind,
    focus: FocusHandle,
    /// The field that takes the keyboard first, once.
    first: Option<FocusHandle>,
    done: bool,
    /// Return in a field submits (subscribed once the window is there).
    returns: Vec<gpui::Subscription>,
}

/// What a form asks for, as typed: done by [`DriveSetup::submit`].
enum Submit {
    Format { path: String, release: Option<Volume>, title: String, format: disks::Format },
    Create { disk: String, table: String, offset: u64, size: u64, name: String, format: disks::Format },
    Resize { path: String, title: String, current: u64, size: u64, has_fs: bool },
    Rename { path: String, title: String, fs: bool, name: String },
    Unlock { path: String, title: String, pass: String },
    Table { disk: Box<Disk>, kind: usize, erase: bool },
}

impl Form {
    /// The slider's range for a resize: down only if the file system
    /// shrinks, up only if it grows.
    fn resize_range(current: u64, max: u64, can: &Option<Result<(bool, bool), String>>) -> (f32, f32) {
        let (shrink, grow) = match can {
            Some(Ok(sg)) => *sg,
            _ => (false, false),
        };
        let cur = (current / MIB) as f32;
        (if shrink { 1.0 } else { cur }, if grow { (max / MIB) as f32 } else { cur })
    }

    /// Whether the default button can be pressed.
    fn ready(&self, cx: &App) -> bool {
        match &self.kind {
            FormKind::Format { fs, .. } => fs.values(cx).ready(),
            FormKind::Create { fs, size, .. } => fs.values(cx).ready() && *size >= 1.0,
            FormKind::Resize { current, size, can, .. } => matches!(can, Some(Ok(_))) && (*size as u64) != current / MIB,
            FormKind::Rename { .. } | FormKind::Table { .. } => true,
            FormKind::Unlock { pass, .. } => !pass.read(cx).text().is_empty(),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.done || !self.ready(cx) {
            return;
        }
        self.done = true;
        let s = match &self.kind {
            FormKind::Format { path, release, title, fs, .. } => Submit::Format { path: path.clone(), release: release.clone(), title: title.clone(), format: fs.values(cx).format() },
            FormKind::Create { disk, table, offset, size, fs, .. } => {
                let v = fs.values(cx);
                Submit::Create { disk: disk.clone(), table: table.clone(), offset: *offset, size: (*size as u64) * MIB, name: v.name.trim().to_owned(), format: v.format() }
            }
            FormKind::Resize { path, title, current, size, has_fs, .. } => Submit::Resize { path: path.clone(), title: title.clone(), current: *current, size: (*size as u64) * MIB, has_fs: *has_fs },
            FormKind::Rename { path, title, fs, name } => Submit::Rename { path: path.clone(), title: title.clone(), fs: *fs, name: name.read(cx).text().trim().to_owned() },
            FormKind::Unlock { path, title, pass } => Submit::Unlock { path: path.clone(), title: title.clone(), pass: pass.read(cx).text().to_owned() },
            FormKind::Table { disk, kind, erase } => Submit::Table { disk: disk.clone(), kind: *kind, erase: *erase },
        };
        if let FormKind::Unlock { pass, .. } = &self.kind {
            pass.update(cx, |f, cx| f.clear_secret(cx));
        }
        let app = self.app.clone();
        cx.defer(move |cx| {
            let _ = app.update(cx, |d, cx| d.submit(s, cx));
        });
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window) {
        self.done = true;
        window.remove_window();
    }

    /// A new file system's fields, top down.
    fn fs_fields(fs: &FsForm, erase: bool, me: &Entity<Form>, cx: &App) -> gpui::Div {
        let edit = |f: fn(&mut FsForm, bool)| {
            let me = me.clone();
            move |v: bool, _: &mut Window, cx: &mut App| {
                me.update(cx, |form, cx| {
                    if let FormKind::Format { fs, .. } | FormKind::Create { fs, .. } = &mut form.kind {
                        f(fs, v);
                    }
                    cx.notify();
                })
            }
        };
        let names: Vec<SharedString> = fs.kinds.iter().map(|k| SharedString::from(k.1)).collect();
        let kind_me = me.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .child(div().h(px(m::CONTROL_HEIGHT)).child(TextControl::new(fs.name.clone()).label("Name:").divider(DIVIDER)))
            .child(div().h(px(m::TIGHT)))
            .child(div().h(px(m::CONTROL_HEIGHT)).child(MenuField::new("drive-setup-kind", names, fs.kind).label("File system:").divider(DIVIDER).on_change(move |i, _, cx| {
                kind_me.update(cx, |form, cx| {
                    if let FormKind::Format { fs, .. } | FormKind::Create { fs, .. } = &mut form.kind {
                        fs.kind = i;
                    }
                    cx.notify();
                })
            })))
            .child(div().h(px(m::TIGHT)))
            .child(div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(CheckBox::new("Encrypt it with a passphrase", fs.encrypt).on_toggle(edit(|fs, v| fs.encrypt = v))));
        if fs.encrypt {
            col = col
                .child(div().h(px(m::CONTROL_HEIGHT)).child(TextControl::new(fs.pass.clone()).label("Passphrase:").divider(DIVIDER)))
                .child(div().h(px(m::TIGHT)))
                .child(div().h(px(m::CONTROL_HEIGHT)).child(TextControl::new(fs.again.clone()).label("Again:").divider(DIVIDER)))
                .child(div().h(px(m::TIGHT)));
        }
        if erase {
            col = col.child(div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(CheckBox::new("Overwrite the old data with zeros (slow)", fs.erase).on_toggle(edit(|fs, v| fs.erase = v))));
        }
        if let Some(problem) = fs.values(cx).problem() {
            col = col.child(failure(problem));
        }
        col
    }
}

/// A line in the failure colour.
fn failure(text: impl Into<SharedString>) -> impl IntoElement {
    let text = text.into();
    canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), c(p.failure), b.size.width);
        },
    )
    .w_full()
    .h(px(m::LABEL_ROW))
    .flex_shrink_0()
}

impl Form {
    /// The form's text fields.
    fn fields(&self) -> Vec<Entity<TextField>> {
        match &self.kind {
            FormKind::Format { fs, .. } | FormKind::Create { fs, .. } => vec![fs.name.clone(), fs.pass.clone(), fs.again.clone()],
            FormKind::Rename { name, .. } => vec![name.clone()],
            FormKind::Unlock { pass, .. } => vec![pass.clone()],
            _ => Vec::new(),
        }
    }
}

impl Render for Form {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        // Return in a field (which takes it before the window does)
        // submits the form; typing there redraws it.
        if self.returns.is_empty() {
            for field in self.fields() {
                self.returns.push(cx.subscribe_in(&field, window, |f, _, e: &olo_ui::widgets::TextFieldEvent, window, cx| match e {
                    olo_ui::widgets::TextFieldEvent::Entered => f.submit(window, cx),
                    _ => cx.notify(),
                }));
            }
        }
        // The keyboard is in the form (Return and Escape are its), on its
        // first field to begin with.
        if let Some(first) = self.first.take() {
            window.focus(&first);
        } else if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus);
        }
        let col_w = FORM_W - ALERT_TEXT_X - m::WINDOW_INSET;
        let slider = |id: &'static str, value: f32, lo: f32, hi: f32| {
            let me = me.clone();
            Slider::new(id, value, lo..=hi).step(1.0).label("Size:").value_text(format_size((value as u64) * MIB)).limits(format_size((lo as u64) * MIB), format_size((hi as u64) * MIB)).on_change(
                move |v, _, cx| {
                    me.update(cx, |form, cx| {
                        if let FormKind::Create { size, .. } | FormKind::Resize { size, .. } = &mut form.kind {
                            *size = v;
                        }
                        cx.notify();
                    })
                },
            )
        };
        // The icon, the message, the fields and their height, the default
        // button.
        let (icon, message, fields, fields_h, button): (&str, String, gpui::Div, f32, &str) = match &self.kind {
            FormKind::Format { message, fs, .. } => ("warning", message.clone(), Form::fs_fields(fs, true, &me, cx), fs.height(true, cx), "Format"),
            FormKind::Create { message, fs, max, size, .. } => {
                let max_mib = (*max / MIB).max(1) as f32;
                let fields = div()
                    .flex()
                    .flex_col()
                    .child(div().h(px(SLIDER_H)).child(slider("drive-setup-size", *size, 1.0, max_mib)))
                    .child(div().h(px(m::SPACING)))
                    .child(Form::fs_fields(fs, false, &me, cx));
                ("drive-setup", message.clone(), fields, SLIDER_H + m::SPACING + fs.height(false, cx), "Create")
            }
            FormKind::Resize { title, current, max, size, can, .. } => {
                let message = format!("Resize “{title}”, now {}.", format_size(*current));
                let (lo, hi) = Form::resize_range(*current, *max, can);
                let (fields, h) = match can {
                    None => {
                        let t = "Finding out what this file system allows…";
                        (div().child(paragraph(t)), paragraph_height(cx, t, col_w))
                    }
                    Some(Err(e)) => (div().child(failure(e.clone())), m::LABEL_ROW),
                    Some(Ok(_)) if hi <= lo => {
                        let t = "There is no room to grow it, and its file system can't shrink.";
                        (div().child(paragraph(t)), paragraph_height(cx, t, col_w))
                    }
                    Some(Ok(_)) => (div().h(px(SLIDER_H)).child(slider("drive-setup-resize", *size, lo, hi)), SLIDER_H),
                };
                ("drive-setup", message, fields, h, "Resize")
            }
            FormKind::Rename { title, name, .. } => {
                ("drive-setup", format!("A new name for “{title}”:"), div().h(px(m::CONTROL_HEIGHT)).child(TextControl::new(name.clone())), m::CONTROL_HEIGHT, "Rename")
            }
            FormKind::Unlock { title, pass, .. } => {
                ("vault", format!("“{title}” is encrypted. Its passphrase:"), div().h(px(m::CONTROL_HEIGHT)).child(TextControl::new(pass.clone())), m::CONTROL_HEIGHT, "Unlock")
            }
            FormKind::Table { disk, kind, erase } => {
                let message = format!("A new partition table erases everything on “{}” ({}, {}): every partition on it goes.", disk.name(), disk.device, format_size(disk.size));
                let (k, e) = (me.clone(), me.clone());
                let fields = div()
                    .flex()
                    .flex_col()
                    .child(div().h(px(m::CONTROL_HEIGHT)).child(
                        MenuField::new("drive-setup-table", vec!["GUID (GPT, for anything recent)".into(), "MBR (for older devices)".into()], *kind).label("Type:").divider(DIVIDER).on_change(
                            move |i, _, cx| {
                                k.update(cx, |form, cx| {
                                    if let FormKind::Table { kind, .. } = &mut form.kind {
                                        *kind = i;
                                    }
                                    cx.notify();
                                })
                            },
                        ),
                    ))
                    .child(div().h(px(m::TIGHT)))
                    .child(div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(CheckBox::new("Overwrite the old data with zeros (slow)", *erase).on_toggle(move |v, _, cx| {
                        e.update(cx, |form, cx| {
                            if let FormKind::Table { erase, .. } = &mut form.kind {
                                *erase = v;
                            }
                            cx.notify();
                        })
                    })));
                ("warning", message, fields, 2.0 * m::CONTROL_HEIGHT + m::TIGHT, "Erase")
            }
        };
        // (The window grows or shrinks to what it shows, fields coming
        // and going: `fits_content`.)
        let _ = fields_h;
        let ready = self.ready(cx);
        let (cancel, ok) = (me.clone(), me.clone());
        let buttons = button_row()
            .child(Button::new("Cancel").on_click(move |window, cx| cancel.update(cx, |f, _| f.cancel(window))))
            .child(Button::new(button).default(true).enabled(ready).on_click(move |window, cx| ok.update(cx, |f, cx| f.submit(window, cx))));
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|f, _: &alert::Default, window, cx| f.submit(window, cx)))
            .on_action(cx.listener(|f, _: &alert::Cancel, window, _| f.cancel(window)))
            .child(AlertFrame::new(icon.to_owned()).child(paragraph(message)).child(div().h(px(m::SPACING))).child(fields).child(div().flex_1()).child(buttons))
    }
}

/// The benchmark's window.
struct Bench {
    id: u64,
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

impl Bench {
    fn start(&mut self, cx: &mut Context<Self>) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        self.samples.clear();
        self.result = None;
        self.error = None;
        let (path, size, id, proxy) = (self.path.clone(), self.size, self.id, olo_ui::proxy(cx));
        std::thread::spawn(move || {
            let r = disks::benchmark(&path, size, BENCH_SAMPLES, &stop, &mut |_, rate| proxy.send(Msg::Bench(id, BenchMsg::Sample(rate))));
            proxy.send(Msg::Bench(id, BenchMsg::Done(r)));
        });
        cx.notify();
    }
}

impl Render for Bench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let intro = format!("How fast “{}” reads, from its start to its end, and how long it takes to find a block. Nothing is written.", self.title);
        let samples = self.samples.clone();
        let graph = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let inner = style::inset(window, b, 2.0);
                style::fill_rect(window, inner, c(p.document_background));
                let grid = c(mix(p.document_text, p.document_background, 0.9));
                for k in 1..4 {
                    let gy = inner.top() + inner.size.height * (k as f32 / 4.0);
                    style::hline(window, inner.left(), inner.right(), gy, grid);
                }
                let top = olo_ui::widgets::nice_ceiling(samples.iter().copied().fold(0.0, f64::max));
                if !samples.is_empty() {
                    let faint = c(mix(p.document_text, p.document_background, 0.4));
                    style::label(window, cx, inner.left() + px(4.0), inner.top() + px(8.0), style::Align::Left, rate_text(top), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), faint);
                }
                if samples.len() > 1 {
                    let step = f32::from(inner.size.width) / (BENCH_SAMPLES - 1) as f32;
                    let h = f32::from(inner.size.height);
                    let pts: Vec<Point<Pixels>> =
                        samples.iter().enumerate().map(|(i, &v)| point(inner.left() + px(i as f32 * step), inner.bottom() - px(2.0 + (v / top) as f32 * (h - 4.0)))).collect();
                    let ink = c(p.keyboard_navigation);
                    window.with_content_mask(Some(gpui::ContentMask { bounds: inner }), |window| {
                        window.paint_layer(inner, |window| {
                            for w in pts.windows(2) {
                                window.paint_segment(w[0], w[1], px(1.5), true, ink);
                            }
                        });
                    });
                }
                style::sunken(window, b, &p, false);
                olo_ui::agent::record(
                    window,
                    cx,
                    olo_ui::agent::Node { role: "Figure", name: Some("Read speed".into()), value: samples.last().map(|v| rate_text(*v).into()), bounds: b, enabled: true, ..Default::default() },
                );
            },
        )
        .w_full()
        .flex_1();
        let text = match (&self.result, &self.error) {
            (_, Some(e)) => e.clone(),
            (Some(r), _) => format!(
                "Average {}, slowest {}, fastest {}; a block found in {}.",
                rate_text(r.average),
                rate_text(r.min),
                rate_text(r.max),
                if r.access_ms < 1.0 { format!("{:.0} µs", r.access_ms * 1000.0) } else { format!("{:.1} ms", r.access_ms) }
            ),
            (None, _) if self.stop.is_some() => format!("Reading… {} of {BENCH_SAMPLES}", self.samples.len()),
            (None, _) => "Choose Start to measure.".into(),
        };
        let failed = self.error.is_some();
        let result = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let color = if failed { c(p.failure) } else { c(p.panel_text) };
                style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, b.size.width);
            },
        )
        .w_full()
        .h(px(m::LABEL_ROW))
        .flex_shrink_0();
        let running = self.stop.is_some();
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .child(paragraph(intro))
            .child(div().h(px(m::SPACING)))
            .child(graph)
            .child(div().h(px(m::TIGHT)))
            .child(result)
            .child(div().h(px(m::SPACING)))
            .child(button_row().child(Button::new(if running { "Stop" } else { "Start" }).default(!running).on_click(move |_, cx| me.update(cx, |b, cx| b.start(cx)))))
    }
}

enum Msg {
    Status(Result<Status, String>),
    Changed,
    Progress(Option<f32>, String),
    Done { fail: String, result: Result<Option<Item>, String> },
    CanResize(u64, Result<(bool, bool), String>),
    Bench(u64, BenchMsg),
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
    window: Option<WindowHandle<Frame>>,
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
    list: Entity<ListModel>,
    working: Option<Working>,
    /// Forms waiting on a worker (a resize's limits) and benchmarks, by id.
    forms: HashMap<u64, WeakEntity<Form>>,
    benches: HashMap<u64, WeakEntity<Bench>>,
    next_id: u64,
    /// Redraws a progress bar with no measure (it sweeps) while a step
    /// runs.
    sweep: Option<Task<()>>,
}

impl DriveSetup {
    fn open(&mut self, cx: &mut Context<Self>) {
        let view = cx.entity();
        olo_ui::open_or_raise(self.window, cx, WindowOptions::new("DriveSetup", 800.0, 480.0).min_size(480.0, 340.0).keep_size("main"), move |_, _| view, |d, w| d.window = w);
    }

    /// Read everything again on a worker (once more after, if asked
    /// while reading).
    fn refresh(&mut self, cx: &App) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let proxy = olo_ui::proxy(cx);
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

    fn alert(&self, message: String, cx: &mut Context<Self>) {
        cx.defer(move |cx| {
            open_alert(cx, "DriveSetup", "stop", &message, &["OK"], |_, _| {});
        });
    }

    fn confirm(&self, message: String, button: &'static str, action: Action, cx: &mut Context<Self>) {
        let me = cx.entity();
        cx.defer(move |cx| {
            let action = std::cell::RefCell::new(Some(action));
            open_alert(cx, "DriveSetup", "warning", &message, &["Cancel", button], move |choice, cx| {
                if choice == Some(1)
                    && let Some(action) = action.borrow_mut().take()
                {
                    me.update(cx, |d, cx| d.act(action, cx));
                }
            });
        });
    }

    /// Opens a form; its id (for what a worker says back about it).
    fn form(&mut self, title: &str, kind: impl FnOnce(&mut App) -> FormKind + 'static, cx: &mut Context<Self>) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let me = cx.entity();
        let title = title.to_owned();
        cx.defer(move |cx| {
            let kind = kind(cx);
            // The first field takes the keyboard.
            let first = match &kind {
                FormKind::Format { fs, .. } | FormKind::Create { fs, .. } => Some(fs.name.focus_handle(cx)),
                FormKind::Rename { name, .. } => Some(name.focus_handle(cx)),
                FormKind::Unlock { pass, .. } => Some(pass.focus_handle(cx)),
                _ => None,
            };
            let form = cx.new(|cx| Form { app: me.downgrade(), kind, focus: cx.focus_handle(), first, done: false, returns: Vec::new() });
            me.update(cx, |d, _| d.forms.insert(id, form.downgrade()));
            let view = form.clone();
            let options = WindowOptions { fixed: true, ..WindowOptions::new(title, FORM_W, 120.0).dialog() };
            olo_ui::open_window(cx, options, move |_, _| view);
        });
        id
    }

    /// Do `work` on a worker as the step under way: it can report
    /// progress, and says what to select after. `fail` begins the alert
    /// if it fails.
    fn run(
        &mut self,
        what: String,
        fail: String,
        stop: Option<Arc<AtomicBool>>,
        work: impl FnOnce(&dyn Fn(Option<f32>, String)) -> Result<Option<Item>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.working.is_some() {
            return;
        }
        self.working = Some(Working { what, stop, fraction: None, detail: String::new() });
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let report = |f: Option<f32>, detail: String| proxy.send(Msg::Progress(f, detail));
            let result = work(&report);
            proxy.send(Msg::Done { fail, result });
        });
        self.sweeping(cx);
        cx.notify();
    }

    /// While a step runs with no measure, its bar sweeps: ten frames a
    /// second, and none once it's done or measured.
    fn sweeping(&mut self, cx: &mut Context<Self>) {
        let unmeasured = self.working.as_ref().is_some_and(|w| w.fraction.is_none()) || (self.working.is_none() && self.status.jobs.first().is_some_and(|j| j.progress.is_none()));
        if !unmeasured {
            self.sweep = None;
            return;
        }
        if self.sweep.is_some() {
            return;
        }
        self.sweep = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    fn command(&mut self, command: &str, cx: &mut Context<Self>) {
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
                self.run(format!("Mounting “{t}”…"), format!("“{t}” couldn't be mounted."), None, move |_| disks::mount(&v.path).map(|_| None).map_err(|e| disks::error_text(&e)), cx);
            }
            "unmount" if can.unmount => {
                let v = volume.unwrap();
                let t = v.title();
                self.run(format!("Unmounting “{t}”…"), format!("“{t}” couldn't be unmounted."), None, move |_| e(disks::unmount(&v.path)), cx);
            }
            "unlock" if can.unlock => {
                let v = volume.unwrap();
                let title = v.title();
                self.form("Unlock", move |cx| FormKind::Unlock { path: v.path, title, pass: text_field(cx, |f| f.name("Passphrase").password(true)) }, cx);
            }
            "lock" if can.lock => {
                let v = volume.unwrap();
                // The encrypted volume itself, or what this is unlocked from.
                let (o, t) = match outer {
                    Some(o) => (o.clone(), o.title()),
                    None => (v.clone(), v.title()),
                };
                self.run(format!("Locking “{t}”…"), format!("“{t}” couldn't be locked."), None, move |_| e(disks::release(&o)), cx);
            }
            "format" if can.format => {
                let v = volume.unwrap();
                let title = v.title();
                let where_ = if outer.is_some() { format!("inside the encrypted volume on “{dname}”") } else { format!("on “{dname}”") };
                let message = format!("Formatting erases everything on “{title}” ({}, {}) {where_}.", v.device, format_size(v.size));
                let kinds = disks::formats(&self.status.filesystems);
                let label = v.label.clone();
                let release = Some(v.clone()).filter(Volume::in_use);
                self.form("Format", move |cx| FormKind::Format { path: v.path, release, title, message, fs: FsForm::new(kinds, label, cx) }, cx);
            }
            "create" if can.create => {
                let (offset, size) = free.unwrap();
                let max = size / MIB * MIB;
                let message = format!("Make a partition in the {} of free space on “{dname}”.", format_size(size));
                let kinds = disks::formats(&self.status.filesystems);
                let table = disk.table.clone().unwrap_or_default();
                self.form("Create Partition", move |cx| FormKind::Create { disk: disk.path, table, offset, max, size: (max / MIB) as f32, message, fs: FsForm::new(kinds, String::new(), cx) }, cx);
            }
            "delete" if can.delete => {
                let v = volume.unwrap();
                let part = outer.unwrap_or(v);
                let title = part.title();
                let message = format!("Delete “{title}” ({}, {}) from “{dname}”? Everything on it will be lost.", part.device, format_size(part.size));
                let release = Some(part.clone()).filter(Volume::in_use);
                self.confirm(message, "Delete", Action::Delete { part: part.path, release, title }, cx);
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
                let (path, title, current, kind) = (v.path.clone(), v.title(), v.size, v.content.clone());
                let id = self.form("Resize", move |_| FormKind::Resize { path, title, current, max, size: (current / MIB) as f32, has_fs, can: (!has_fs).then_some(Ok((true, true))) }, cx);
                if has_fs {
                    let proxy = olo_ui::proxy(cx);
                    std::thread::spawn(move || proxy.send(Msg::CanResize(id, disks::can_resize(&kind))));
                }
            }
            "rename" if can.rename => {
                let v = volume.unwrap();
                let name = if v.mountable { v.label.clone() } else { v.name.clone() };
                let (path, title, fs) = (v.path.clone(), v.title(), v.mountable);
                self.form(
                    "Rename",
                    move |cx| {
                        let field = text_field(cx, |f| f.name("Name"));
                        field.update(cx, |f, cx| f.set_text(name, cx));
                        FormKind::Rename { path, title, fs, name: field }
                    },
                    cx,
                );
            }
            tracker::COMMAND if can.tracker => {
                let dir = volume.unwrap().mount_points[0].clone();
                if let Err(e) = tracker::open(std::path::Path::new(&dir)) {
                    tracing::warn!("couldn't open Tracker: {e}");
                }
            }
            "eject" if can.eject => {
                self.run(format!("Ejecting “{dname}”…"), format!("“{dname}” couldn't be ejected."), None, move |_| e(disks::eject(&disk)), cx);
            }
            "new-table" if can.table => {
                self.form("New Partition Table", move |_| FormKind::Table { disk: Box::new(disk), kind: 0, erase: false }, cx);
            }
            "write-image" if can.image => {
                let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
                let me = cx.entity();
                cx.defer(move |cx| {
                    file_panel::show(cx, "Write Disk Image", FilePanel::open(home).types(&["iso", "img", "raw", "bin"]), move |a, cx| {
                        if let PanelResult::Chosen(image) = a.result {
                            me.update(cx, |d, cx| d.chose_image(disk, image, cx));
                        }
                    });
                });
            }
            "benchmark" if can.bench => {
                self.next_id += 1;
                let id = self.next_id;
                let (path, size) = (disk.path.clone(), disk.size);
                let me = cx.entity();
                cx.defer(move |cx| {
                    let bench = cx.new(|_| Bench { id, path, title: dname.clone(), size, samples: Vec::new(), result: None, error: None, stop: None });
                    me.update(cx, |d, _| d.benches.insert(id, bench.downgrade()));
                    let view = bench.clone();
                    olo_ui::open_window(cx, WindowOptions::new(format!("Benchmark: {dname}"), 480.0, 320.0).min_size(400.0, 260.0).floating(), move |_, _| view);
                    // Closing it stops it.
                    cx.observe_release(&bench, |b, _| {
                        if let Some(stop) = &b.stop {
                            stop.store(true, Ordering::Relaxed);
                        }
                    })
                    .detach();
                });
            }
            "check-health" if can.health => {
                let dr = disk.drive.clone().unwrap();
                let nvme = disk.device.starts_with("/dev/nvme");
                self.run(format!("Checking “{dname}”…"), format!("“{dname}” couldn't be checked."), None, move |_| e(disks::smart_update(&dr.path, nvme)), cx);
            }
            "quit" => cx.quit(),
            _ => {}
        }
        cx.notify();
    }

    /// A disk image chosen to write: it must fit; then the alert.
    fn chose_image(&mut self, disk: Disk, image: PathBuf, cx: &mut Context<Self>) {
        let name = disk.name();
        let file = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let len = std::fs::metadata(&image).map(|m| m.len()).unwrap_or(0);
        if len > disk.size {
            return self.alert(format!("“{file}” ({}) is bigger than “{name}” ({}).", format_size(len), format_size(disk.size)), cx);
        }
        let message = format!("Write “{file}” ({}) to “{name}” ({}, {})? Everything on the disk will be erased.", format_size(len), disk.device, format_size(disk.size));
        self.confirm(message, "Write", Action::WriteImage { disk, image }, cx);
    }

    /// A form's default button.
    fn submit(&mut self, s: Submit, cx: &mut Context<Self>) {
        let e = |r: olo_system::Result<()>| r.map(|_| None).map_err(|e| disks::error_text(&e));
        match s {
            Submit::Format { path, release, title, format } => {
                self.run(
                    format!("Formatting “{title}”…"),
                    format!("“{title}” couldn't be formatted."),
                    None,
                    move |_| {
                        if let Some(v) = &release {
                            disks::release(v).map_err(|e| disks::error_text(&e))?;
                        }
                        e(disks::format(&path, &format))
                    },
                    cx,
                );
            }
            Submit::Create { disk, table, offset, size, name, format } => {
                self.run(
                    "Creating a partition…".into(),
                    "The partition couldn't be made.".into(),
                    None,
                    move |_| disks::create_partition(&disk, &table, offset, size, &name, &format).map(|p| Some(Item::Volume(p))).map_err(|e| disks::error_text(&e)),
                    cx,
                );
            }
            Submit::Resize { path, title, current, size, has_fs } => {
                self.run(format!("Resizing “{title}”…"), format!("“{title}” couldn't be resized."), None, move |_| e(disks::resize(&path, current, size, has_fs)), cx);
            }
            Submit::Rename { path, title, fs, name } => {
                self.run(
                    format!("Renaming “{title}”…"),
                    format!("“{title}” couldn't be renamed."),
                    None,
                    move |_| e(if fs { disks::set_label(&path, &name) } else { disks::set_partition_name(&path, &name) }),
                    cx,
                );
            }
            Submit::Unlock { path, title, pass } => {
                self.run(
                    format!("Unlocking “{title}”…"),
                    format!("“{title}” couldn't be unlocked."),
                    None,
                    move |_| disks::unlock(&path, &pass).map(|p| Some(Item::Volume(p))).map_err(|e| disks::error_text(&e)),
                    cx,
                );
            }
            Submit::Table { disk, kind, erase } => {
                let name = disk.name();
                self.run(
                    format!("Erasing “{name}”…"),
                    format!("“{name}” couldn't be erased."),
                    None,
                    move |_| {
                        for v in &disk.volumes {
                            disks::release(v).map_err(|e| disks::error_text(&e))?;
                        }
                        disks::new_table(&disk.path, if kind == 0 { "gpt" } else { "dos" }, erase).map_err(|e| disks::error_text(&e))?;
                        Ok(Some(Item::Disk(disk.path.clone())))
                    },
                    cx,
                );
            }
        }
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Delete { part, release, title } => {
                self.run(
                    format!("Deleting “{title}”…"),
                    format!("“{title}” couldn't be deleted."),
                    None,
                    move |_| {
                        if let Some(v) = &release {
                            disks::release(v).map_err(|e| disks::error_text(&e))?;
                        }
                        disks::delete_partition(&part).map(|_| None).map_err(|e| disks::error_text(&e))
                    },
                    cx,
                );
            }
            Action::WriteImage { disk, image } => {
                let name = disk.name();
                let stop = Arc::new(AtomicBool::new(false));
                let flag = stop.clone();
                self.run(
                    format!("Writing to “{name}”…"),
                    format!("The image couldn't be written to “{name}”."),
                    Some(stop),
                    move |report| {
                        for v in &disk.volumes {
                            disks::release(v).map_err(|e| disks::error_text(&e))?;
                        }
                        disks::write_image(&disk.path, &image, &flag, &mut |p| {
                            let f = p.done as f32 / p.total.max(1) as f32;
                            report(Some(f), format!("{} of {}, {}/s", format_size(p.done), format_size(p.total), format_size(p.rate as u64)));
                        })?;
                        Ok(Some(Item::Disk(disk.path.clone())))
                    },
                    cx,
                );
            }
        }
    }

    /// The context menu of the selection.
    fn item_menu(&self) -> Menu {
        let can = self.can();
        match &self.selected {
            Some(Item::Volume(_)) => Menu::new("")
                .item("Mount", "mount")
                .enabled(can.mount)
                .item("Unmount", "unmount")
                .enabled(can.unmount)
                .item(tracker::LABEL, tracker::COMMAND)
                .enabled(can.tracker)
                .separator()
                .item("Unlock…", "unlock")
                .enabled(can.unlock)
                .item("Lock", "lock")
                .enabled(can.lock)
                .separator()
                .item("Format…", "format")
                .enabled(can.format)
                .item("Rename…", "rename")
                .enabled(can.rename)
                .item("Resize…", "resize")
                .enabled(can.resize)
                .item("Delete…", "delete")
                .enabled(can.delete),
            Some(Item::Free { .. }) => Menu::new("").item("Create Partition…", "create").enabled(can.create),
            _ => self.disk_menu(Menu::new(""), &can),
        }
    }

    fn disk_menu(&self, menu: Menu, can: &Can) -> Menu {
        menu.item("Eject", "eject")
            .key("e")
            .enabled(can.eject)
            .separator()
            .item("Check Health", "check-health")
            .enabled(can.health)
            .item("Benchmark…", "benchmark")
            .enabled(can.bench)
            .separator()
            .item("Write Disk Image…", "write-image")
            .enabled(can.image)
            .item("New Partition Table…", "new-table")
            .enabled(can.table)
    }

    fn menus(&self) -> Vec<Menu> {
        let can = self.can();
        vec![
            self.disk_menu(Menu::new("Disk"), &can).separator().item("Quit", "quit").key("q"),
            Menu::new("Partition")
                .item("Mount", "mount")
                .enabled(can.mount)
                .item("Unmount", "unmount")
                .enabled(can.unmount)
                .item(tracker::LABEL, tracker::COMMAND)
                .enabled(can.tracker)
                .separator()
                .item("Unlock…", "unlock")
                .enabled(can.unlock)
                .item("Lock", "lock")
                .enabled(can.lock)
                .separator()
                .item("Create…", "create")
                .enabled(can.create)
                .item("Format…", "format")
                .enabled(can.format)
                .item("Rename…", "rename")
                .enabled(can.rename)
                .item("Resize…", "resize")
                .enabled(can.resize)
                .item("Delete…", "delete")
                .enabled(can.delete),
        ]
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

    /// The status line: the step under way (with Stop), UDisks's own
    /// jobs, or what is selected.
    fn status_line(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let job = self.status.jobs.first();
        let busy = self.working.as_ref().map(|w| (w.what.clone(), w.fraction, w.detail.clone(), w.stop.clone()));
        let busy = busy.or_else(|| job.map(|j| (format!("{}…", j.title()), j.progress.map(|f| f as f32), String::new(), None)));
        if let Some((what, fraction, detail, stop)) = busy {
            let fraction = fraction.or(job.and_then(|j| j.progress).map(|f| f as f32));
            let cancel = job.filter(|j| j.cancelable).map(|j| j.path.clone());
            let trailing = match (fraction, detail.is_empty()) {
                (_, false) => detail,
                (Some(f), true) => format!("{:.0}%", f * 100.0),
                (None, true) => String::new(),
            };
            // Without a measure, a bar that sweeps.
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            let shown = fraction.unwrap_or(((t * 0.6).fract()) as f32);
            let mut row = div().flex().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).child(div().flex_1().child(StatusBar::new(shown).label(what).trailing(trailing)));
            if stop.is_some() || cancel.is_some() {
                let _ = cx;
                row = row.child(Button::new("Stop").min_width(80.0).on_click(move |_, _| {
                    if let Some(stop) = &stop {
                        stop.store(true, Ordering::Relaxed);
                    } else if let Some(job) = cancel.clone() {
                        std::thread::spawn(move || {
                            if let Err(e) = disks::cancel(&job) {
                                tracing::warn!("couldn't stop the job: {e}");
                            }
                        });
                    }
                }));
            }
            return row.into_any_element();
        }
        let (text, warn) = self.describe();
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let color = if warn { c(p.failure) } else { c(p.panel_text) };
                style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, b.size.width);
            },
        )
        .w_full()
        .h(px(m::CONTROL_HEIGHT))
        .into_any_element()
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if message.is::<Launch>() {
            return self.open(cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Status(r) => {
                self.loading = false;
                if std::mem::take(&mut self.again) {
                    self.refresh(cx);
                }
                // Read again every few seconds: drawn only if it changed.
                if r.as_ref().is_ok_and(|s| self.loaded && *s == self.status && self.error.is_none() && self.pending.is_none()) {
                    return;
                }
                match r {
                    Ok(s) => {
                        self.status = s;
                        self.loaded = true;
                        self.error = None;
                        self.settle();
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Changed => self.refresh(cx),
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
                    Err(e) if stopped => self.alert(e, cx),
                    Err(e) => self.alert(format!("{fail} {e}"), cx),
                }
                self.refresh(cx);
            }
            Msg::CanResize(id, r) => {
                if let Some(form) = self.forms.get(&id).and_then(WeakEntity::upgrade) {
                    form.update(cx, |f, cx| {
                        if let FormKind::Resize { can, .. } = &mut f.kind {
                            *can = Some(r);
                        }
                        cx.notify();
                    });
                }
            }
            Msg::Bench(id, m) => {
                if let Some(bench) = self.benches.get(&id).and_then(WeakEntity::upgrade) {
                    bench.update(cx, |b, cx| {
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
                        cx.notify();
                    });
                }
            }
        }
        self.forms.retain(|_, f| f.upgrade().is_some());
        self.benches.retain(|_, b| b.upgrade().is_some());
        self.sweeping(cx);
        cx.notify();
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&Saved { selected: self.selected.clone() }).ok()
    }
}

/// What a confirmed alert goes on to do.
enum Action {
    Delete { part: String, release: Option<Volume>, title: String },
    WriteImage { disk: Disk, image: PathBuf },
}

impl Render for DriveSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let bar_me = me.clone();
        let bar = MenuBar::new(self.menus(), move |cmd, _, cx| bar_me.update(cx, |d, cx| d.command(cmd, cx)));
        let root = div().size_full().flex().flex_col().child(bar);
        if !self.loaded {
            let error = self.error.clone();
            return root.child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        if let Some(e) = &error {
                            let p = theme::palette(cx);
                            style::label(
                                window,
                                cx,
                                b.center().x,
                                b.center().y,
                                style::Align::Center,
                                format!("Can't see the disks: {e}"),
                                style::PLAIN_SIZE,
                                style::ui_font(FontWeight::NORMAL),
                                c(p.failure),
                            );
                        }
                    },
                )
                .flex_1()
                .w_full(),
            );
        }

        // The chosen disk's map, under its name.
        let disk = self.sel().map(|s| s.disk.clone());
        let selected = self.selected.clone();
        let what = disk.as_ref().map(|d| format!("{} ({}), {}, {}", d.name(), d.device, format_size(d.size), table_name(d))).unwrap_or_default();
        let head = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, what.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), c(p.panel_text), b.size.width);
            },
        )
        .w_full()
        .h(px(m::LABEL_ROW))
        .flex_shrink_0();
        let pick = me.clone();
        let on_pick: map::OnPick = std::rc::Rc::new(move |item, at, window, cx| {
            let menu = pick.update(cx, |d, cx| {
                d.select(item);
                cx.notify();
                d.item_menu()
            });
            if let Some(at) = at {
                let to = pick.clone();
                menu::context_menu(window, cx, menu, at, move |cmd, _, cx| to.update(cx, |d, cx| d.command(cmd, cx)));
            }
        });
        let map_el = canvas(
            |b, window, _| map::hitbox(window, b),
            move |b, hitbox, window, cx| {
                if let Some(disk) = &disk {
                    map::paint(window, cx, b, &hitbox, disk, selected.as_ref(), on_pick.clone());
                }
            },
        )
        .w_full()
        .h(px(MAP_H))
        .flex_shrink_0();

        // The list.
        let rows = std::rc::Rc::new(rows(&self.status));
        let at = self.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.item == s));
        let reveal = std::mem::take(&mut self.reveal);
        self.list.update(cx, |l, _| {
            let want: Vec<usize> = at.into_iter().collect();
            if l.state.selected.iter().copied().collect::<Vec<_>>() != want {
                l.state.selected = want.into_iter().collect();
            }
            if reveal && let Some(i) = at {
                l.reveal(i);
            }
        });
        let columns =
            [Column::new("Name", 200.0), Column::new("Device", 84.0), Column::new("Contents", 120.0), Column::new("Size", 72.0).right(), Column::new("Free", 130.0), Column::new("Mounted At", 140.0)];
        let (cells, sel_rows, ctx_rows, open_rows) = (rows.clone(), rows.clone(), rows.clone(), rows.clone());
        let (sel, ctx, open) = (me.clone(), me.clone(), me.clone());
        let list = ColumnListView::new("drive-setup-list", &self.list, columns, rows.len(), move |cell, window, cx| {
            let row = &cells[cell.row];
            let p = theme::palette(cx);
            if cell.column == 4
                && let Some(space) = row.space
            {
                let left = cell.bounds.left() + px(m::CELL_INSET);
                let bar = style::snap(window, Bounds { origin: point(left, cell.bounds.center().y - px(USAGE_H / 2.0)), size: size(px(USAGE_W), px(USAGE_H)) });
                // On panel grey, so its whole bevel shows on the list's
                // white.
                let w = style::hairline(window);
                style::fill_rect(window, Bounds::from_corners(point(bar.left() - w, bar.top() - w), point(bar.right() + w, bar.bottom() + w)), c(p.panel_background));
                level_bar(window, bar, space.fraction(), nearly_full(&space).then(|| c(p.failure)), &p);
                cell.bounds = Bounds::from_corners(point(bar.right(), cell.bounds.top()), cell.bounds.bottom_right());
                return cell.text(window, cx, row.cells[4].clone());
            }
            if cell.column != 0 {
                return cell.text(window, cx, row.cells[cell.column].clone());
            }
            let x = cell.bounds.left() + px(m::CELL_INSET + row.depth as f32 * INDENT);
            if let Some(icon) = row.icon {
                let r = style::snap_centred(window, point(x + px(ICON / 2.0), cell.bounds.center().y), ICON, ICON);
                icons::paint(window, r, icon);
            }
            cell.bounds = Bounds::from_corners(point(x + px(ICON), cell.bounds.top()), cell.bounds.bottom_right());
            let free = matches!(row.item, Item::Free { .. });
            let color = if free && !cell.selected { c(mix(p.list_item_text, p.list_background, 0.45)) } else { cell.text_color };
            let font = style::ui_font(if row.depth == 0 { FontWeight::BOLD } else { FontWeight::NORMAL });
            cell.text_with(window, cx, row.cells[0].clone(), font, color);
        })
        .empty_text("No disks.")
        .on_select(move |_, cx| {
            let r = sel.read(cx).list.read(cx).state.single();
            if let Some(row) = r.and_then(|i| sel_rows.get(i)) {
                let item = row.item.clone();
                sel.update(cx, |d, cx| {
                    d.select(item);
                    d.reveal = false;
                    cx.notify();
                });
            }
        })
        .on_context(move |i, at, window, cx| {
            let Some(row) = ctx_rows.get(i) else { return };
            let item = row.item.clone();
            let menu = ctx.update(cx, |d, cx| {
                d.select(item);
                d.reveal = false;
                cx.notify();
                d.item_menu()
            });
            let to = ctx.clone();
            menu::context_menu(window, cx, menu, at, move |cmd, _, cx| to.update(cx, |d, cx| d.command(cmd, cx)));
        })
        .on_open(move |i, _, cx| {
            let Some(row) = open_rows.get(i) else { return };
            let item = row.item.clone();
            open.update(cx, |d, cx| {
                d.select(item);
                let can = d.can();
                let cmd = if can.tracker {
                    tracker::COMMAND
                } else if can.mount {
                    "mount"
                } else if can.unlock {
                    "unlock"
                } else if can.create {
                    "create"
                } else {
                    ""
                };
                d.command(cmd, cx);
            });
        });

        let status = self.status_line(cx);
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .p(px(m::WINDOW_INSET))
                .flex()
                .flex_col()
                .child(head)
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(map_el)
                .child(div().h(px(m::SPACING)).flex_shrink_0())
                .child(div().flex_1().min_h_0().child(list))
                .child(div().h(px(m::SPACING)).flex_shrink_0())
                .child(status),
        )
    }
}

/// MB/s in words (GB/s when fast).
fn rate_text(mb: f64) -> String {
    if mb >= 1000.0 {
        format!("{:.1} GB/s", mb / 1000.0)
    } else if mb >= 100.0 {
        format!("{mb:.0} MB/s")
    } else {
        format!("{mb:.1} MB/s")
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
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let saved: Saved = olo_ui::restored_state().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let watcher = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let tell = watcher.clone();
            if let Err(e) = disks::watch(move || tell.send(Msg::Changed)) {
                tracing::warn!("can't watch UDisks: {e}");
            }
        });
        let list = ListModel::new(cx, false);
        let app = cx.new(|_| DriveSetup {
            window: None,
            status: Status::default(),
            loaded: false,
            error: None,
            loading: false,
            again: false,
            selected: None,
            sel_disk: None,
            pending: saved.selected,
            reveal: false,
            list,
            working: None,
            forms: HashMap::new(),
            benches: HashMap::new(),
            next_id: 0,
            sweep: None,
        });
        app.update(cx, |d, cx| {
            d.refresh(cx);
            d.open(cx);
        });
        // The free space, again every few seconds while the window is
        // open (writing files sends UDisks no signal).
        let ticking = app.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(SPACE_EVERY).await;
                let Ok(()) = ticking.update(cx, |d, cx| {
                    if d.window.is_some() {
                        d.refresh(cx);
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |d, cx| d.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state());
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
        let disk =
            Disk { path: "sdb".into(), device: "/dev/sdb".into(), size: 1000 * MIB, table: Some("gpt".into()), volumes: vec![volume("sdb1", MIB, 499 * MIB, "exfat"), luks], ..Default::default() };
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
