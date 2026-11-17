//! An archive looked into as a folder (TODO: "browsing into an archive
//! as a folder"): opening a zip, tar, 7-Zip, RAR, ISO or cpio in Tracker
//! shows what's in it, a level at a time, as a folder's list does
//! (folders first, then by name; size and modified); a folder in it
//! opens in the same window (Go ▸ Enclosing Folder, ⌘↑, back out); a
//! file in it opens with what opens its kind (taken out into Tracker's
//! cache first, once). Extract Selected… takes what's selected out into a
//! folder chosen; Extract All, everything, beside the archive (Tracker's
//! job). A password, when it has one, is asked for here: typed (and
//! kept in Passwords once it has worked, if asked), or Passwords' kept one
//! (`archives::saved_password`).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_archive::{Entry, EntryKind};
use olo_kit::{
    Child, ClickEvent, Cx, Element, IntoElement as _, Menu, MenuBar, ParentElement as _, Styled as _, View, WindowOptions, div,
    file_panel::{self, FilePanel, PanelResult},
    px,
    widgets::{Button, Column, ColumnListView, ListModel, StatusBar, StringView, TextControl, TextField, TextFieldEvent, text_field},
};
use olo_kit_base::metrics as m;

use crate::{ToHub, ops, to_hub, undo};

/// One thing at the level shown.
#[derive(Clone, Debug)]
struct Node {
    name: String,
    /// Its path in the archive.
    rel: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: Option<i64>,
}

/// What's directly in `dir` (folders implied by what's deeper too).
fn children(entries: &[Entry], dir: &Path) -> Vec<Node> {
    let mut map: BTreeMap<String, Node> = BTreeMap::new();
    for e in entries {
        let Some(p) = &e.path else { continue };
        let Ok(rest) = p.strip_prefix(dir) else { continue };
        let mut parts = rest.components();
        let Some(first) = parts.next() else { continue };
        let name = first.as_os_str().to_string_lossy().into_owned();
        let deeper = parts.next().is_some();
        let n = map.entry(name.clone()).or_insert_with(|| Node { name: name.clone(), rel: dir.join(&name), is_dir: deeper, size: 0, mtime: None });
        if deeper {
            n.is_dir = true;
        } else {
            n.is_dir |= e.kind == EntryKind::Dir;
            n.size = e.size;
            n.mtime = e.mtime.or(n.mtime);
        }
    }
    let mut out: Vec<Node> = map.into_values().collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Where a file taken out to be opened goes: Tracker's cache, a folder
/// for each archive as it is now (its path, size and time).
fn cache_for(archive: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    archive.hash(&mut h);
    if let Ok(m) = std::fs::metadata(archive) {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("olo/tracker/archives").join(format!("{:016x}", h.finish()))
}

#[derive(Clone)]
pub enum Msg {
    Listed(Result<Arc<Vec<Entry>>, String>, bool),
    Selected,
    Up,
    Opened,
    Password,
    ExtractAll,
    ExtractTo(PathBuf),
    Extracted(Result<Vec<PathBuf>, String>, bool),
    Progress(f32, String),
    Keep(bool),
    UseSaved,
    Saved(Result<Option<String>, String>),
}

pub struct ArchiveWin {
    path: PathBuf,
    name: String,
    entries: Option<Arc<Vec<Entry>>>,
    password: Option<String>,
    /// It wants a password: none given (false), or the one given wrong.
    wants: Option<bool>,
    /// Keep the password typed in Passwords once it has worked.
    keep: bool,
    /// A typed password not yet kept (kept once it has worked).
    to_keep: Option<String>,
    field: Child<TextField>,
    dir: PathBuf,
    rows: Rc<Vec<Node>>,
    list: ListModel,
    note: Option<(String, bool)>,
    busy: Option<(f32, String)>,
    cancel: Arc<AtomicBool>,
}

pub fn options(path: &Path) -> WindowOptions {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    WindowOptions::new(name, 620.0, 440.0).min_size(420.0, 260.0)
}

impl ArchiveWin {
    pub fn new(path: PathBuf, cx: &mut Cx<Self>) -> ArchiveWin {
        let field = text_field(cx, |f| f.name("Password").password(true));
        cx.subscribe(&field, |_: &mut ArchiveWin, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                cx.send(Msg::Password);
            }
        });
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut w = ArchiveWin {
            path,
            name,
            entries: None,
            password: None,
            wants: None,
            keep: false,
            to_keep: None,
            field,
            dir: PathBuf::new(),
            rows: Rc::default(),
            list: ListModel::new(true),
            note: None,
            busy: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        w.read(cx);
        w
    }

    fn read(&mut self, cx: &mut Cx<Self>) {
        let (path, pw) = (self.path.clone(), self.password.clone());
        self.busy = Some((0.0, "Reading…".into()));
        cx.work(
            move || {
                let stop = AtomicBool::new(false);
                match olo_archive::list(&path, pw.as_deref(), &stop) {
                    Ok(e) => (Ok(Arc::new(e)), false),
                    Err(olo_archive::Error::Password(wrong)) => (Err(String::new()), wrong || pw.is_some()),
                    Err(e) => (Err(e.to_string()), false),
                }
            },
            |(r, wrong)| Msg::Listed(r, wrong),
        );
    }

    fn show(&mut self) {
        self.rows = Rc::new(self.entries.as_ref().map(|e| children(e, &self.dir)).unwrap_or_default());
        let mut st = self.list.state_mut();
        st.selected.clear();
        st.anchor = None;
    }

    fn selected(&self) -> Vec<Node> {
        self.list.state().selected.iter().filter_map(|r| self.rows.get(*r)).cloned().collect()
    }

    /// Take `nodes` out into `into` (a worker; progress here; Undo's).
    fn extract(&mut self, nodes: Vec<Node>, into: PathBuf, to_open: bool, cx: &mut Cx<Self>) {
        let names: Vec<String> = nodes.iter().map(|n| n.rel.to_string_lossy().into_owned()).collect();
        let strip = self.dir.to_string_lossy().into_owned();
        let (path, pw) = (self.path.clone(), self.password.clone());
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let progress = cx.messenger();
        self.busy = Some((0.0, if to_open { "Opening…".into() } else { "Extracting…".into() }));
        cx.work(
            move || {
                if to_open {
                    let _ = std::fs::create_dir_all(&into);
                }
                let mut said = |p: &olo_archive::Progress| {
                    let f = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                    progress.send(Msg::Progress(f, p.item.rsplit('/').next().unwrap_or_default().to_owned()));
                };
                let ctl = olo_archive::Control::new(&cancel, &mut said);
                match olo_archive::extract_some(&path, &names, &strip, &into, pw.as_deref(), &ctl) {
                    Ok(made) => (Ok(made), false),
                    Err(olo_archive::Error::Password(wrong)) => (Err(String::new()), wrong || pw.is_some()),
                    Err(e) => (Err(e.to_string()), false),
                }
            },
            move |(r, wrong)| match (to_open, r) {
                (true, Ok(made)) => {
                    // Opened with what opens its kind, off the UI thread.
                    if let Some(p) = made.into_iter().next() {
                        std::thread::spawn(move || {
                            if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Path(p.clone())) {
                                tracing::warn!("open {}: {e}", p.display());
                            }
                        });
                    }
                    Msg::Opened
                }
                (true, Err(e)) => Msg::Extracted(Err(e), wrong),
                (false, r) => Msg::Extracted(r, wrong),
            },
        );
    }

    fn open_row(&mut self, i: usize, cx: &mut Cx<Self>) {
        let Some(n) = self.rows.get(i).cloned() else { return };
        if n.is_dir {
            self.dir = n.rel;
            self.show();
            return;
        }
        // Taken out once into the cache (the archive as it is now), then opened.
        let cache = cache_for(&self.path).join(self.dir.as_os_str());
        let there = cache.join(&n.name);
        if there.exists() {
            std::thread::spawn(move || {
                let _ = olo_xdg::open::open(&olo_xdg::open::Target::Path(there));
            });
            return;
        }
        self.extract(vec![n], cache, true, cx);
    }

    /// The password typed has worked: kept in Passwords, if asked.
    fn keep_worked(&mut self) {
        if let Some(pw) = self.to_keep.take() {
            let path = self.path.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::archives::keep_password(&path, &pw) {
                    to_hub(ToHub::Said(format!("The password of “{}” couldn’t be kept: {e}", path.file_name().unwrap_or_default().to_string_lossy())));
                }
            });
        }
    }

    fn menus(&self) -> Vec<Menu> {
        let any = !self.list.state().selected.is_empty();
        vec![
            Menu::new("File")
                .app_items(&["tracker"])
                .item("Open", "open")
                .key("o")
                .enabled(any)
                .separator()
                .item("Extract Selected To…", "extract-selected")
                .enabled(any && self.busy.is_none())
                .item("Extract All", "extract-all")
                .enabled(self.entries.is_some())
                .close_quit(),
            Menu::new("Go").item("Enclosing Folder", "parent").key("up").enabled(!self.dir.as_os_str().is_empty()),
        ]
    }
}

impl View for ArchiveWin {
    type Message = Msg;

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            Msg::Listed(r, wrong) => {
                self.busy = None;
                match r {
                    Ok(e) => {
                        self.entries = Some(e);
                        self.wants = None;
                        self.note = None;
                        self.keep_worked();
                        self.show();
                    }
                    Err(e) if e.is_empty() => {
                        self.wants = Some(wrong);
                        self.field.focus(cx);
                    }
                    Err(e) => self.note = Some((e, true)),
                }
            }
            Msg::Selected => {}
            Msg::Up => {
                if let Some(p) = self.dir.parent() {
                    let back_to = self.dir.clone();
                    self.dir = p.to_owned();
                    self.show();
                    // The folder come out of, selected.
                    if let Some(i) = self.rows.iter().position(|n| n.rel == back_to) {
                        self.list.state_mut().select(i);
                    }
                }
            }
            Msg::Opened => self.busy = None,
            Msg::Password => {
                let pw = self.field.read(cx).text().to_owned();
                if pw.is_empty() {
                    return;
                }
                if self.keep {
                    self.to_keep = Some(pw.clone());
                }
                self.password = Some(pw);
                self.wants = None;
                if self.entries.is_some() {
                    self.note = Some(("Try again now.".into(), false));
                } else {
                    self.read(cx);
                }
            }
            Msg::ExtractAll => {
                to_hub(ToHub::ArchiveJob { kind: ops::Kind::Extract, sources: vec![self.path.clone()], dest: None, targets: Vec::new(), origin: None, password: self.password.clone(), keep: false });
            }
            Msg::ExtractTo(into) => {
                let nodes = self.selected();
                if !nodes.is_empty() {
                    self.extract(nodes, into, false, cx);
                }
            }
            Msg::Extracted(r, wrong) => {
                self.busy = None;
                match r {
                    Ok(made) => {
                        self.keep_worked();
                        self.note = Some((format!("{} taken out.", if made.len() == 1 { "1 item".to_owned() } else { format!("{} items", made.len()) }), false));
                        to_hub(ToHub::Done("Extract".into(), undo::Change::Made(made)));
                        to_hub(ToHub::ReloadAll);
                    }
                    Err(e) if e.is_empty() => {
                        self.wants = Some(wrong);
                        self.field.focus(cx);
                    }
                    Err(e) => self.note = Some((e, true)),
                }
            }
            Msg::Keep(on) => self.keep = on,
            Msg::UseSaved => {
                let path = self.path.clone();
                self.note = Some(("Asking Passwords…".into(), false));
                cx.work(move || crate::archives::saved_password(&path), Msg::Saved);
            }
            Msg::Saved(r) => match r {
                Ok(Some(pw)) => {
                    self.password = Some(pw);
                    self.wants = None;
                    self.note = None;
                    if self.entries.is_some() {
                        self.note = Some(("Passwords’ password is used now: try again.".into(), false));
                    } else {
                        self.read(cx);
                    }
                }
                Ok(None) => self.note = Some((format!("Passwords keeps no password for it (an item “{}”).", crate::archives::secret_title(&self.path)), true)),
                Err(e) => self.note = Some((e, true)),
            },
            Msg::Progress(f, item) => {
                if let Some(b) = &mut self.busy {
                    *b = (f, item);
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "open" => {
                let one = self.list.state().single();
                if let Some(i) = one {
                    self.open_row(i, cx);
                }
            }
            "parent" => cx.send(Msg::Up),
            "extract-all" => cx.send(Msg::ExtractAll),
            "extract-selected" => {
                let to = cx.messenger();
                let dir = self.path.parent().map(Path::to_owned).unwrap_or_default();
                file_panel::show("Extract To", FilePanel::folder(dir), move |a| {
                    if let PanelResult::Chosen(into) = a.result {
                        to.send(Msg::ExtractTo(into));
                    }
                });
            }
            "close" => {
                self.cancel.store(true, Ordering::Relaxed);
                cx.close_window();
            }
            "quit" => olo_kit::quit(),
            _ => return false,
        }
        cx.invalidate();
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = olo_kit::theme::palette();
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let mut place = vec![self.name.clone()];
        place.extend(self.dir.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
        let head = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Enclosing Folder").id("parent").enabled(!self.dir.as_os_str().is_empty()).on_click(cx.msg(Msg::Up)))
            .child(div().flex_1().min_w_0().child(StringView::new(place.join(" ▸ ")).cut(true)));
        let rows = self.rows.clone();
        let names = rows.clone();
        let columns: Rc<[Column]> = Rc::from(vec![Column::new("Name", 300.0), Column::new("Size", 80.0).right(), Column::new("Modified", 150.0)]);
        let list = ColumnListView::new(cx, "archive-list", &self.list, columns, rows.len(), move |_: &ArchiveWin, cell, g| {
            let Some(n) = rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.icon_text(g, if n.is_dir { "folder" } else { "document" }, if n.is_dir { "▸" } else { "·" }, n.name.clone()),
                1 => cell.text(g, if n.is_dir { String::new() } else { olo_kit_base::files::format_size(n.size) }),
                _ => cell.text(g, n.mtime.map(date).unwrap_or_default()),
            }
        })
        .type_select(move |_: &ArchiveWin, i| names.get(i).map(|n| n.name.clone().into()).unwrap_or_default())
        .empty_text(if self.entries.is_some() { "Nothing here." } else { "" })
        .on_select(cx.msg(Msg::Selected))
        .on_open(cx.listener(|v: &mut ArchiveWin, i: &usize, cx| {
            v.open_row(*i, cx);
            cx.invalidate();
        }));
        let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(head);
        if let Some(wrong) = self.wants {
            col = col
                .child(StringView::new(if wrong { "That password isn’t right. Try again:".to_owned() } else { format!("“{}” is protected by a password.", self.name) }).wrap(true))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().child(TextControl::new(cx, &self.field).label("Password:")))
                        .child(Button::new("Open").default(true).on_click(cx.msg(Msg::Password))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(olo_kit::widgets::CheckBox::new("Keep it in Passwords", self.keep).id("keep").on_toggle(cx.msg_with(|on: &bool| Msg::Keep(*on))))
                        .child(div().flex_1())
                        .child(Button::new("Use Saved Password").id("use-saved").on_click(cx.msg(Msg::UseSaved))),
                );
        }
        col = col.child(div().flex_1().min_h_0().child(list));
        let status: Element = match (&self.busy, &self.note) {
            (Some((f, item)), _) => div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().w(px(140.0)).child(StatusBar::new(*f).bar_height(10.0)))
                .child(StringView::new(item.clone()).dim(true).cut(true))
                .into_element(),
            (None, Some((t, failed))) => {
                let s = StringView::new(t.clone()).cut(true);
                (if *failed { s.color(olo_kit::style::c(p.failure)) } else { s.dim(true) }).into_element()
            }
            (None, None) => {
                StringView::new(self.entries.as_ref().map(|e| format!("{} items in it, {} unpacked", e.len(), olo_kit_base::files::format_size(e.iter().map(|x| x.size).sum()))).unwrap_or_default())
                    .dim(true)
                    .cut(true)
                    .into_element()
            }
        };
        let any = !self.list.state().selected.is_empty();
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(status))
                .child(Button::new("Extract Selected…").id("extract-selected").enabled(any && self.busy.is_none()).on_click(cx.listener(|v: &mut ArchiveWin, _: &ClickEvent, cx| {
                    v.command("extract-selected", cx);
                })))
                .child(Button::new("Extract All").id("extract-all").default(true).enabled(self.entries.is_some()).on_click(cx.msg(Msg::ExtractAll))),
        );
        div().size_full().flex().flex_col().bg(olo_kit::style::c(p.panel_background)).child(bar).child(col).into_element()
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// "15 Nov 2026, 14:05" (the computer's time).
fn date(t: i64) -> String {
    let st = std::time::UNIX_EPOCH + std::time::Duration::from_secs(t.max(0) as u64);
    olo_kit_base::files::format_time(Some(st))
}
