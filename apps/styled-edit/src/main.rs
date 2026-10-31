//! StyledEdit: BeOS's text editor. Plain text files with fonts, sizes,
//! colours and styles kept beside the text in attributes (doc.rs), so
//! other programs still see plain text. One window per document; Find and
//! Replace windows as in Haiku; unsaved work survives a reload.

mod convert;
mod doc;
mod fonts;

use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};

use doc::{Align, Change, Doc, Run, Style};
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    editor::{self, Editor},
    egui::{self, Color32, Key, Rect, Ui, pos2, text::LayoutJob, vec2},
    file_panel::{FilePanel, Filter, PANEL_SIZE, PanelResult},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, CheckBox, TextControl},
};

const APP_ID: &str = "olo.styled-edit";

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
    /// Open documents' files changed on disk (another program wrote one).
    OnDisk(Vec<PathBuf>),
    /// A formatted document read in: its text, runs and paragraphs.
    Imported(PathBuf, Result<(String, Vec<Run>, Vec<doc::Para>), String>),
    /// A copy written in another format.
    Exported(PathBuf, Result<(), String>),
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
    /// Shows and edits `doc` (a paragraph at a time: documents of any
    /// length stay quick).
    editor: Editor,
    loading: bool,
    saving: bool,
    after_save: After,
    latin1: bool,
    read_only: bool,
    /// The selection and view to come back to once loaded (after a
    /// reload).
    restore: Option<(Range<usize>, (usize, f32))>,
    /// The file as last loaded or saved here (when it was written, how
    /// long), to tell another program's change from our own save.
    on_disk: Option<(std::time::SystemTime, u64)>,
    /// The misspelled word right-clicked, and what it might be.
    spell: Option<(Range<usize>, Vec<String>)>,
    /// Words counted for the status line, and for which text and
    /// selection.
    words: Option<((u64, Range<usize>), Words)>,
}

/// Words in the selection (if any) and in the whole document.
type Words = (Option<usize>, usize);

/// The document as the editor sees it: its text, styles in the fonts
/// they use, and edits that keep both (and undo) in step.
struct Styled<'a> {
    doc: &'a mut Doc,
    fonts: &'a mut fonts::Fonts,
    ctx: &'a egui::Context,
    base: Color32,
}

impl editor::Document for Styled<'_> {
    fn text(&self) -> &str {
        self.doc.text()
    }
    fn sections(&mut self, range: Range<usize>, job: &mut LayoutJob) {
        let (fonts, ctx) = (&mut *self.fonts, self.ctx);
        self.doc.sections(range, self.base, &mut |s| fonts.family(ctx, s), job);
    }
    fn paragraph(&mut self, range: Range<usize>) -> editor::Paragraph {
        self.doc.format(self.doc.para_index(range.start), self.doc.align)
    }
    fn replace(&mut self, range: Range<usize>, text: &str, selection: Range<usize>) {
        self.doc.edit(range, text, selection);
    }
    fn copied(&mut self, range: Range<usize>) {
        self.doc.copied(range);
    }
    fn move_text(&mut self, from: Range<usize>, to: usize, selection: Range<usize>) -> Range<usize> {
        self.doc.move_text(from, to, selection)
    }
}

impl DocWin {
    fn new(path: Option<PathBuf>, untitled: usize) -> Self {
        DocWin {
            path,
            untitled,
            doc: Doc::new(String::new(), None),
            editor: Editor::new(("styled-edit", EDITORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed))),
            loading: false,
            saving: false,
            after_save: After::Nothing,
            latin1: false,
            read_only: false,
            restore: None,
            on_disk: None,
            spell: None,
            words: None,
        }
    }

    /// The words in the selection (if any) and in all of it, counted again
    /// when either changes; None for a document too long to count on the
    /// way (a millisecond's worth).
    fn word_count(&mut self) -> Option<Words> {
        const MOST: usize = 1 << 20;
        if self.doc.text().len() > MOST {
            return None;
        }
        let sel = self.bytes();
        let key = (self.doc.generation, sel.clone());
        if let Some((k, w)) = &self.words
            && *k == key
        {
            return Some(*w);
        }
        let count = |t: &str| t.split_whitespace().count();
        let text = self.doc.text();
        let w = ((!sel.is_empty()).then(|| count(&text[sel.clone()])), count(text));
        self.words = Some((key, w));
        Some(w)
    }

    /// Keys a list item takes before the editor: Return in an empty one
    /// ends the list (or steps out a level), Tab and ⇧Tab indent it more
    /// or less, Backspace at its start takes its marker away. Whether one
    /// was taken.
    fn list_keys(&mut self, ui: &Ui) -> bool {
        let sel = self.bytes();
        let i = self.doc.para_index(sel.start);
        let para = self.doc.paras()[i].clone();
        if para.list == doc::List::None {
            return false;
        }
        let range = self.doc.para_range(i);
        let empty = sel.is_empty() && range.is_empty();
        let at_start = sel.is_empty() && sel.start == range.start;
        let (enter, tab, back_tab, backspace) = ui.input_mut(|k| {
            (
                empty && k.consume_key(egui::Modifiers::NONE, Key::Enter),
                k.consume_key(egui::Modifiers::NONE, Key::Tab),
                k.consume_key(egui::Modifiers::SHIFT, Key::Tab),
                at_start && !empty && k.consume_key(egui::Modifiers::NONE, Key::Backspace),
            )
        });
        if enter {
            let out = para.level == 0;
            self.doc.set_paras(sel.clone(), sel, |p| if out { p.list = doc::List::None } else { p.level -= 1 });
        } else if tab {
            self.doc.set_paras(sel.clone(), sel, |p| p.level = (p.level + 1).min(doc::MAX_LEVEL));
        } else if back_tab {
            self.doc.set_paras(sel.clone(), sel, |p| p.level = p.level.saturating_sub(1));
        } else if backspace {
            self.doc.set_paras(sel.clone(), sel, |p| p.list = doc::List::None);
        }
        enter || tab || back_tab || backspace
    }

    fn title(&self) -> String {
        match &self.path {
            Some(p) => file_name(p),
            None => format!("Untitled {}", self.untitled),
        }
    }

    /// The selection (bytes of the text).
    fn bytes(&self) -> Range<usize> {
        self.editor.selection()
    }

    fn selected_text(&self) -> &str {
        &self.doc.text()[self.bytes()]
    }

    /// The document changed under the editor (undo, Replace): show it,
    /// selecting `select`.
    fn changed(&mut self, change: Change, select: Range<usize>) {
        self.editor.replaced(self.doc.text(), change.removed, change.inserted);
        self.editor.select(self.doc.text(), select);
    }

    /// A new document (loaded, restored), selecting `select`.
    fn reset(&mut self, doc: Doc, select: Range<usize>) {
        self.doc = doc;
        self.editor.reset(self.doc.text());
        self.editor.select(self.doc.text(), select);
    }

    /// Select bytes `r` of the text.
    fn select_bytes(&mut self, r: Range<usize>) {
        self.editor.select(self.doc.text(), r);
    }
}

/// Tells documents' editors apart.
static EDITORS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Purpose {
    Open,
    SaveAs,
    /// A formatted document (Markdown) in as a new one.
    Import,
    /// A copy in another format.
    Export(Format),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Format {
    Markdown,
    Html,
    Word,
    OpenDocument,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Word => "docx",
            Format::OpenDocument => "odt",
        }
    }
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
    /// The open documents' files, followed as they change
    /// (`olo_kit::watch`): one changed by another program is read again
    /// if it has no unsaved changes here.
    watch: Option<olo_kit::watch::Watch>,
    /// Misspelled words are underlined (Edit ▸ Check Spelling While
    /// Typing).
    spelling: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedDoc {
    path: Option<PathBuf>,
    untitled: usize,
    /// Unsaved text and styles; None when the file holds it all.
    edited: Option<(String, Vec<Run>)>,
    /// The unsaved text's paragraphs.
    #[serde(default)]
    paras: Vec<doc::Para>,
    align: Align,
    wrap: bool,
    selection: (usize, usize),
    /// The editor's view ([`Editor::view`]).
    #[serde(default)]
    view: (usize, f32),
}

/// When a file was last written and how long it is.
fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    std::fs::metadata(path).ok().map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

impl StyledEdit {
    fn new_doc(&mut self, cx: &mut Cx) -> WindowId {
        self.untitled += 1;
        let d = DocWin::new(None, self.untitled);
        self.add_window(cx, d)
    }

    fn add_window(&mut self, cx: &mut Cx, d: DocWin) -> WindowId {
        let id = cx.open_window(WindowOptions::new(d.title(), 560, 440).min_size(280, 180).path(d.path.clone()));
        self.windows.insert(id, Win::Doc(Box::new(d)));
        id
    }

    /// Follow the open documents' files (as they are now).
    fn follow_files(&mut self, cx: &mut Cx) {
        let paths: Vec<PathBuf> = self.windows.values().filter_map(|w| if let Win::Doc(d) = w { d.path.clone() } else { None }).collect();
        match &self.watch {
            Some(w) => w.set(paths),
            None => {
                let proxy = cx.proxy();
                self.watch = olo_kit::watch::watch(paths, move |changed| proxy.send(Msg::OnDisk(changed.to_vec())));
            }
        }
    }

    fn open_file(&mut self, cx: &mut Cx, path: PathBuf) {
        // Word and OpenDocument aren't text: in as a new document.
        if olo_richtext::format_of(&path).is_some() {
            return self.import(cx, path);
        }
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
        self.follow_files(cx);
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
            Purpose::Import => (
                "Import",
                FilePanel::open(dir).filters(
                    vec![
                        Filter::new("All Documents").glob("*.md").glob("*.markdown").glob("*.mdown").glob("*.docx").glob("*.odt").mime("text/markdown"),
                        Filter::new("Markdown").glob("*.md").glob("*.markdown").glob("*.mdown").mime("text/markdown"),
                        Filter::new("Word").glob("*.docx"),
                        Filter::new("OpenDocument Text").glob("*.odt"),
                    ],
                    0,
                ),
            ),
            Purpose::Export(f) => {
                // The document's name, in the format's extension.
                let title = d.map(|d| d.title()).unwrap_or_default();
                let stem = Path::new(&title).file_stem().map_or(title.clone(), |s| s.to_string_lossy().into_owned());
                ("Export", FilePanel::save(dir, format!("{stem}.{}", f.extension())))
            }
        };
        let id = cx.open_window(WindowOptions::new(title, PANEL_SIZE.0, PANEL_SIZE.1).min_size(420, 300));
        self.windows.insert(id, Win::Panel { doc, purpose, panel: Box::new(panel) });
    }

    /// A formatted document read in (on a worker) as a new, untitled one:
    /// saving it writes a StyledEdit file, never over what it came from.
    fn import(&mut self, cx: &mut Cx, path: PathBuf) {
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = match olo_richtext::format_of(&path) {
                Some(_) => olo_richtext::read(&path).map(convert::from_rich),
                None => std::fs::read(&path).map_err(|e| e.to_string()).map(|bytes| convert::from_markdown(&String::from_utf8_lossy(&bytes))),
            };
            proxy.send(Msg::Imported(path, r));
        });
    }

    /// A copy of the document in `format` at `to` (on a worker); the
    /// document stays as it is, and where.
    fn export(&mut self, cx: &mut Cx, win: WindowId, to: PathBuf, format: Format) {
        let Some(Win::Doc(d)) = self.windows.get(&win) else { return };
        let (text, runs, paras, align, title) = (d.doc.text().to_owned(), d.doc.runs().to_vec(), d.doc.paras().to_vec(), d.doc.align, d.title());
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let out = match format {
                Format::Markdown => Ok(convert::to_markdown(&text, &runs, &paras).into_bytes()),
                Format::Html => Ok(convert::to_html(&text, &runs, &paras, &title, align).into_bytes()),
                Format::Word => olo_richtext::write_docx(&convert::to_rich(&text, &runs, &paras, align)),
                Format::OpenDocument => olo_richtext::write_odt(&convert::to_rich(&text, &runs, &paras, align)),
            };
            let r = out.and_then(|bytes| std::fs::write(&to, bytes).map_err(|e| e.to_string()));
            proxy.send(Msg::Exported(to, r));
        });
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
        let (text, runs, paras, align, wrap, generation) =
            (d.doc.text().to_owned(), d.doc.runs().to_vec(), d.doc.paras().to_vec(), d.doc.align, d.doc.wrap, d.doc.generation);
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = doc::save(&path, &text, &runs, &paras, align, wrap);
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
        let selection = d.bytes();
        if s.all {
            let ranges = doc::find_all(d.doc.text(), &s.find, s.case);
            if let Some(change) = d.doc.replace_all(&ranges, &s.replace, selection.clone()) {
                let at = selection.start.min(d.doc.text().len());
                d.changed(change, at..at);
            }
            return;
        }
        // The selection itself if it's a match, else the next one.
        let from = if s.backwards { selection.end } else { selection.start };
        let Some(r) = doc::find(d.doc.text(), &s.find, from, s.backwards, s.case, s.wrap) else { return };
        let change = d.doc.replace(r.clone(), &s.replace, selection);
        d.changed(change, r.start..r.start + s.replace.len());
    }

    fn doc_menus(&self, d: &DocWin) -> Vec<Menu> {
        let has_sel = !d.bytes().is_empty();
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
            .item("Import…", "import", None)
            .submenu(
                Menu::new("Export")
                    .item("Word Document…", "export:docx", None)
                    .item("OpenDocument Text…", "export:odt", None)
                    .separator()
                    .item("Markdown…", "export:md", None)
                    .item("Web Page (HTML)…", "export:html", None),
            )
            .separator()
            .item(olo_kit::tracker::LABEL, olo_kit::tracker::COMMAND, None)
            .enabled(d.path.is_some())
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
            .enabled(editable && !self.search.find.is_empty())
            .separator()
            .item("Check Spelling While Typing", "spelling", None)
            .checked(self.spelling);

        // The style where the selection starts (or what typing will use).
        let cur = d.doc.style_at(d.bytes().start);
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
        // The paragraph the selection starts in.
        let para = d.doc.paras()[d.doc.para_index(d.bytes().start)].clone();
        let align = para.align.unwrap_or(d.doc.align);
        let mut kinds = Menu::new("Style");
        for (k, label, id) in doc::Kind::ALL {
            kinds = kinds.item(label, format!("kind:{id}"), None).checked(para.kind == k);
        }
        let paragraph = Menu::new("Paragraph")
            .submenu(kinds)
            .separator()
            .item("Bulleted List", "list-bullet", None)
            .checked(para.list == doc::List::Bullet)
            .item("Numbered List", "list-number", None)
            .checked(para.list == doc::List::Number)
            .separator()
            .item("Indent More", "indent-more", None)
            .enabled(para.level < doc::MAX_LEVEL)
            .item("Indent Less", "indent-less", None)
            .enabled(para.level > 0)
            .separator()
            .item("Align Left", "align-left", None)
            .checked(align == Align::Left)
            .item("Align Center", "align-center", None)
            .checked(align == Align::Center)
            .item("Align Right", "align-right", None)
            .checked(align == Align::Right)
            .item("Justify", "align-justify", None)
            .checked(align == Align::Justify);
        let document = Menu::new("Document").item("Wrap Lines", "wrap", None).checked(d.doc.wrap);
        // Styles and layout are saved with the file, so a read-only one
        // keeps its own (as Haiku's StyledEdit has it).
        vec![file, edit, font.all_enabled(editable), paragraph.all_enabled(editable), document.all_enabled(editable)]
    }

    fn doc_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        match command {
            "new" => {
                self.new_doc(cx);
                return;
            }
            "open" => return self.open_panel(cx, Some(win), Purpose::Open),
            "import" => return self.open_panel(cx, Some(win), Purpose::Import),
            "export:md" => return self.open_panel(cx, Some(win), Purpose::Export(Format::Markdown)),
            "export:html" => return self.open_panel(cx, Some(win), Purpose::Export(Format::Html)),
            "export:docx" => return self.open_panel(cx, Some(win), Purpose::Export(Format::Word)),
            "export:odt" => return self.open_panel(cx, Some(win), Purpose::Export(Format::OpenDocument)),
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
            olo_kit::tracker::COMMAND => {
                if let Some(Win::Doc(d)) = self.windows.get(&win)
                    && let Some(path) = &d.path
                    && let Err(e) = olo_kit::tracker::open(path)
                {
                    self.show_error(cx, format!("“{}” couldn't be shown in Tracker. {e}", file_name(path)));
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
            "spelling" => {
                self.spelling = !self.spelling;
                for w in self.windows.keys().copied().collect::<Vec<_>>() {
                    cx.redraw(w);
                }
                return;
            }
            "spell:learn" | "spell:ignore" => {
                let checker = olo_kit::spell::loaded();
                if let (Some(c), Some(Win::Doc(d))) = (checker, self.windows.get(&win))
                    && let Some((r, _)) = &d.spell
                {
                    let word = d.doc.text()[r.clone()].to_owned();
                    if command == "spell:learn" { c.learn(&word) } else { c.ignore(&word) }
                }
                for (&w, win) in self.windows.iter_mut() {
                    if let Win::Doc(d) = win {
                        d.editor.respell();
                        cx.redraw(w);
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
        let selection = d.bytes();
        let bytes = selection.clone();
        let restyle = |d: &mut DocWin, f: &dyn Fn(&mut Style)| {
            d.doc.restyle(bytes.clone(), selection.clone(), f);
            d.editor.restyled(bytes.clone());
        };
        match command {
            "undo" | "redo" => {
                let to = if command == "undo" { d.doc.undo(selection) } else { d.doc.redo(selection) };
                if let Some((to, change)) = to {
                    d.changed(change, to);
                }
            }
            "copy" | "cut" => {
                d.doc.copied(bytes.clone());
                cx.copy(d.selected_text().to_owned());
                if command == "cut" && !d.read_only {
                    let change = d.doc.replace(bytes, "", selection.clone());
                    d.changed(change, selection.start..selection.start);
                }
            }
            "clear" if !d.read_only => {
                let change = d.doc.replace(bytes, "", selection.clone());
                d.changed(change, selection.start..selection.start);
            }
            "select-all" => {
                let all = 0..d.doc.text().len();
                d.editor.select(d.doc.text(), all);
            }
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
            "align-left" | "align-center" | "align-right" | "align-justify" => {
                let a = match command {
                    "align-center" => Align::Center,
                    "align-right" => Align::Right,
                    "align-justify" => Align::Justify,
                    _ => Align::Left,
                };
                let base = d.doc.align;
                d.doc.set_paras(bytes, selection, |p| p.align = (a != base).then_some(a));
            }
            "list-bullet" | "list-number" => {
                let list = if command == "list-bullet" { doc::List::Bullet } else { doc::List::Number };
                // On if the first paragraph isn't in this kind of list yet.
                let on = d.doc.paras()[d.doc.para_index(bytes.start)].list != list;
                d.doc.set_paras(bytes, selection, |p| p.list = if on { list } else { doc::List::None });
            }
            c if c.starts_with("spell:") => {
                // A guess for the misspelled word, in its place.
                let guess = c.strip_prefix("spell:").and_then(|k| k.parse::<usize>().ok());
                if let (Some(k), Some((r, guesses))) = (guess, d.spell.take())
                    && let Some(g) = guesses.get(k)
                    && r.end <= d.doc.text().len()
                {
                    let change = d.doc.replace(r.clone(), g, selection);
                    d.changed(change, r.start + g.len()..r.start + g.len());
                }
            }
            "indent-more" => d.doc.set_paras(bytes, selection, |p| p.level = (p.level + 1).min(doc::MAX_LEVEL)),
            "indent-less" => d.doc.set_paras(bytes, selection, |p| p.level = p.level.saturating_sub(1)),
            "wrap" => d.doc.wrap = !d.doc.wrap,
            c if c.starts_with("kind:") => {
                let Some((kind, ..)) = doc::Kind::ALL.into_iter().find(|(_, _, id)| Some(*id) == c.strip_prefix("kind:")) else { return };
                let r = d.doc.paras_in(bytes.clone());
                let whole = d.doc.para_range(r.start).start..d.doc.para_range(r.end - 1).end;
                d.doc.set_kind(bytes, selection, kind);
                d.editor.restyled(whole);
            }
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
        if command == "wrap" {
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
        let full = ui.max_rect();
        let status = Rect::from_min_max(pos2(full.left(), full.bottom() - STATUS_H), full.max);
        let area = Rect::from_min_max(full.min, pos2(full.right(), status.top()));

        // Undo and Redo (⌘Z / ⇧⌘Z on a Mac, Ctrl+Z / Ctrl+Y on Windows)
        // are ours, not the field's.
        let editable = !d.read_only && !d.loading;
        let (redo, undo) = ui.input_mut(|i| {
            let redo = olo_kit::keyboard::consume(i, "redo");
            (redo, olo_kit::keyboard::consume(i, "undo"))
        });
        if editable && (undo || redo) {
            let sel = d.bytes();
            let to = if undo { d.doc.undo(sel) } else { d.doc.redo(sel) };
            if let Some((to, change)) = to {
                d.changed(change, to);
            }
        }
        if !ui.memory(|mem| mem.focused().is_some()) {
            ui.memory_mut(|mem| mem.request_focus(d.editor.id()));
        }
        if editable && ui.memory(|mem| mem.has_focus(d.editor.id())) && d.list_keys(ui) {
            ui.ctx().request_repaint();
        }
        let options = editor::Options {
            editable,
            wrap: d.doc.wrap,
            align: match d.doc.align {
                Align::Left | Align::Justify => egui::Align::Min,
                Align::Center => egui::Align::Center,
                Align::Right => egui::Align::Max,
            },
            justify: d.doc.align == Align::Justify,
            margin: vec2(m::TEXT_INSET, 4.0),
            background: Some(p.document_background),
            spelling: self.spelling,
        };
        let DocWin { doc, editor, .. } = &mut **d;
        let mut styled = Styled { doc, fonts: &mut self.fonts, ctx: &ui.ctx().clone(), base: c(p.document_text) };
        let out = editor.show(ui, area, &mut styled, &options);
        // Right-click on a misspelled word (the editor has put the caret
        // in it): what it might be, first.
        if out.response.secondary_clicked() {
            d.spell = None;
            if self.spelling
                && editable
                && let Some(checker) = olo_kit::spell::checker(ui.ctx())
                && let Some(r) = olo_kit::spell::word_at(d.doc.text(), d.editor.caret())
                && !checker.check(&d.doc.text()[r.clone()])
            {
                let guesses = checker.suggest(&d.doc.text()[r.clone()]);
                d.spell = Some((r, guesses));
            }
        }
        // Right-click: the Edit menu's own commands, so styles go along.
        let chosen = widgets::context_menu(&out.response, || {
            let selected = !d.bytes().is_empty();
            let mut menu = Menu::new("Edit");
            if let Some((_, guesses)) = &d.spell {
                for (k, g) in guesses.iter().enumerate() {
                    menu = menu.item(g.as_str(), format!("spell:{k}"), None);
                }
                if guesses.is_empty() {
                    menu = menu.item("No Guesses Found", "spell:none", None).enabled(false);
                }
                menu = menu.separator().item("Ignore Spelling", "spell:ignore", None).item("Learn Spelling", "spell:learn", None).separator();
            }
            menu.item("Cut", "cut", Some(Key::X))
                .enabled(editable && selected)
                .item("Copy", "copy", Some(Key::C))
                .enabled(selected)
                .item("Paste", "paste", Some(Key::V))
                .enabled(editable)
                .separator()
                .item("Select All", "select-all", Some(Key::A))
        });

        // The status line: where the caret is and what the file is.
        let painter = ui.painter();
        painter.rect_filled(status, 0.0, c(p.panel_background));
        style::hline(painter, status.left(), status.right(), status.top(), style::shade(&p, p.panel_background, 1.3));
        let (line, col) = d.editor.line_col(d.doc.text(), d.editor.caret());
        let mut info = format!("Line {line}, Column {col}");
        if let Some(words) = d.word_count() {
            let n = |k: usize| if k == 1 { "1 word".to_owned() } else { format!("{k} words") };
            info.push_str("   ");
            info.push_str(&match words {
                (Some(sel), all) => format!("{sel} of {}", n(all)),
                (None, all) => n(all),
            });
        }
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
            // Centred under the line along the top.
            status.center().y + style::hairline(ui.ctx()) / 2.0,
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
        // Tracker's New Window: always a new document.
        if !opened && args.iter().any(|a| a == "--new-window") {
            self.new_doc(cx);
        } else if !opened {
            // A document here, else a new one here (each workspace has
            // its own).
            match self.windows.iter().find(|(id, w)| matches!(w, Win::Doc(_)) && cx.on_this_workspace(**id)) {
                Some((&id, _)) => cx.activate(id),
                None => {
                    self.new_doc(cx);
                }
            }
        }
    }
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
        if matches!(self.windows.get(&window), Some(Win::Doc(_))) {
            olo_kit::dnd::open_files(ui, "open");
        }
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
                    (Purpose::SaveAs, None) | (Purpose::Export(_), None) => {}
                    (Purpose::Import, _) => self.import(cx, path),
                    (Purpose::Export(f), Some(doc)) => self.export(cx, doc, path, f),
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

    fn dropped(&mut self, cx: &mut Cx, _window: WindowId, drop: olo_kit::dnd::Drop) {
        for p in drop.paths() {
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
            Msg::Fonts(fonts::Event::Loaded) => {
                // Text in a font that has come laid out with it now.
                for w in self.windows.values_mut() {
                    if let Win::Doc(d) = w {
                        d.editor.relayout();
                    }
                }
                cx.redraw_all();
            }
            Msg::Loaded(win, path, result) => {
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                if d.path.as_ref() != Some(&path) {
                    return;
                }
                d.loading = false;
                match result {
                    Ok(l) => {
                        let mut doc = Doc::with_paras(l.text, l.runs, l.paras);
                        doc.align = l.align;
                        doc.wrap = l.wrap;
                        let (select, view) = d.restore.take().unwrap_or_default();
                        d.reset(doc, select);
                        d.editor.set_view(d.doc.text(), view);
                        d.latin1 = l.latin1;
                        d.read_only = l.read_only;
                        d.on_disk = stamp(&path);
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
                self.follow_files(cx);
            }
            Msg::Saved(win, generation, path, result) => {
                let Some(Win::Doc(d)) = self.windows.get_mut(&win) else { return };
                d.saving = false;
                let after = std::mem::replace(&mut d.after_save, After::Nothing);
                match result {
                    Ok(()) => {
                        d.on_disk = stamp(&path);
                        if d.path.as_ref() != Some(&path) {
                            d.path = Some(path);
                            d.read_only = false;
                            cx.set_title(win, d.title());
                            cx.set_path(win, d.path.clone());
                        }
                        d.latin1 = false;
                        d.doc.saved_generation = generation;
                        self.follow_files(cx);
                        self.finish(cx, win, after);
                    }
                    Err(e) => {
                        let message = format!("“{}” couldn't be saved. {e}", file_name(&path));
                        self.show_error(cx, message);
                    }
                }
                cx.redraw(win);
            }
            Msg::Imported(path, result) => match result {
                Ok((text, runs, paras)) => {
                    self.untitled += 1;
                    let mut d = DocWin::new(None, self.untitled);
                    let mut doc = Doc::with_paras(text, Some(runs), Some(paras));
                    // Not saved anywhere yet.
                    doc.saved_generation = 0;
                    d.reset(doc, 0..0);
                    self.add_window(cx, d);
                }
                Err(e) => self.show_error(cx, format!("“{}” couldn't be imported. {e}", file_name(&path))),
            },
            Msg::Exported(path, result) => {
                if let Err(e) = result {
                    self.show_error(cx, format!("“{}” couldn't be written. {e}", file_name(&path)));
                }
            }
            Msg::OnDisk(paths) => {
                // Written by another program (not our own save): read again
                // where nothing here is unsaved, keeping the place in it.
                let reload: Vec<(WindowId, PathBuf)> = self
                    .windows
                    .iter_mut()
                    .filter_map(|(&id, w)| {
                        let Win::Doc(d) = w else { return None };
                        let path = d.path.clone().filter(|p| paths.contains(p))?;
                        let now = stamp(&path);
                        if now.is_none() || now == d.on_disk || d.doc.edited() || d.saving || d.loading {
                            return None;
                        }
                        d.on_disk = now;
                        d.restore = Some((d.editor.selection(), d.editor.view()));
                        Some((id, path))
                    })
                    .collect();
                for (id, path) in reload {
                    self.load(cx, id, path);
                }
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
                    paras: if d.doc.edited() || d.path.is_none() { d.doc.paras().to_vec() } else { Vec::new() },
                    align: d.doc.align,
                    wrap: d.doc.wrap,
                    selection: (d.bytes().start, d.bytes().end),
                    view: d.editor.view(),
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
                    let mut doc = Doc::with_paras(text, Some(runs), Some(s.paras).filter(|p| !p.is_empty()));
                    doc.align = s.align;
                    doc.wrap = s.wrap;
                    // Unsaved work stays unsaved (an empty new one is clean).
                    if !(doc.text().is_empty() && s.path.is_none()) {
                        doc.saved_generation = 0;
                    }
                    d.reset(doc, s.selection.0..s.selection.1);
                    d.editor.set_view(d.doc.text(), s.view);
                    self.add_window(cx, d);
                }
                None => {
                    let Some(path) = s.path else { continue };
                    d.loading = true;
                    d.restore = Some((s.selection.0..s.selection.1, s.view));
                    let id = self.add_window(cx, d);
                    self.load(cx, id, path);
                }
            }
        }
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        let fonts = fonts::Fonts::start(move |e| proxy.send(Msg::Fonts(e)));
        let mut app = StyledEdit { windows: HashMap::new(), fonts, search: Search { wrap: true, ..Search::default() }, untitled: 0, watch: None, spelling: true };
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
