//! StyledEdit: BeOS's text editor. Plain text files with fonts, sizes,
//! colours and styles kept beside the text in attributes (doc.rs), so
//! other programs still see plain text. One window per document; Find and
//! Replace windows as in Haiku; unsaved work survives a reload.

mod doc;
mod fonts;

use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use doc::{Align, Doc, Run, Style, byte_of, char_of};
use serde::{Deserialize, Serialize};
use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{
        self, Color32, Galley, Id, Key, Modifiers, Rect, Ui, pos2,
        text::{CCursor, CCursorRange},
        vec2,
    },
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, CheckBox, ScrollView, TextControl},
};

const APP_ID: &str = "tabula.styled-edit";

/// The Font ▸ Color menu (Black is the document's text colour, so it
/// reads in dark mode too).
const COLORS: [(&str, Option<[u8; 3]>); 7] = [
    ("Black", None),
    ("Red", Some([220, 0, 0])),
    ("Green", Some([0, 150, 0])),
    ("Blue", Some([0, 0, 220])),
    ("Cyan", Some([0, 160, 180])),
    ("Magenta", Some([190, 0, 190])),
    ("Yellow", Some([210, 170, 0])),
];

/// Families per submenu of the Font menu.
const FAMILY_GROUP: usize = 24;

enum Msg {
    Fonts(fonts::Event),
    Loaded(WindowId, PathBuf, Result<doc::Loaded, String>),
    /// Saved the document as it was at generation `.1` to `.2`.
    Saved(WindowId, u64, PathBuf, Result<(), String>),
}

/// What to do once the document is saved (or its changes discarded).
#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    Nothing,
    Close,
    Revert,
}

struct DocWin {
    /// None until first saved.
    path: Option<PathBuf>,
    /// "Untitled N" for a new document.
    untitled: usize,
    doc: Doc,
    /// The live text the field edits (synced into `doc` as it changes).
    buffer: String,
    loading: bool,
    saving: bool,
    after_save: After,
    latin1: bool,
    read_only: bool,
    /// The selection, in characters, as of the last frame.
    selection: Range<usize>,
    /// A selection to set (and scroll to) next frame.
    select: Option<Range<usize>>,
    /// The field lost the keyboard (to a menu, say), which collapses its
    /// selection: put it back when it returns, as BeOS keeps it.
    blurred: bool,
}

impl DocWin {
    fn new(path: Option<PathBuf>, untitled: usize) -> Self {
        DocWin {
            path,
            untitled,
            doc: Doc::new(String::new(), None),
            buffer: String::new(),
            loading: false,
            saving: false,
            after_save: After::Nothing,
            latin1: false,
            read_only: false,
            selection: 0..0,
            select: Some(0..0),
            blurred: false,
        }
    }

    fn title(&self) -> String {
        match &self.path {
            Some(p) => file_name(p),
            None => format!("Untitled {}", self.untitled),
        }
    }

    /// The selection in bytes of the text.
    fn bytes(&self) -> Range<usize> {
        let t = self.doc.text();
        byte_of(t, self.selection.start)..byte_of(t, self.selection.end)
    }

    fn selected_text(&self) -> &str {
        &self.doc.text()[self.bytes()]
    }

    /// The doc changed under the field (undo, replace): show it.
    fn reset_buffer(&mut self, select: Range<usize>) {
        self.buffer = self.doc.text().to_owned();
        self.select = Some(select);
    }

    /// Select bytes `r` of the text.
    fn select_bytes(&mut self, r: Range<usize>) {
        let t = self.doc.text();
        self.select = Some(char_of(t, r.start)..char_of(t, r.end));
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Purpose {
    Open,
    SaveAs,
}

enum Win {
    Doc(Box<DocWin>),
    Panel { doc: Option<WindowId>, purpose: Purpose, panel: Box<FilePanel> },
    Unsaved { doc: WindowId, after: After, message: String },
    Revert { doc: WindowId, message: String },
    Error { message: String },
    Find { doc: WindowId, replace: bool },
}

/// The Find and Replace windows' settings, shared by every document.
#[derive(Default)]
struct Search {
    find: String,
    replace: String,
    case: bool,
    wrap: bool,
    backwards: bool,
    all: bool,
}

struct StyledEdit {
    windows: HashMap<WindowId, Win>,
    fonts: fonts::Fonts,
    search: Search,
    untitled: usize,
}

#[derive(Serialize, Deserialize)]
struct SavedDoc {
    path: Option<PathBuf>,
    untitled: usize,
    /// Unsaved text and styles; None when the file holds it all.
    edited: Option<(String, Vec<Run>)>,
    align: Align,
    wrap: bool,
    selection: (usize, usize),
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn sorted(r: CCursorRange) -> Range<usize> {
    let (a, b) = (r.primary.index.0, r.secondary.index.0);
    a.min(b)..a.max(b)
}

impl StyledEdit {
    fn new_doc(&mut self, cx: &mut Cx) -> WindowId {
        self.untitled += 1;
        let d = DocWin::new(None, self.untitled);
        self.add_window(cx, d)
    }

    fn add_window(&mut self, cx: &mut Cx, d: DocWin) -> WindowId {
        let id = cx.open_window(WindowOptions::new(d.title(), 560, 440).min_size(280, 180));
        self.windows.insert(id, Win::Doc(Box::new(d)));
        id
    }

    fn open_file(&mut self, cx: &mut Cx, path: PathBuf) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Doc(d) if d.path.as_ref() == Some(&path))) {
            cx.activate(id);
            return;
        }
        let mut d = DocWin::new(Some(path.clone()), 0);
        d.loading = true;
        let id = self.add_window(cx, d);
        self.load(cx, id, path);
    }

    fn load(&mut self, cx: &mut Cx, id: WindowId, path: PathBuf) {
        if let Some(Win::Doc(d)) = self.windows.get_mut(&id) {
            d.loading = true;
        }
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = doc::load(&path);
            proxy.send(Msg::Loaded(id, path, r));
        });
    }

    fn close(&mut self, cx: &mut Cx, win: WindowId) {
        self.windows.remove(&win);
        cx.close_window(win);
        let orphans: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| match w {
                Win::Unsaved { doc, .. } | Win::Revert { doc, .. } | Win::Find { doc, .. } => *doc == win,
                Win::Panel { doc: Some(doc), .. } => *doc == win,
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect();
        for o in orphans {
            self.windows.remove(&o);
            cx.close_window(o);
        }
        if !self.windows.values().any(|w| matches!(w, Win::Doc(_) | Win::Panel { .. })) {
            cx.quit();
        }
    }

    fn dialog(&mut self, cx: &mut Cx, win: Win, message: &str) {
        let id = cx.open_window(WindowOptions::alert("StyledEdit", message));
        self.windows.insert(id, win);
    }

    fn show_error(&mut self, cx: &mut Cx, message: String) {
        self.dialog(cx, Win::Error { message: message.clone() }, &message);
    }

    fn open_panel(&mut self, cx: &mut Cx, doc: Option<WindowId>, purpose: Purpose) {
        let d = doc.and_then(|d| match self.windows.get(&d) {
            Some(Win::Doc(d)) => Some(d),
            _ => None,
        });
        let path = d.and_then(|d| d.path.clone());
        let dir = path.as_deref().and_then(Path::parent).map_or_else(home, Path::to_path_buf);
        let (title, panel) = match purpose {
            Purpose::Open => ("Open", FilePanel::open(dir)),
            Purpose::SaveAs => ("Save As", FilePanel::save(dir, d.map(|d| d.title()).unwrap_or_default())),
        };
        let id = cx.open_window(WindowOptions::new(title, PANEL_SIZE.0, PANEL_SIZE.1).min_size(420, 300));
        self.windows.insert(id, Win::Panel { doc, purpose, panel: Box::new(panel) });
    }

    /// Write the document to `to` (its own file when None, or ask where).
    fn save(&mut self, cx: &mut Cx, win: WindowId, to: Option<PathBuf>, after: After) {
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        let Some(path) = to.or_else(|| d.path.clone().filter(|_| !d.read_only)) else {
            d.after_save = after;
            return self.open_panel(cx, Some(win), Purpose::SaveAs);
        };
        d.saving = true;
        d.after_save = after;
        let (text, runs, align, wrap, generation) = (d.doc.text().to_owned(), d.doc.runs().to_vec(), d.doc.align, d.doc.wrap, d.doc.generation);
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = doc::save(&path, &text, &runs, align, wrap);
            proxy.send(Msg::Saved(win, generation, path, r));
        });
    }

    /// Close or revert a document, asking about unsaved changes.
    fn leave(&mut self, cx: &mut Cx, win: WindowId, after: After) {
        let Some(Win::Doc(d)) = self.windows.get(&win) else { return };
        if !d.doc.edited() {
            return self.finish(cx, win, after);
        }
        if self.windows.values().any(|w| matches!(w, Win::Unsaved { doc, .. } if *doc == win)) {
            return;
        }
        let message = format!("Save changes to the document “{}”?", d.title());
        self.dialog(cx, Win::Unsaved { doc: win, after, message: message.clone() }, &message);
    }

    fn finish(&mut self, cx: &mut Cx, win: WindowId, after: After) {
        match after {
            After::Nothing => {}
            After::Close => self.close(cx, win),
            After::Revert => {
                if let Some(Win::Doc(d)) = self.windows.get(&win)
                    && let Some(p) = d.path.clone()
                {
                    self.load(cx, win, p);
                }
            }
        }
    }

    fn open_find(&mut self, cx: &mut Cx, doc: WindowId, replace: bool) {
        // One Find or Replace window per document.
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Find { doc: d, replace: r } if *d == doc && *r == replace)) {
            cx.activate(id);
            return;
        }
        let (title, h) = if replace { ("Replace", REPLACE_H) } else { ("Find", FIND_H) };
        let id = cx.open_window(WindowOptions::new(title, FIND_W as u32, h as u32).min_size(FIND_W as u32, h as u32).max_size(900, h as u32));
        self.windows.insert(id, Win::Find { doc, replace });
    }

    /// Find the search text from the selection; false if it isn't there.
    fn find_next(&mut self, win: WindowId) -> bool {
        let s = &self.search;
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return false };
        let sel = d.bytes();
        let from = if s.backwards { sel.start } else { sel.end };
        match doc::find(d.doc.text(), &s.find, from, s.backwards, s.case, s.wrap) {
            Some(r) => {
                d.select_bytes(r);
                true
            }
            None => false,
        }
    }

    /// Replace the next match (or all of them), selecting what went in.
    fn replace(&mut self, win: WindowId) {
        let s = &self.search;
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        if d.read_only || s.find.is_empty() {
            return;
        }
        let selection = d.selection.clone();
        if s.all {
            let ranges = doc::find_all(d.doc.text(), &s.find, s.case);
            d.doc.replace_all(&ranges, &s.replace, selection.clone());
            d.reset_buffer(selection.start..selection.start);
            return;
        }
        // The selection itself if it's a match, else the next one.
        let sel = d.bytes();
        let from = if s.backwards { sel.end } else { sel.start };
        let Some(r) = doc::find(d.doc.text(), &s.find, from, s.backwards, s.case, s.wrap) else { return };
        d.doc.replace(r.clone(), &s.replace, selection);
        let start = char_of(d.doc.text(), r.start);
        d.reset_buffer(start..start + s.replace.chars().count());
    }

    fn doc_menus(&self, d: &DocWin) -> Vec<Menu> {
        let has_sel = !d.selection.is_empty();
        let editable = !d.read_only && !d.loading;
        let file = Menu::new("File")
            .item("New", "new", Some(Key::N))
            .item("Open…", "open", Some(Key::O))
            .separator()
            .item("Save", "save", Some(Key::S))
            .enabled(!d.saving && !d.loading && (d.doc.edited() || d.path.is_none()))
            .item_shift("Save As…", "save-as", Key::S)
            .enabled(!d.saving && !d.loading)
            .item("Revert to Saved…", "revert", None)
            .enabled(d.path.is_some() && d.doc.edited() && !d.saving)
            .separator()
            .item("Close", "close", Some(Key::W))
            .item("Quit", "quit", Some(Key::Q));
        let edit = Menu::new("Edit")
            .item("Undo", "undo", Some(Key::Z))
            .enabled(editable && d.doc.can_undo())
            .item_shift("Redo", "redo", Key::Z)
            .enabled(editable && d.doc.can_redo())
            .separator()
            .item("Cut", "cut", Some(Key::X))
            .enabled(editable && has_sel)
            .item("Copy", "copy", Some(Key::C))
            .enabled(has_sel)
            .item("Paste", "paste", Some(Key::V))
            .enabled(editable)
            .item("Clear", "clear", None)
            .enabled(editable && has_sel)
            .separator()
            .item("Select All", "select-all", Some(Key::A))
            .separator()
            .item("Find…", "find", Some(Key::F))
            .item("Find Again", "find-again", Some(Key::G))
            .enabled(!self.search.find.is_empty())
            .item("Find Selection", "find-selection", Some(Key::H))
            .enabled(has_sel)
            .item("Replace…", "replace", Some(Key::R))
            .enabled(editable)
            .item("Replace Same", "replace-same", Some(Key::T))
            .enabled(editable && !self.search.find.is_empty());

        // The style where the selection starts (or what typing will use).
        let cur = d.doc.style_at(byte_of(d.doc.text(), d.selection.start));
        let mut size = Menu::new("Size");
        for s in doc::SIZES {
            size = size.item(format!("{s}"), format!("size:{s}"), None).checked(cur.size == s);
        }
        let mut color = Menu::new("Color");
        for (k, (name, rgb)) in COLORS.iter().enumerate() {
            color = color.item(*name, format!("color:{k}"), None).checked(cur.color == *rgb);
        }
        let mut font = Menu::new("Font")
            .submenu(size)
            .submenu(color)
            .separator()
            .item("Bold", "bold", Some(Key::B))
            .checked(cur.bold)
            .item("Italic", "italic", Some(Key::I))
            .checked(cur.italic)
            .item("Underline", "underline", Some(Key::U))
            .checked(cur.underline)
            .item("Strikeout", "strikeout", None)
            .checked(cur.strikeout)
            .separator()
            .item("System Font", "family:", None)
            .checked(cur.family.is_empty());
        // Families the document uses come first, then all of them in
        // alphabetical groups (menus don't scroll).
        let mut used: Vec<&str> = d.doc.runs().iter().map(|r| r.style.family.as_str()).filter(|f| !f.is_empty()).collect();
        used.sort_unstable();
        used.dedup();
        for f in &used {
            font = font.item(*f, format!("family:{f}"), None).checked(cur.family == *f);
        }
        let families = &self.fonts.families;
        if !families.is_empty() {
            font = font.separator();
            let chunks: Vec<&[String]> = families.chunks(FAMILY_GROUP).collect();
            for (chunk, title) in chunks.iter().zip(guide_words(&chunks)) {
                let mut group = Menu::new(title);
                for f in chunk.iter() {
                    group = group.item(f.as_str(), format!("family:{f}"), None).checked(cur.family == *f);
                }
                font = font.submenu(group);
            }
        }
        let align = d.doc.align;
        let document = Menu::new("Document")
            .item("Align Left", "align-left", None)
            .checked(align == Align::Left)
            .item("Align Center", "align-center", None)
            .checked(align == Align::Center)
            .item("Align Right", "align-right", None)
            .checked(align == Align::Right)
            .item("Justify", "align-justify", None)
            .checked(align == Align::Justify)
            .separator()
            .item("Wrap Lines", "wrap", None)
            .checked(d.doc.wrap);
        vec![file, edit, font, document]
    }

    fn doc_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        match command {
            "new" => {
                self.new_doc(cx);
                return;
            }
            "open" => return self.open_panel(cx, Some(win), Purpose::Open),
            "save" => return self.save(cx, win, None, After::Nothing),
            "save-as" => return self.open_panel(cx, Some(win), Purpose::SaveAs),
            "close" => return self.leave(cx, win, After::Close),
            "quit" => {
                let docs: Vec<WindowId> = self.windows.iter().filter(|(_, w)| matches!(w, Win::Doc(_))).map(|(id, _)| *id).collect();
                for d in docs {
                    self.leave(cx, d, After::Close);
                }
                return;
            }
            "revert" => {
                let Some(Win::Doc(d)) = self.windows.get(&win) else { return };
                let message = format!("Revert to the last saved version of “{}”?", d.title());
                return self.dialog(cx, Win::Revert { doc: win, message: message.clone() }, &message);
            }
            "find" => return self.open_find(cx, win, false),
            "replace" => return self.open_find(cx, win, true),
            "find-again" => {
                self.find_next(win);
                return;
            }
            "replace-same" => return self.replace(win),
            "find-selection" => {
                if let Some(Win::Doc(d)) = self.windows.get(&win) {
                    self.search.find = d.selected_text().to_owned();
                    self.find_next(win);
                }
                return;
            }
            "paste" => return cx.paste(win),
            _ => {}
        }
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        let selection = d.selection.clone();
        let bytes = d.bytes();
        let restyle = |d: &mut DocWin, f: &dyn Fn(&mut Style)| d.doc.restyle(bytes.clone(), selection.clone(), f);
        match command {
            "undo" | "redo" => {
                let to = if command == "undo" { d.doc.undo(selection) } else { d.doc.redo(selection) };
                if let Some(to) = to {
                    d.reset_buffer(to);
                }
            }
            "copy" | "cut" => {
                d.doc.copied(bytes.clone());
                cx.copy(d.selected_text().to_owned());
                if command == "cut" && !d.read_only {
                    d.doc.replace(bytes, "", selection.clone());
                    d.reset_buffer(selection.start..selection.start);
                }
            }
            "clear" if !d.read_only => {
                d.doc.replace(bytes, "", selection.clone());
                d.reset_buffer(selection.start..selection.start);
            }
            "select-all" => d.select = Some(0..d.doc.text().chars().count()),
            "bold" => {
                let on = !d.doc.style_at(bytes.start).bold;
                restyle(d, &|s| s.bold = on);
            }
            "italic" => {
                let on = !d.doc.style_at(bytes.start).italic;
                restyle(d, &|s| s.italic = on);
            }
            "underline" => {
                let on = !d.doc.style_at(bytes.start).underline;
                restyle(d, &|s| s.underline = on);
            }
            "strikeout" => {
                let on = !d.doc.style_at(bytes.start).strikeout;
                restyle(d, &|s| s.strikeout = on);
            }
            "align-left" => d.doc.align = Align::Left,
            "align-center" => d.doc.align = Align::Center,
            "align-right" => d.doc.align = Align::Right,
            "align-justify" => d.doc.align = Align::Justify,
            "wrap" => d.doc.wrap = !d.doc.wrap,
            c => {
                if let Some(size) = c.strip_prefix("size:").and_then(|s| s.parse::<f32>().ok()) {
                    restyle(d, &|s| s.size = size);
                } else if let Some(k) = c.strip_prefix("color:").and_then(|s| s.parse::<usize>().ok())
                    && let Some((_, rgb)) = COLORS.get(k)
                {
                    restyle(d, &|s| s.color = *rgb);
                } else if let Some(family) = c.strip_prefix("family:") {
                    restyle(d, &|s| s.family = family.to_owned());
                } else {
                    return;
                }
            }
        }
        if command.starts_with("align-") || command == "wrap" {
            // Layout changes are edits too (they're saved with the file).
            d.doc.generation += 1;
        }
        cx.redraw(win);
    }

    /// The document window; returns a command chosen from the text's
    /// right-click menu.
    fn doc_ui(&mut self, window: WindowId, ui: &mut Ui) -> Option<String> {
        let p = style::palette(ui.ctx());
        let Some(Win::Doc(d)) = self.windows.get_mut(&window) else { return None };
        let fonts = &mut self.fonts;
        let full = ui.max_rect();
        let status = Rect::from_min_max(pos2(full.left(), full.bottom() - STATUS_H), full.max);
        let area = Rect::from_min_max(full.min, pos2(full.right(), status.top()));

        // ⌘Z / ⌘⇧Z are our undo, not the field's.
        let editable = !d.read_only && !d.loading;
        let (redo, undo) = ui.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) | i.consume_key(Modifiers::COMMAND, Key::Y);
            (redo, i.consume_key(Modifiers::COMMAND, Key::Z))
        });
        if editable && (undo || redo) {
            let sel = d.selection.clone();
            let to = if undo { d.doc.undo(sel) } else { d.doc.redo(sel) };
            if let Some(to) = to {
                d.reset_buffer(to);
            }
        }
        // Copies from the field remember their styles, to paste them back.
        if ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut))) && !d.selection.is_empty() {
            d.doc.copied(d.bytes());
        }

        let edit_id = Id::unique(("styled-edit", window));
        if !ui.memory(|mem| mem.focused().is_some()) {
            ui.memory_mut(|mem| mem.request_focus(edit_id));
        }
        let focused = ui.memory(|mem| mem.has_focus(edit_id));
        // While a menu is open the window hasn't the keyboard, and the
        // field's selection can't be trusted.
        let active = focused && ui.input(|i| i.focused);
        let reveal = d.select.take();
        if let Some(r) = &reveal {
            d.selection = r.clone();
        }
        let restore = match (&reveal, active && d.blurred) {
            (Some(r), _) => Some(r.clone()),
            (None, true) => Some(d.selection.clone()),
            (None, false) => None,
        };
        d.blurred = !active;
        if let Some(r) = restore {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), edit_id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(r.start), CCursor::new(r.end))));
            state.store(ui.ctx(), edit_id);
        }
        let base = c(p.document_text);
        let wrap = d.doc.wrap;
        let before = d.selection.clone();
        let DocWin { doc, buffer, .. } = &mut **d;
        // The galley as laid out, before egui paints the selection into it.
        let latest: std::cell::RefCell<Option<Arc<Galley>>> = Default::default();
        let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| -> Arc<Galley> {
            let text = text.as_str();
            if editable {
                doc.sync(text, before.clone());
            }
            // egui caches laid-out paragraphs itself (and drops them
            // when the font atlas is rebuilt, as after a reconnect).
            let job = doc.layout(text, width, base, &mut |s| fonts.family(ui.ctx(), s));
            let g = ui.fonts_mut(|f| f.layout_job(job));
            *latest.borrow_mut() = Some(g.clone());
            g
        };
        // A read-only file is shown through a `&str`: selectable, not editable.
        let mut read_only: &str;
        let target: &mut dyn egui::TextBuffer = if editable {
            &mut *buffer
        } else {
            read_only = buffer.as_str();
            &mut read_only
        };
        let out = ScrollView::new(("styled-edit-scroll", window))
            .horizontal(!wrap)
            .focused(focused)
            .background(p.document_background)
            .show_in(ui, area, |ui, _| {
                let size = ui.available_size();
                // egui paints selected text in one colour; it's drawn again
                // over the highlight below, in its own colours.
                ui.visuals_mut().selection.stroke.color = Color32::TRANSPARENT;
                let out = egui::TextEdit::multiline(target)
                    .id(edit_id)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(m::TEXT_INSET as i8, 4)))
                    .desired_width(if wrap { size.x } else { f32::INFINITY })
                    .min_size(size)
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui);
                if let Some(range) = out.state.cursor.char_range()
                    && !range.is_empty()
                    && let Some(laid) = latest.borrow().clone()
                    && let Some(g) = selected_glyphs(&laid, range)
                {
                    let pos = out.galley_pos - vec2(out.galley.rect.left(), 0.0);
                    ui.painter().with_clip_rect(out.text_clip_rect).galley(pos, Arc::new(g), base);
                }
                out
            });
        if let Some(r) = out.cursor_range
            && active
        {
            d.selection = sorted(r);
        }
        // Right-click: the Edit menu's own commands, so styles go along.
        let chosen = widgets::context_menu(&out.response, || {
            let selected = !d.selection.is_empty();
            Menu::new("Edit")
                .item("Cut", "cut", Some(Key::X))
                .enabled(editable && selected)
                .item("Copy", "copy", Some(Key::C))
                .enabled(selected)
                .item("Paste", "paste", Some(Key::V))
                .enabled(editable)
                .separator()
                .item("Select All", "select-all", Some(Key::A))
        });
        if let Some(r) = reveal {
            let rect = out.galley.pos_from_cursor(CCursor::new(r.end));
            ui.scroll_to_rect(rect.translate(out.galley_pos.to_vec2()).expand(8.0), Some(egui::Align::Center));
        }

        // The status line: where the caret is and what the file is.
        let painter = ui.painter();
        painter.rect_filled(status, 0.0, c(p.panel_background));
        style::hline(painter, status.left(), status.right(), status.top(), style::shade(&p, p.panel_background, 1.3));
        let text = d.doc.text();
        let caret = byte_of(text, d.selection.end);
        let line = text[..caret].matches('\n').count() + 1;
        let col = text[..caret].rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        let mut info = format!("Line {line}, Column {col}");
        if d.loading {
            info = "Loading…".into();
        }
        if d.saving {
            info.push_str("   Saving…");
        }
        if d.read_only {
            info.push_str("   Read-only");
        }
        if d.latin1 {
            info.push_str("   Latin-1 (saved as UTF-8)");
        }
        style::label(
            painter,
            status.left() + m::TIGHT + 2.0,
            status.center().y + 1.0,
            egui::Align::Min,
            &info,
            egui::FontId::proportional(style::SMALL_SIZE),
            c(p.panel_text),
        );
        chosen
    }

    fn find_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui, doc: WindowId, replace: bool) {
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let s = &mut self.search;
        let mut go = false;
        let mut cancel = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            let divider = if replace { 86.0 } else { 40.0 };
            ui.add(TextControl::new(&mut s.find).label("Find:").divider(divider).autofocus(true));
            if replace {
                ui.add(TextControl::new(&mut s.replace).label("Replace with:").divider(divider));
            }
            ui.add_space(m::SPACING - m::TIGHT);
            ui.add(CheckBox::new(&mut s.case, "Case-sensitive"));
            ui.add(CheckBox::new(&mut s.wrap, "Wrap-around search"));
            ui.add(CheckBox::new(&mut s.backwards, "Search backwards"));
            if replace {
                ui.add(CheckBox::new(&mut s.all, "Replace all occurrences"));
            }
        });
        let bw = m::BUTTON_MIN_WIDTH;
        let y = full.bottom() - m::CONTROL_HEIGHT;
        let go_r = Rect::from_min_size(pos2(full.right() - bw, y), vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = go_r.translate(vec2(-(bw + m::SPACING + 2.0), 0.0));
        if ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            cancel = true;
        }
        let label = match (replace, s.all) {
            (false, _) => "Find",
            (true, false) => "Replace",
            (true, true) => "Replace All",
        };
        let ready = !s.find.is_empty();
        if ui.add_enabled_ui(ready, |ui| ui.put(go_r, Button::new(label).default(true))).inner.clicked() || ready && ui.input(|i| i.key_pressed(Key::Enter)) {
            go = true;
        }
        if go {
            if replace {
                self.replace(doc);
            } else {
                self.find_next(doc);
            }
            cx.redraw(doc);
        }
        if go || cancel {
            self.windows.remove(&window);
            cx.close_window(window);
            if go {
                cx.activate(doc);
            }
        }
    }

    fn handle_args(&mut self, cx: &mut Cx, args: &[String], cwd: Option<&Path>) {
        let mut opened = false;
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(cx, p);
            opened = true;
        }
        if !opened {
            match self.windows.iter().find(|(_, w)| matches!(w, Win::Doc(_))) {
                Some((&id, _)) => cx.activate(id),
                None => {
                    self.new_doc(cx);
                }
            }
        }
    }
}

/// Just the selected glyphs of `galley` (its selected rows, with
/// everything else in them transparent), to draw over the selection.
fn selected_glyphs(galley: &Galley, range: CCursorRange) -> Option<Galley> {
    let [min, max] = range.sorted_cursors();
    let (min, max) = (galley.layout_from_cursor(min), galley.layout_from_cursor(max));
    let mut g = galley.clone();
    g.rows = galley.rows.get(min.row..=max.row)?.to_vec();
    for (k, placed) in g.rows.iter_mut().enumerate() {
        let ri = min.row + k;
        let row = Arc::make_mut(&mut placed.row);
        let vertex = |i: usize| row.glyphs.get(i).map_or(row.visuals.glyph_vertex_range.end, |g| g.first_vertex as usize);
        let first = if ri == min.row { vertex(min.column.0) } else { vertex(0) };
        let last = if ri == max.row { vertex(max.column.0) } else { row.visuals.glyph_vertex_range.end };
        for (vi, v) in row.visuals.mesh.vertices.iter_mut().enumerate() {
            if !(first..last).contains(&vi) {
                v.color = Color32::TRANSPARENT;
            }
        }
    }
    Some(g)
}

/// Titles for alphabetical groups of names, like a dictionary's guide
/// words: each end is cut to the shortest prefix that tells it from the
/// neighbouring group ("Jo – Noto Sans Ar").
fn guide_words(chunks: &[&[String]]) -> Vec<String> {
    fn lcp(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x.eq_ignore_ascii_case(y)).count()
    }
    fn cut(name: &str, other: Option<&str>) -> String {
        let n = match other {
            Some(o) => lcp(name, o) + 1,
            // The outer ends just need their first word.
            None => name.split(' ').next().map_or(0, |w| w.chars().count()),
        };
        name.chars().take(n.max(1)).collect::<String>().trim_end().to_owned()
    }
    (0..chunks.len())
        .map(|i| {
            let first = chunks[i].first().map(String::as_str).unwrap_or_default();
            let last = chunks[i].last().map(String::as_str).unwrap_or_default();
            let prev = i.checked_sub(1).and_then(|j| chunks[j].last()).map(String::as_str);
            let next = chunks.get(i + 1).and_then(|c| c.first()).map(String::as_str);
            format!("{} – {}", cut(first, prev), cut(last, next))
        })
        .collect()
}

const STATUS_H: f32 = widgets::SCROLL_BAR + 2.0;
const FIND_W: f32 = 340.0;
const FIND_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + 3.0 * m::MARK_ROW + 2.0 * m::TIGHT + m::SPACING + m::CONTROL_HEIGHT;
const REPLACE_H: f32 = FIND_H + m::CONTROL_HEIGHT + m::TIGHT + m::MARK_ROW + m::TIGHT;

impl App for StyledEdit {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        match self.windows.get_mut(&window) {
            Some(Win::Doc(_)) => {
                if let Some(command) = self.doc_ui(window, ui) {
                    self.doc_command(cx, window, &command);
                }
            }
            Some(Win::Find { doc, replace }) => {
                let (doc, replace) = (*doc, *replace);
                self.find_ui(cx, window, ui, doc, replace);
            }
            Some(Win::Panel { doc, purpose, panel }) => {
                let (doc, purpose) = (*doc, *purpose);
                let Some(result) = panel.ui(ui) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                let PanelResult::Chosen(path) = result else {
                    // Cancelling a Save As drops what was to follow it.
                    if let Some(Win::Doc(d)) = doc.and_then(|d| self.windows.get_mut(&d)) {
                        d.after_save = After::Nothing;
                    }
                    if !self.windows.values().any(|w| matches!(w, Win::Doc(_) | Win::Panel { .. })) {
                        cx.quit();
                    }
                    return;
                };
                match (purpose, doc) {
                    (Purpose::Open, _) => self.open_file(cx, path),
                    (Purpose::SaveAs, Some(doc)) => {
                        let after = match self.windows.get(&doc) {
                            Some(Win::Doc(d)) => d.after_save,
                            _ => After::Nothing,
                        };
                        self.save(cx, doc, Some(path), after);
                    }
                    (Purpose::SaveAs, None) => {}
                }
            }
            Some(Win::Unsaved { doc, after, message }) => {
                let (doc, after, message) = (*doc, *after, message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Don't save", "Save"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                match choice {
                    1 => self.finish(cx, doc, after),
                    2 => self.save(cx, doc, None, after),
                    _ => {}
                }
            }
            Some(Win::Revert { doc, message }) => {
                let (doc, message) = (*doc, message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Revert"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.finish(cx, doc, After::Revert);
                }
            }
            Some(Win::Error { message }) => {
                let message = message.clone();
                if widgets::alert(ui, "warning", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                    if !self.windows.values().any(|w| matches!(w, Win::Doc(_) | Win::Panel { .. })) {
                        cx.quit();
                    }
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        match self.windows.get(&window) {
            Some(Win::Doc(d)) => Some(self.doc_menus(d)),
            _ => None,
        }
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        if matches!(self.windows.get(&window), Some(Win::Doc(_))) {
            self.doc_command(cx, window, command);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        match self.windows.get(&window) {
            Some(Win::Doc(_)) => self.leave(cx, window, After::Close),
            Some(Win::Panel { .. }) | Some(Win::Error { .. }) => self.close(cx, window),
            _ => {
                self.windows.remove(&window);
                cx.close_window(window);
            }
        }
    }

    fn files_dropped(&mut self, cx: &mut Cx, _window: WindowId, paths: Vec<PathBuf>) {
        for p in paths {
            self.open_file(cx, p);
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(cx, &launch.args, launch.cwd.as_deref()),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Fonts(fonts::Event::Families(f)) => self.fonts.families = f,
            Msg::Fonts(fonts::Event::Loaded) => cx.redraw_all(),
            Msg::Loaded(win, path, result) => {
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                if d.path.as_ref() != Some(&path) {
                    return;
                }
                d.loading = false;
                match result {
                    Ok(l) => {
                        let mut doc = Doc::new(l.text, l.runs);
                        doc.align = l.align;
                        doc.wrap = l.wrap;
                        d.doc = doc;
                        d.latin1 = l.latin1;
                        d.read_only = l.read_only;
                        d.reset_buffer(0..0);
                    }
                    // A new file: it's made on the first save.
                    Err(_) if !path.exists() => {}
                    Err(e) => {
                        let message = format!("“{}” couldn't be opened. {e}", file_name(&path));
                        self.close(cx, win);
                        self.show_error(cx, message);
                        return;
                    }
                }
                cx.redraw(win);
            }
            Msg::Saved(win, generation, path, result) => {
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                d.saving = false;
                let after = std::mem::replace(&mut d.after_save, After::Nothing);
                match result {
                    Ok(()) => {
                        if d.path.as_ref() != Some(&path) {
                            d.path = Some(path);
                            d.read_only = false;
                            cx.set_title(win, d.title());
                        }
                        d.latin1 = false;
                        d.doc.saved_generation = generation;
                        self.finish(cx, win, after);
                    }
                    Err(e) => {
                        let message = format!("“{}” couldn't be saved. {e}", file_name(&path));
                        self.show_error(cx, message);
                    }
                }
                cx.redraw(win);
            }
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let docs: Vec<SavedDoc> = self
            .windows
            .values()
            .filter_map(|w| match w {
                Win::Doc(d) => Some(SavedDoc {
                    path: d.path.clone(),
                    untitled: d.untitled,
                    edited: (d.doc.edited() || d.path.is_none()).then(|| (d.doc.text().to_owned(), d.doc.runs().to_vec())),
                    align: d.doc.align,
                    wrap: d.doc.wrap,
                    selection: (d.selection.start, d.selection.end),
                }),
                _ => None,
            })
            .collect();
        serde_json::to_string(&docs).ok()
    }
}

impl StyledEdit {
    fn restore(&mut self, cx: &mut Cx, docs: Vec<SavedDoc>) {
        for s in docs {
            self.untitled = self.untitled.max(s.untitled);
            let mut d = DocWin::new(s.path.clone(), s.untitled);
            match s.edited {
                Some((text, runs)) => {
                    let mut doc = Doc::new(text, Some(runs));
                    doc.align = s.align;
                    doc.wrap = s.wrap;
                    // Unsaved work stays unsaved (an empty new one is clean).
                    if !(doc.text().is_empty() && s.path.is_none()) {
                        doc.saved_generation = 0;
                    }
                    d.doc = doc;
                    d.reset_buffer(s.selection.0..s.selection.1);
                    self.add_window(cx, d);
                }
                None => {
                    let Some(path) = s.path else { continue };
                    d.loading = true;
                    let id = self.add_window(cx, d);
                    self.load(cx, id, path);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        let fonts = fonts::Fonts::start(move |e| proxy.send(Msg::Fonts(e)));
        let mut app = StyledEdit { windows: HashMap::new(), fonts, search: Search { wrap: true, ..Search::default() }, untitled: 0 };
        match cx.restored_state().and_then(|s| serde_json::from_str::<Vec<SavedDoc>>(&s).ok()) {
            Some(docs) if !docs.is_empty() => app.restore(cx, docs),
            _ => app.handle_args(cx, &args, None),
        }
        app
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn guide_words() {
        let names: Vec<String> = ["Adwaita Sans", "Cantarell", "Jomolhari", "Noto Sans Arabic", "Noto Sans Bengali", "Z003"].map(String::from).to_vec();
        let chunks: Vec<&[String]> = names.chunks(2).collect();
        assert_eq!(super::guide_words(&chunks), ["Adwaita – C", "J – Noto Sans A", "Noto Sans B – Z003"]);
    }
}
