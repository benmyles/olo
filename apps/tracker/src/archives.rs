//! Compress and Extract (Ben, 2026-11-14: "olo's tracker should have
//! native support for compression/archives so that it can compress files
//! and folders (i.e. multi select them then right click and Compress ->
//! Compress as ... and e.g. Extract -> Extract to ...)"), done by
//! `olo_archive` as Tracker's jobs: on a worker, in Tracker Status with
//! Stop, what they made selected after, and undone by Edit ▸ Undo (into
//! the Trash, as a copy is).
//!
//! - Compress ▸ Zip, Tar (gzip, xz, zstd, bzip2), Tar, 7-Zip: beside the
//!   selection, "name.zip" for one item, "Archive.zip" for several (a
//!   name taken gets a number); Compress As… names it, chooses its
//!   format, and protects it with a password (zip's AES-256, 7-Zip's).
//! - Extract Here: each archive's contents beside it (one item at its
//!   top as it is, several in a folder named for it); Extract To… into a
//!   folder chosen.
//! - A password-protected archive asks for its password, and again if
//!   it's wrong.

use std::path::PathBuf;

use olo_archive::Format;
use olo_kit::{
    Child, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, ParentElement as _, Styled as _, View, WindowOptions, div,
    file_panel::{self, FilePanel, PanelResult},
    px,
    widgets::{Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent, button_row, text_field},
};
use olo_kit_base::metrics as m;

use crate::{ToHub, fs::Entry, ops, to_hub};

/// The Compress ▸ submenu, and Extract's items when everything chosen is
/// an archive (by its kind: nothing is read on the UI thread).
pub fn menu_items(menu: Menu, entries: &[Entry]) -> Menu {
    if entries.is_empty() {
        return menu;
    }
    let mut compress = Menu::new("Compress");
    for f in Format::ALL {
        compress = compress.item(f.label(), format!("compress:{}", f.extension().trim_start_matches('.')));
    }
    let mut menu = menu.submenu(compress.separator().item("Compress As…", "compress-as"));
    if entries.iter().all(|e| !e.is_dir && e.kind == olo_kit_base::files::Kind::Archive) {
        menu = menu.item("Extract Here", "extract-here").item("Extract To…", "extract-to");
    }
    menu
}

/// Run a menu's command on what's selected (`origin`: the window), or
/// say it isn't one of these.
pub fn command(command: &str, selected: Vec<PathBuf>, dir: PathBuf, origin: u64) -> bool {
    if selected.is_empty() {
        return false;
    }
    let job = |kind, dest, targets| to_hub(ToHub::ArchiveJob { kind, sources: selected.clone(), dest, targets, origin: Some(origin), password: None });
    match command {
        c if c.starts_with("compress:") => match Format::of_word(&c[9..]) {
            Some(f) => job(ops::Kind::Compress(f), None, Vec::new()),
            None => return false,
        },
        "compress-as" => CompressAs::open(selected, dir, origin),
        "extract-here" => job(ops::Kind::Extract, None, Vec::new()),
        "extract-to" => {
            let sources = selected;
            file_panel::show("Extract To", FilePanel::folder(dir), move |a| {
                if let PanelResult::Chosen(into) = a.result {
                    to_hub(ToHub::ArchiveJob { kind: ops::Kind::Extract, sources, dest: Some(into), targets: Vec::new(), origin: Some(origin), password: None });
                }
            });
        }
        _ => return false,
    }
    true
}

/// What a job needs to run again, with a password.
#[derive(Clone)]
pub struct Again {
    pub kind: ops::Kind,
    pub sources: Vec<PathBuf>,
    pub dest: Option<PathBuf>,
    pub origin: Option<u64>,
}

/// "“photos.zip” is protected by a password": a field, and Extract.
pub struct AskPassword {
    field: Child<TextField>,
    again: Again,
    wrong: bool,
}

#[derive(Clone)]
pub enum AskMsg {
    Go,
    Cancel,
}

impl AskPassword {
    pub fn open(again: Again, wrong: bool) {
        // (As tall as what it shows: `fixed` with `fits_content`.)
        let options = WindowOptions { fixed: true, ..WindowOptions::new("Password", 420.0, 80.0).dialog().fits_content() };
        olo_kit::open_window(options, move |cx| {
            let field = text_field(cx, |f| f.name("Password").password(true));
            cx.subscribe(&field, |_: &mut AskPassword, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    cx.send(AskMsg::Go);
                }
            });
            field.focus(cx);
            AskPassword { field, again, wrong }
        });
    }
}

impl View for AskPassword {
    type Message = AskMsg;

    fn message(&mut self, m: AskMsg, cx: &mut Cx<Self>) {
        if let AskMsg::Go = m {
            let pw = self.field.read(cx).text().to_owned();
            if pw.is_empty() {
                return;
            }
            let a = self.again.clone();
            to_hub(ToHub::ArchiveJob { kind: a.kind, sources: a.sources, dest: a.dest, targets: Vec::new(), origin: a.origin, password: Some(pw) });
        }
        cx.close_window();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        if command == "close" {
            cx.close_window();
            return true;
        }
        false
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = olo_kit::theme::palette();
        let name = match self.again.sources.as_slice() {
            [one] => format!("“{}”", one.file_name().unwrap_or_default().to_string_lossy()),
            many => format!("{} archives", many.len()),
        };
        let said = if self.wrong { "That password isn’t right. Try again:".to_owned() } else { format!("{name} is protected by a password.") };
        let verb = if matches!(self.again.kind, ops::Kind::Extract) { "Extract" } else { "Open" };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .bg(olo_kit::style::c(p.panel_background))
            // (Escape cancels, as every form's does.)
            .on_window_key_down(cx.listener(|_: &mut AskPassword, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    cx.close_window();
                }
            }))
            .child(StringView::new(said).wrap(true))
            .child(TextControl::new(cx, &self.field).label("Password:"))
            .child(button_row().child(Button::new("Cancel").on_click(cx.msg(AskMsg::Cancel))).child(Button::new(verb).id("extract").default(true).on_click(cx.msg(AskMsg::Go))))
            .into_element()
    }
}

/// Compress As…: the archive's name, its format, and a password if it
/// should have one.
pub struct CompressAs {
    name: Child<TextField>,
    password: Child<TextField>,
    format: usize,
    protect: bool,
    sources: Vec<PathBuf>,
    dir: PathBuf,
    origin: u64,
    note: Option<String>,
}

#[derive(Clone)]
pub enum AsMsg {
    Format(usize),
    Protect(bool),
    Go,
    Cancel,
}

/// The formats Compress As… offers, most used first.
const AS_FORMATS: [Format; 7] = Format::ALL;

impl CompressAs {
    pub fn open(sources: Vec<PathBuf>, dir: PathBuf, origin: u64) {
        let options = WindowOptions { fixed: true, ..WindowOptions::new("Compress As", 460.0, 120.0).dialog().fits_content() };
        olo_kit::open_window(options, move |cx| {
            let stem = olo_archive::archive_name(&sources, Format::Zip).trim_end_matches(".zip").to_owned();
            let name = text_field(cx, |f| f.name("Name").with_text(stem));
            let password = text_field(cx, |f| f.name("Password").password(true));
            for f in [&name, &password] {
                cx.subscribe(f, |_: &mut CompressAs, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        cx.send(AsMsg::Go);
                    }
                });
            }
            name.focus(cx);
            CompressAs { name, password, format: 0, protect: false, sources, dir, origin, note: None }
        });
    }
}

impl View for CompressAs {
    type Message = AsMsg;

    fn message(&mut self, m: AsMsg, cx: &mut Cx<Self>) {
        match m {
            AsMsg::Format(i) => self.format = i,
            AsMsg::Protect(on) => {
                self.protect = on;
                if on {
                    self.password.focus(cx);
                }
            }
            AsMsg::Cancel => cx.close_window(),
            AsMsg::Go => {
                let format = AS_FORMATS[self.format];
                let stem = self.name.read(cx).text().trim().to_owned();
                if stem.is_empty() || stem.contains('/') {
                    self.note = Some("A name, without “/”.".into());
                    cx.invalidate();
                    return;
                }
                let password = (self.protect && format.takes_password()).then(|| self.password.read(cx).text().to_owned()).filter(|p| !p.is_empty());
                if self.protect && format.takes_password() && password.is_none() {
                    self.note = Some("Type the password it’s to be opened with.".into());
                    cx.invalidate();
                    return;
                }
                let file = if stem.to_lowercase().ends_with(format.extension()) { stem } else { format!("{stem}{}", format.extension()) };
                to_hub(ToHub::ArchiveJob {
                    kind: ops::Kind::Compress(format),
                    sources: self.sources.clone(),
                    dest: Some(self.dir.clone()),
                    targets: vec![self.dir.join(file)],
                    origin: Some(self.origin),
                    password,
                });
                cx.close_window();
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        if command == "close" {
            cx.close_window();
            return true;
        }
        false
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = olo_kit::theme::palette();
        let format = AS_FORMATS[self.format];
        let labels = AS_FORMATS.iter().map(|f| f.label().into()).collect();
        let what = match self.sources.as_slice() {
            [one] => format!("“{}”", one.file_name().unwrap_or_default().to_string_lossy()),
            many => format!("{} items", many.len()),
        };
        let mut col = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .bg(olo_kit::style::c(p.panel_background))
            .on_window_key_down(cx.listener(|_: &mut CompressAs, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    cx.close_window();
                }
            }))
            .child(StringView::new(format!("Compress {what} into a new archive beside {}:", if self.sources.len() == 1 { "it" } else { "them" })).wrap(true))
            .child(TextControl::new(cx, &self.name).label("Name:").divider(72.0))
            .child(MenuField::new("compress-format", labels, self.format).label("Format:").divider(72.0).on_change(cx.msg_with(|i: &usize| AsMsg::Format(*i))))
            .child(
                CheckBox::new("Protect it with a password", self.protect && format.takes_password())
                    .id("protect")
                    .enabled(format.takes_password())
                    .on_toggle(cx.msg_with(|on: &bool| AsMsg::Protect(*on))),
            );
        if self.protect && format.takes_password() {
            col = col.child(TextControl::new(cx, &self.password).label("Password:").divider(72.0));
        }
        if !format.takes_password() {
            col = col.child(StringView::new("Only Zip and 7-Zip archives can have a password.").dim(true).small(true));
        }
        if let Some(n) = &self.note {
            col = col.child(StringView::new(n.clone()).color(olo_kit::style::c(p.failure)).wrap(true));
        }
        col.child(button_row().child(Button::new("Cancel").on_click(cx.msg(AsMsg::Cancel))).child(Button::new("Compress").id("compress").default(true).on_click(cx.msg(AsMsg::Go)))).into_element()
    }
}
