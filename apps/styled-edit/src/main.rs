//! StyledEdit: BeOS's text editor. Plain text files with fonts, sizes,
//! colours and styles kept beside the text in attributes (doc.rs), so
//! other programs still see plain text. One window per document; Find and
//! Replace windows as in Haiku; unsaved work survives a reload. On Olo's
//! kit: the document in the kit's editor (`olo_kit::editor`), its
//! paragraphs' indents and tabs on the kit's ruler.
//!
//! A file's kind sets how it's shown ([`Mode`]): source code (by its name
//! or `#!` line) in the kit's code view (`olo_kit::code`: coloured by its
//! language, line numbers), Markdown as it looks or as its source
//! (markdown.rs), anything else as a styled document.

mod convert;
mod doc;
mod fonts;
mod markdown;
mod pages;
mod pictures;
mod table;

use std::{
    cell::RefCell,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_kit::{
    Child, ClickEvent, ClipboardEntry, ClipboardItem, Cx, Element, ExternalPaths, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, LocalWindow, Menu, MenuBar, MenuItem,
    ParentElement as _, Pixels, Point, Styled as _, View, ViewRef, WindowHandle, WindowOptions, canvas,
    code::{self, Code},
    div,
    editor::{self, EditKey, Editor, EditorHost},
    file_panel::{self, FilePanel, Filter, PanelResult},
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{
        Button, CheckBox, SCROLL_BAR, TextControl, TextField, TextFieldEvent, ToolBar, Unsaved, ask_to_save, button_row, heading, open_alert, open_app_alert,
        ruler::{self, Ruler, RulerEvent},
        text_field,
    },
};
use serde::{Deserialize, Serialize};

use doc::{Align, Change, Doc, Run, Style};

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

// The menus' items with numbers in them, written out: a window says its
// menus at every body, and these were made again each time
// (`menus_say_the_commands_they_always_did` checks them).
/// `doc::Kind::ALL`'s commands, in its order.
const KIND_COMMANDS: [&str; 7] = ["kind:body", "kind:title", "kind:heading-1", "kind:heading-2", "kind:heading-3", "kind:quote", "kind:code"];
/// `doc::SIZES`' items, in its order.
const SIZE_ITEMS: [(&str, &str); 10] =
    [("9", "size:9"), ("10", "size:10"), ("11", "size:11"), ("12", "size:12"), ("14", "size:14"), ("18", "size:18"), ("24", "size:24"), ("36", "size:36"), ("48", "size:48"), ("72", "size:72")];
/// `COLORS`' commands, in its order.
const COLOR_COMMANDS: [&str; 7] = ["color:0", "color:1", "color:2", "color:3", "color:4", "color:5", "color:6"];
/// Markdown's Paragraph ▸ Heading 1 to 6.
const MD_HEADINGS: [(&str, &str); 6] =
    [("Heading 1", "md:heading-1"), ("Heading 2", "md:heading-2"), ("Heading 3", "md:heading-3"), ("Heading 4", "md:heading-4"), ("Heading 5", "md:heading-5"), ("Heading 6", "md:heading-6")];

/// Families per submenu of the Font menu.
const FAMILY_GROUP: usize = 24;

/// An imported document: its text, runs, paragraphs, header and footer.
type Imported = (String, Vec<Run>, Vec<doc::Para>, Option<doc::Page>);

/// What a window (or a worker of the app's) asks of the app, through the
/// proxy, on its looper.
enum ToHub {
    Fonts(fonts::Event),
    /// A file to show in a window of its own (or its window forward).
    Open(PathBuf),
    /// A new, untitled document.
    New,
    /// A formatted document to read in as a new one, and what was read.
    Import(PathBuf),
    Imported(PathBuf, Box<Result<Imported, String>>),
    /// A window's document is this file now (saved under another name).
    Showing(u64, Option<PathBuf>),
    /// A window couldn't read its file: it goes, and this is said.
    Failed(u64, String),
    Closed(u64),
    /// Open documents' files changed on disk (another program wrote one).
    OnDisk(Vec<PathBuf>),
    /// Something every window shows changed (spelling on or off, a word
    /// learnt): each says what it shows again (`true`: checks its
    /// spelling again).
    Again(bool),
    /// Quit: every window asked to close.
    Quit,
}

/// What a document's window is told: by its workers, its panels and the
/// app.
enum DocMsg {
    Loaded(PathBuf, Result<doc::Loaded, String>),
    /// Saved the document as it was at generation `.0` to `.1`.
    Saved(u64, PathBuf, Result<(), String>),
    OnDisk(Vec<PathBuf>),
    /// A picture to put in at the selection: its file and its size in
    /// pixels.
    Picture(PathBuf, Option<(u32, u32)>),
    /// One of its pictures decoded: drawn from now on.
    PictureLoaded(pictures::Loaded),
    /// A copy written in another format.
    Exported(PathBuf, Result<(), String>),
    /// The document sent to a printer (its title).
    Printed(String, Result<(), String>),
    /// A file panel of this window's answered (None: cancelled).
    Chosen(Purpose, Option<PathBuf>),
    /// The Print panel answered.
    Print(Option<olo_print::panel::Choice>),
    /// Asked to close (Quit), as its close box asks.
    Leave,
    /// Closed, nothing asked (its file couldn't be read).
    Close,
    /// Something every window shows changed (`ToHub::Again`).
    Again(bool),
}

/// What to do once the document is saved (or its changes discarded).
#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    Nothing,
    Close,
    Revert,
}

/// Words in the selection (if any) and in the whole document.
type Words = (Option<usize>, usize);

/// How a document is shown and edited (Ben, 2026-11-05: "when opening a
/// markdown file StyledEdit should natively support the Markdown … and
/// in source mode it should be like a code editor").
// (One to a document: nothing is saved by boxing its parts.)
#[allow(clippy::large_enum_variant)]
enum Mode {
    /// A styled document: its styles its own, kept beside the file.
    Styled,
    /// Source code: the file's own text in the monospaced face, coloured
    /// by its language, with line numbers; `indent` is what Tab types.
    Code { code: Code, indent: String },
    /// Markdown: the file's own text as it looks, or as its source.
    Markdown(markdown::Look),
}

impl Mode {
    /// How a file opens: Markdown by its name, code by its name or `#!`
    /// line, otherwise (and whenever it has styles of its own) styled.
    fn for_file(path: &Path, text: &str, styled: bool) -> Mode {
        if styled {
            return Mode::Styled;
        }
        if markdown::is_markdown(path) {
            return Mode::Markdown(markdown::Look::new(false));
        }
        match code::detect(path, text.lines().next().unwrap_or("")) {
            Some(lang) => Mode::code(lang, text),
            None => Mode::Styled,
        }
    }

    fn code(lang: &'static code::Language, text: &str) -> Mode {
        Mode::Code { code: Code::new(lang, markdown::CODE), indent: indent_unit(text) }
    }

    fn is_styled(&self) -> bool {
        matches!(self, Mode::Styled)
    }

    /// Shown as code: source code, or Markdown as its source.
    fn is_source(&self) -> bool {
        match self {
            Mode::Styled => false,
            Mode::Code { .. } => true,
            Mode::Markdown(l) => l.source,
        }
    }

    /// What it's saved as across a reload ([`Mode::named`]).
    fn id(&self) -> String {
        match self {
            Mode::Styled => "styled".into(),
            Mode::Markdown(l) => if l.source { "markdown-source" } else { "markdown" }.into(),
            Mode::Code { code, .. } => code.language().id.into(),
        }
    }

    /// The mode saved as `id`, for `text`.
    fn named(id: &str, text: &str) -> Option<Mode> {
        Some(match id {
            "styled" => Mode::Styled,
            "markdown" => Mode::Markdown(markdown::Look::new(false)),
            "markdown-source" => Mode::Markdown(markdown::Look::new(true)),
            id => Mode::code(code::by_name(id)?, text),
        })
    }

    /// What the status line says it is.
    fn name(&self) -> Option<&'static str> {
        match self {
            Mode::Styled => None,
            Mode::Code { code, .. } => Some(code.language().name),
            Mode::Markdown(l) => Some(if l.source { "Markdown Source" } else { "Markdown" }),
        }
    }
}

/// What Tab types in source code: a tab if its lines are indented with
/// them, else as many spaces as its indents step by (two or four).
fn indent_unit(text: &str) -> String {
    let (mut tabs, mut spaces, mut twos) = (0, 0, 0);
    for line in text.lines().take(2000) {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with(' ') {
            spaces += 1;
            let n = line.len() - line.trim_start_matches(' ').len();
            if n % 4 == 2 {
                twos += 1;
            }
        }
    }
    if tabs > spaces {
        "\t".into()
    } else if spaces > 0 && twos * 5 > spaces {
        "  ".into()
    } else {
        "    ".into()
    }
}

/// A picture's file from what a Markdown document names (`![](file)`):
/// from its folder when relative, `%20` and the like decoded. None: on
/// the web (not fetched).
fn markdown_picture(src: &str, folder: Option<&Path>) -> Option<PathBuf> {
    if src.contains("://") && !src.starts_with("file://") {
        return None;
    }
    let src = src.strip_prefix("file://").unwrap_or(src);
    let mut bytes = Vec::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = src.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            bytes.push(v);
            i += 3;
            continue;
        }
        bytes.push(b[i]);
        i += 1;
    }
    let p = PathBuf::from(String::from_utf8(bytes).ok()?);
    Some(match folder {
        Some(f) if p.is_relative() => f.join(p),
        _ => p,
    })
}

/// A document's window.
struct DocView {
    id: u64,
    shared: Arc<Shared>,
    /// A save is under way (an upgrade waits: `Shared::saving`).
    saving_said: Arc<AtomicBool>,
    /// None until first saved.
    path: Option<PathBuf>,
    /// "Untitled N" for a new document.
    untitled: usize,
    doc: Doc,
    /// Shows and edits `doc` (a paragraph at a time: documents of any
    /// length stay quick).
    editor: Editor,
    ruler_view: Child<Ruler>,
    loading: bool,
    saving: bool,
    after_save: After,
    /// Asking about unsaved changes (one alert at a time).
    asking: bool,
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
    /// The ruler shows (Document ▸ Show Ruler).
    ruler: bool,
    /// The text is set on pages (Document ▸ Page View).
    page_view: bool,
    /// The tool bars show (Document ▸ Show Toolbars).
    tool_bars: bool,
    pictures: RefCell<pictures::Pictures>,
    /// The paper and pages the next PDF is made with (the Print panel's,
    /// when it chose a PDF).
    pdf_setup: Option<(olo_print::Paper, Option<(u32, u32)>)>,
    /// Its Find and Replace windows, and its Header & Footer one.
    finds: [Option<LocalWindow<FindView>>; 2],
    margins: Option<LocalWindow<MarginsView>>,
    /// A picture pasted (its bytes), to put in once kept.
    pasted: Option<Vec<u8>>,
    /// Styled, source code or Markdown.
    mode: Mode,
    /// The link right-clicked in a Markdown document (Open Link).
    link: Option<String>,
    /// Source shows line numbers (Document ▸ Line Numbers).
    line_numbers: bool,
    /// Where a marker held on the ruler goes (a line down the text).
    guide: Option<f32>,
}

/// The document as the editor sees it: its text, styles in the fonts
/// they use, and edits that keep both (and undo) in step.
struct Styled<'a> {
    doc: &'a mut Doc,
    /// Its pictures, and the folder their relative paths are from (the
    /// document's).
    pictures: &'a RefCell<pictures::Pictures>,
    folder: Option<&'a Path>,
    pasted: &'a mut Option<Vec<u8>>,
    editable: bool,
    mode: &'a mut Mode,
    palette: &'a olo_theme::Palette,
    /// The text's width, for a Markdown picture's.
    width: f32,
}

impl Styled<'_> {
    /// Source code's syntax (a Markdown document's lines) told of the
    /// text's changes.
    fn sync(&mut self) {
        sync_mode(self.mode, self.doc);
    }

    /// The file Markdown picture `src` is in: one beside the document, or
    /// one on the web, fetched there.
    fn picture_path(pictures: &RefCell<pictures::Pictures>, folder: Option<&Path>, src: &str) -> Option<PathBuf> {
        if src.starts_with("https://") || src.starts_with("http://") {
            return Some(pictures.borrow_mut().web(src));
        }
        markdown_picture(src, folder)
    }

    /// A Markdown picture's size, if it's loaded (fit to the text).
    fn picture_size(pictures: &RefCell<pictures::Pictures>, folder: Option<&Path>, width: f32, src: &str) -> Option<(f32, f32)> {
        let path = Styled::picture_path(pictures, folder, src)?;
        let img = pictures.borrow_mut().image(&path)?;
        let s = img.size(0);
        Some(pictures::fit((s.width.0.max(1) as u32, s.height.0.max(1) as u32), width.max(40.0), 600.0))
    }
}

impl editor::Document for Styled<'_> {
    fn text(&self) -> &str {
        self.doc.text()
    }

    fn spans(&mut self, range: Range<usize>, out: &mut Vec<editor::Span>) {
        self.sync();
        let (pictures, folder, width) = (self.pictures, self.folder, self.width);
        match &mut *self.mode {
            Mode::Styled => self.doc.spans(range, &mut fonts::font, out),
            Mode::Code { code, .. } => code.spans(self.doc.text(), range, self.palette, out),
            Mode::Markdown(look) => {
                look.sync(self.doc.text(), self.doc.generation);
                look.spans(self.doc.text(), range, self.palette, &mut |src| Styled::picture_size(pictures, folder, width, src), out);
            }
        }
    }

    fn link_at(&mut self, at: usize) -> Option<String> {
        match &mut *self.mode {
            Mode::Markdown(look) => {
                look.sync(self.doc.text(), self.doc.generation);
                look.link_at(at)
            }
            _ => None,
        }
    }

    fn paragraph(&mut self, range: Range<usize>) -> editor::Paragraph {
        self.sync();
        let (pictures, folder, width) = (self.pictures, self.folder, self.width);
        match &mut *self.mode {
            Mode::Styled => {}
            Mode::Code { .. } => return editor::Paragraph::default(),
            Mode::Markdown(look) => {
                look.sync(self.doc.text(), self.doc.generation);
                return look.paragraph(range, self.palette, &mut |src| {
                    let size = Styled::picture_size(pictures, folder, width, src)?;
                    let path = Styled::picture_path(pictures, folder, src)?;
                    let image = pictures.borrow_mut().image(&path);
                    Some(editor::Picture { image, size })
                });
            }
        }
        let mut f = self.doc.format(self.doc.para_index(range.start), self.doc.align);
        for (at, pic) in self.doc.pictures_in(range) {
            let image = self.pictures.borrow_mut().image(&pic.resolve(self.folder));
            f.pictures.push((at, editor::Picture { image, size: (pic.width, pic.height) }));
        }
        f
    }

    fn replace(&mut self, range: Range<usize>, text: &str, selection: Range<usize>) {
        self.doc.edit(range, text, selection);
        // (A paste's name taken: none left for what's typed next.)
        self.doc.name_next(None);
        self.sync();
    }

    fn copied(&mut self, range: Range<usize>) {
        self.doc.copied(range);
    }

    fn set_columns(&mut self, range: Range<usize>, columns: Vec<f32>) {
        table::set_columns(self.doc, self.doc.para_index(range.start), columns, range.start..range.start);
    }

    fn move_text(&mut self, from: Range<usize>, to: usize, selection: Range<usize>) -> Range<usize> {
        let r = self.doc.move_text(from, to, selection);
        self.sync();
        r
    }

    /// Markdown's Return goes on with the list or quotation it's in;
    /// source code's keeps the line's indent, and its Tab types the
    /// file's own indent (⇧Tab takes one away).
    fn types(&mut self, key: EditKey, sel: Range<usize>) -> Option<(Range<usize>, String, usize)> {
        if !self.editable {
            return None;
        }
        let text = self.doc.text();
        match (&mut *self.mode, key) {
            (Mode::Markdown(look), EditKey::Enter) => {
                look.sync(text, self.doc.generation);
                look.newline(text, sel)
            }
            (Mode::Code { .. }, EditKey::Enter) => {
                let start = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
                let indent: String = text[start..sel.start].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                let typed = format!("\n{indent}");
                let n = typed.len();
                (!indent.is_empty()).then_some((sel, typed, n))
            }
            // Lines selected: each indented, or outdented, by one.
            (Mode::Code { indent, .. }, EditKey::Tab | EditKey::BackTab) if !sel.is_empty() => {
                let start = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
                let end = sel.end - usize::from(sel.end > sel.start && text[..sel.end].ends_with('\n'));
                let end = text[end..].find('\n').map_or(text.len(), |i| end + i);
                let unit = indent.clone();
                let lines: Vec<String> = text[start..end]
                    .split('\n')
                    .map(|l| {
                        if key == EditKey::Tab {
                            if l.is_empty() { String::new() } else { format!("{unit}{l}") }
                        } else {
                            let take = if l.starts_with('\t') { 1 } else { (l.len() - l.trim_start_matches(' ').len()).min(unit.len().max(1)) };
                            l[take..].to_owned()
                        }
                    })
                    .collect();
                let made = lines.join("\n");
                let n = made.len();
                Some((start..end, made, n))
            }
            (Mode::Code { indent, .. }, EditKey::Tab) if sel.is_empty() && indent.as_str() != "\t" => {
                // To the next indent stop.
                let start = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
                let col = text[start..sel.start].chars().count();
                let n = indent.len() - col % indent.len();
                Some((sel, " ".repeat(n), n))
            }
            (Mode::Code { indent, .. }, EditKey::BackTab) if sel.is_empty() => {
                let start = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
                let line = &text[start..sel.start];
                let take = if line.starts_with('\t') { 1 } else { line.len() - line.trim_start_matches(' ').len().min(line.len()) };
                let take = take.min(if indent.as_str() == "\t" { 1 } else { indent.len() });
                (take > 0).then(|| {
                    let rest = line[take..].to_owned();
                    let n = rest.len();
                    (start..sel.start, rest, n)
                })
            }
            _ => None,
        }
    }

    fn paste(&mut self, item: &ClipboardItem, _: Range<usize>) -> Option<String> {
        // Source code and Markdown take text alone.
        if !self.mode.is_styled() {
            return item.text();
        }
        // A picture on the clipboard goes in as one; else its text.
        for e in item.entries() {
            match e {
                ClipboardEntry::Image(img) => {
                    *self.pasted = Some(img.bytes.clone());
                    return None;
                }
                ClipboardEntry::Data(d) if d.mime.starts_with("image/") => {
                    *self.pasted = Some(d.bytes.to_vec());
                    return None;
                }
                _ => {}
            }
        }
        item.text()
    }

    /// Keys a list item takes before the editor: Return in an empty one
    /// ends the list (or steps out a level), Tab and ⇧Tab indent it more
    /// or less, Backspace at its start takes its marker away.
    fn key(&mut self, key: EditKey, sel: Range<usize>) -> bool {
        if !self.editable || !self.mode.is_styled() {
            return false;
        }
        let i = self.doc.para_index(sel.start);
        let para = self.doc.paras()[i].clone();
        if para.list == doc::List::None {
            return false;
        }
        let range = self.doc.para_range(i);
        let empty = sel.is_empty() && range.is_empty();
        let at_start = sel.is_empty() && sel.start == range.start;
        match key {
            EditKey::Enter if empty => {
                let out = para.level == 0;
                self.doc.set_paras(sel.clone(), sel, |p| if out { p.list = doc::List::None } else { p.level -= 1 });
            }
            EditKey::Tab => self.doc.set_paras(sel.clone(), sel, |p| p.level = (p.level + 1).min(doc::MAX_LEVEL)),
            EditKey::BackTab => self.doc.set_paras(sel.clone(), sel, |p| p.level = p.level.saturating_sub(1)),
            EditKey::Backspace if at_start && !empty => self.doc.set_paras(sel.clone(), sel, |p| p.list = doc::List::None),
            _ => return false,
        }
        true
    }
}

impl EditorHost for DocView {
    fn editor(&mut self, _: usize, cx: &mut Cx<Self>, f: &mut dyn FnMut(&mut Editor, &mut dyn editor::Document, &mut Cx<Self>)) {
        let editable = !self.read_only && !self.loading;
        let folder = self.path.as_deref().and_then(Path::parent);
        let palette = theme::palette();
        let width = self.editor.text_span().map_or(400.0, |s| s.1);
        let mut styled = Styled { doc: &mut self.doc, pictures: &self.pictures, folder, pasted: &mut self.pasted, editable, mode: &mut self.mode, palette: &palette, width };
        f(&mut self.editor, &mut styled, cx);
        styled.sync();
    }

    fn editor_shown(&self, _: usize) -> Option<(&Editor, &str)> {
        Some((&self.editor, self.doc.text()))
    }

    fn text_moved(&mut self, _: usize, (origin, width): (f32, f32), cx: &mut Cx<Self>) {
        self.ruler_view.update(cx, |r, cx| {
            r.follow(origin, width);
            cx.invalidate();
        });
    }

    fn edited(&mut self, _: usize, cx: &mut Cx<Self>) {
        // A picture pasted: kept as a file (on a worker), then put in.
        if let Some(data) = self.pasted.take() {
            let to = cx.messenger();
            std::thread::spawn(move || {
                if let Some(path) = pictures::hold(&data) {
                    let px = pictures::pixels(&path);
                    to.send(DocMsg::Picture(path, px));
                }
            });
        }
        cx.invalidate();
    }

    fn open_link(&mut self, _: usize, link: String, _: &mut Cx<Self>) {
        self.open(link);
    }

    fn context_menu(&mut self, _: usize, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let editable = !self.read_only && !self.loading;
        let spelling = self.shared.common().spelling;
        // On a misspelled word (the editor has put the caret in it): what
        // it might be, first.
        self.spell = None;
        if spelling
            && editable
            && let Some(checker) = olo_kit::spell::checker(&cx.invalidator())
            && let Some(r) = olo_kit::spell::word_at(self.doc.text(), self.editor.caret())
            && !checker.check(&self.doc.text()[r.clone()])
        {
            let guesses = checker.suggest(&self.doc.text()[r.clone()]);
            self.spell = Some((r, guesses));
        }
        let selected = !self.bytes().is_empty();
        let mut menu = Menu::new("Edit");
        // On a link in a Markdown document: where it goes, first.
        self.link = match &mut self.mode {
            Mode::Markdown(look) => {
                look.sync(self.doc.text(), self.doc.generation);
                look.link_at(self.editor.caret())
            }
            _ => None,
        };
        if self.link.is_some() {
            menu = menu.item("Open Link", "open-link").separator();
        }
        if let Some((_, guesses)) = &self.spell {
            for (k, g) in guesses.iter().enumerate() {
                menu = menu.item(g.clone(), format!("spell:{k}"));
            }
            if guesses.is_empty() {
                menu = menu.item("No Guesses Found", "spell:none").enabled(false);
            }
            menu = menu.separator().item("Ignore Spelling", "spell:ignore").item("Learn Spelling", "spell:learn").separator();
        }
        // The Edit menu's own commands, so styles go along.
        let menu = menu
            .item("Cut", "cut")
            .enabled(editable && selected)
            .keys("⌘X")
            .item("Copy", "copy")
            .enabled(selected)
            .keys("⌘C")
            .item("Paste", "paste")
            .enabled(editable)
            .keys("⌘V")
            .separator()
            .item("Select All", "select-all")
            .keys("⌘A");
        let me = cx.view_ref();
        olo_kit::menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |d, cx| d.run(&cmd, cx));
        });
    }
}

impl DocView {
    /// Opens link `to` (Open Link, or the command key and a click): a
    /// file from the document's folder, a URL by what opens it, on a
    /// worker (it may start an app). Not a link within the document
    /// (`#heading`).
    fn open(&self, to: String) {
        if to.starts_with('#') {
            return;
        }
        let target = match olo_xdg::open::Target::parse(&to) {
            olo_xdg::open::Target::Path(p) if p.is_relative() => olo_xdg::open::Target::Path(self.path.as_deref().and_then(Path::parent).map_or(p.clone(), |f| f.join(p))),
            t => t,
        };
        std::thread::spawn(move || {
            if let Err(e) = olo_xdg::open::open(&target) {
                tracing::warn!("opening {to}: {e}");
            }
        });
    }

    fn new(id: u64, shared: Arc<Shared>, path: Option<PathBuf>, untitled: usize, cx: &mut Cx<Self>) -> DocView {
        let ruler_view = cx.new_view(|_| Ruler::new());
        cx.subscribe(&ruler_view, |d: &mut DocView, e: &RulerEvent, cx| d.ruled(e, cx));
        let saving_said = Arc::new(AtomicBool::new(false));
        {
            let mut saving = shared.saving.lock().unwrap_or_else(|e| e.into_inner());
            saving.retain(|s| s.strong_count() > 0);
            saving.push(Arc::downgrade(&saving_said));
        }
        // Its pictures, decoded on workers, come to it.
        let mut pictures = pictures::Pictures::default();
        let to = cx.messenger();
        pictures.set_sender(move |loaded| {
            to.send(DocMsg::PictureLoaded(loaded));
        });
        let (ruler, tool_bars) = (shared.ruler.load(Ordering::Relaxed), shared.tool_bars.load(Ordering::Relaxed));
        DocView {
            id,
            shared,
            saving_said,
            path,
            untitled,
            doc: Doc::new(String::new(), None),
            editor: Editor::new(),
            ruler_view,
            loading: false,
            saving: false,
            after_save: After::Nothing,
            asking: false,
            latin1: false,
            read_only: false,
            restore: None,
            on_disk: None,
            spell: None,
            words: None,
            ruler,
            page_view: false,
            tool_bars,
            pictures: RefCell::new(pictures),
            pdf_setup: None,
            finds: [None, None],
            margins: None,
            pasted: None,
            mode: Mode::Styled,
            link: None,
            line_numbers: true,
            guide: None,
        }
    }

    /// The ruler changed the caret's paragraph's indents and tabs: the
    /// selected paragraphs take them (one step to undo).
    fn ruled(&mut self, e: &RulerEvent, cx: &mut Cx<Self>) {
        if let RulerEvent::Changed(s) = e
            && !self.read_only
            && !self.loading
        {
            let bytes = self.bytes();
            let r = self.doc.paras_in(bytes.clone());
            let whole = self.doc.para_range(r.start).start..self.doc.para_range(r.end - 1).end;
            self.doc.set_paras(bytes.clone(), bytes, |p| {
                if p.row.is_none() {
                    p.set_ruler(s);
                }
            });
            self.editor.restyled(whole);
        }
        cx.invalidate();
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
        // (A picture isn't a word.)
        let count = |t: &str| t.split(|c: char| c.is_whitespace() || c == editor::PICTURE).filter(|w| !w.is_empty()).count();
        let text = self.doc.text();
        let w = ((!sel.is_empty()).then(|| count(&text[sel.clone()])), count(text));
        self.words = Some((key, w));
        Some(w)
    }

    fn title(&self) -> String {
        match &self.path {
            Some(p) => file_name(p),
            None => format!("Untitled {}", self.untitled),
        }
    }

    /// The window's title, as the document is now called.
    fn set_title(&self, cx: &mut Cx<Self>) {
        cx.set_window_title(self.title());
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
        self.sync_code();
        self.editor.replaced(self.doc.text(), change.removed, change.inserted);
        self.editor.select(self.doc.text(), select);
    }

    /// Source code's syntax told of the text's changes (undo, Replace).
    fn sync_code(&mut self) {
        sync_mode(&mut self.mode, &mut self.doc);
    }

    /// Shown as `mode` from now (the text the same, its place kept).
    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.doc.keep_edits(!self.mode.is_styled());
        if let Mode::Code { code, .. } = &mut self.mode {
            code.reset();
        }
        self.editor.relayout();
    }

    /// A new document (loaded, restored), selecting `select`.
    fn reset(&mut self, doc: Doc, select: Range<usize>) {
        self.doc = doc;
        self.doc.keep_edits(!self.mode.is_styled());
        if let Mode::Code { code, .. } = &mut self.mode {
            code.reset();
        }
        // (Its generations start again: the count kept was the old one's.)
        self.words = None;
        self.editor.reset(self.doc.text());
        self.editor.select(self.doc.text(), select);
    }

    /// Document ▸ Language: shown as styled text, Markdown, or source code
    /// in language `id`.
    fn choose_language(&mut self, id: &str) {
        let mode = match id {
            "styled" => Mode::Styled,
            "markdown" => Mode::Markdown(markdown::Look::new(false)),
            id => match code::by_name(id) {
                Some(lang) => Mode::code(lang, self.doc.text()),
                None => return,
            },
        };
        // (Styles would be lost: a styled document with its own stays.)
        if self.mode.is_styled() && !matches!(mode, Mode::Styled) && !self.doc.is_plain() {
            return;
        }
        self.set_mode(mode);
    }

    /// A Markdown document's own commands: styles and paragraphs made with
    /// its marks. Whether `command` was one.
    fn markdown_command(&mut self, command: &str) -> bool {
        if self.read_only || self.loading {
            return false;
        }
        use markdown::Make;
        match command {
            "bold" => self.md_wrap("**"),
            "italic" => self.md_wrap("*"),
            "strikeout" => self.md_wrap("~~"),
            "md:code" => self.md_wrap("`"),
            "md:link" => {
                // [the selection](), the caret where the address goes.
                let sel = self.bytes();
                let typed = format!("[{}]()", &self.doc.text()[sel.clone()]);
                let at = sel.start + typed.len() - 1;
                let change = self.doc.replace(sel.clone(), &typed, sel);
                self.changed(change, at..at);
            }
            "md:body" => self.md_lines(Make::Body),
            "md:quote" => self.md_lines(Make::Quote),
            "list-bullet" => self.md_lines(Make::Bullets),
            "list-number" => self.md_lines(Make::Numbers),
            "md:tasks" => self.md_lines(Make::Tasks),
            c => match c.strip_prefix("md:heading-").and_then(|n| n.parse::<u8>().ok()) {
                Some(n) => self.md_lines(Make::Heading(n)),
                None => return false,
            },
        }
        true
    }

    /// The selection between `mark`s (`**` bold): taken away if it's
    /// already between them.
    fn md_wrap(&mut self, mark: &str) {
        let sel = self.bytes();
        let text = self.doc.text();
        let m = mark.len();
        let around = sel.start >= m && text.get(sel.start - m..sel.start) == Some(mark) && text.get(sel.end..sel.end + m) == Some(mark);
        if around {
            if let Some(change) = self.doc.replace_all(&[sel.start - m..sel.start, sel.end..sel.end + m], "", sel.clone()) {
                self.changed(change, sel.start - m..sel.end - m);
            }
        } else {
            let typed = format!("{mark}{}{mark}", &text[sel.clone()]);
            let change = self.doc.replace(sel.clone(), &typed, sel.clone());
            self.changed(change, sel.start + m..sel.end + m);
        }
    }

    /// The selection's lines made `make` (a heading, a list), as one step.
    fn md_lines(&mut self, make: markdown::Make) {
        let sel = self.bytes();
        let r = self.doc.paras_in(sel.clone());
        let whole = self.doc.para_range(r.start).start..self.doc.para_range(r.end - 1).end;
        let was = &self.doc.text()[whole.clone()];
        let made = markdown::make_lines(was, make);
        if made == was {
            return;
        }
        let change = self.doc.replace(whole.clone(), &made, sel.clone());
        let to = if sel.is_empty() { whole.start + made.len()..whole.start + made.len() } else { whole.start..whole.start + made.len() };
        self.changed(change, to);
    }

    /// What it's printed and exported as, if not its own styles
    /// (Markdown as it looks, code coloured).
    fn source(&self) -> Option<convert::Source> {
        match &self.mode {
            Mode::Styled => None,
            Mode::Markdown(_) => Some(convert::Source::Markdown(self.path.as_deref().and_then(Path::parent).map(Path::to_path_buf))),
            Mode::Code { code, .. } => Some(convert::Source::Code(code.language())),
        }
    }

    /// Select bytes `r` of the text.
    fn select_bytes(&mut self, r: Range<usize>) {
        self.editor.select(self.doc.text(), r);
    }

    /// The status line: where the caret is and what the file is.
    fn status(&mut self) -> String {
        let (line, col) = self.editor.line_col(self.doc.text(), self.editor.caret());
        let mut info = format!("Line {line}, Column {col}");
        if let Some(words) = self.word_count() {
            let n = |k: usize| if k == 1 { "1 word".to_owned() } else { format!("{k} words") };
            info.push_str("   ");
            info.push_str(&match words {
                (Some(sel), all) => format!("{sel} of {}", n(all)),
                (None, all) => n(all),
            });
        }
        if self.loading {
            info = "Loading…".into();
        }
        if self.saving {
            info.push_str("   Saving…");
        }
        if self.read_only {
            info.push_str("   Read-only");
        }
        if self.latin1 {
            info.push_str("   Latin-1 (saved as UTF-8)");
        }
        if let Some(name) = self.mode.name() {
            info.push_str("   ");
            info.push_str(name);
        }
        info
    }

    /// A command about the document itself (its styles, layout, undo).
    fn edit_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        match command {
            "source" => {
                if let Mode::Markdown(look) = &mut self.mode {
                    look.source = !look.source;
                    self.editor.relayout();
                }
                return cx.invalidate();
            }
            "line-numbers" => {
                self.line_numbers = !self.line_numbers;
                return cx.invalidate();
            }
            "open-link" => {
                // A link to a file is from the document's folder; opened by
                // what opens its kind, on a worker (it may start an app).
                // (Not a link within the document, `#heading`.)
                if let Some(to) = self.link.take() {
                    self.open(to);
                }
                return;
            }
            c if c.starts_with("language:") => {
                self.choose_language(&c["language:".len()..]);
                return cx.invalidate();
            }
            _ => {}
        }
        if matches!(self.mode, Mode::Markdown(_)) && self.markdown_command(command) {
            return cx.invalidate();
        }
        // (Styles are a styled document's alone.)
        let style_command = ["bold", "italic", "underline", "strikeout", "indent-more", "indent-less", "list-bullet", "list-number"].contains(&command)
            || ["align-", "kind:", "size:", "color:", "family:", "table:", "picture:"].iter().any(|p| command.starts_with(p));
        if !self.mode.is_styled() && style_command {
            return;
        }
        // The step it makes, named for it (Edit ▸ Undo Bold); a paste's
        // name is taken by the paste, which comes after.
        self.doc.name_next(step_name(command));
        self.doc_command(command, cx);
        if command != "paste" {
            self.doc.name_next(None);
        }
    }

    /// The document's part of `command`: undo, the clipboard, styles.
    fn doc_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        let selection = self.bytes();
        let bytes = selection.clone();
        let restyle = |d: &mut DocView, f: &dyn Fn(&mut Style)| {
            d.doc.restyle(bytes.clone(), selection.clone(), f);
            d.editor.restyled(bytes.clone());
        };
        match command {
            "undo" | "redo" => {
                let to = if command == "undo" { self.doc.undo(selection) } else { self.doc.redo(selection) };
                if let Some((to, change)) = to {
                    self.changed(change, to);
                }
            }
            "copy" | "cut" => {
                self.doc.copied(bytes.clone());
                olo_kit::clipboard::write(ClipboardItem::new_string(self.selected_text().to_owned()));
                if command == "cut" && !self.read_only {
                    let change = self.doc.replace(bytes, "", selection.clone());
                    self.changed(change, selection.start..selection.start);
                }
            }
            "paste" => {
                editor::edit(self, 0, editor::Edit::Paste, cx);
            }
            "clear" if !self.read_only => {
                let change = self.doc.replace(bytes, "", selection.clone());
                self.changed(change, selection.start..selection.start);
            }
            "select-all" => {
                let all = 0..self.doc.text().len();
                self.editor.select(self.doc.text(), all);
            }
            c if c.starts_with("picture:") => {
                let Some(pic) = picture_selected(&self.doc, bytes.clone()) else { return };
                let k = match c {
                    "picture:fit" => PICTURE_ROOM.0 / pic.width,
                    "picture:larger" => 1.25,
                    _ => 0.8,
                };
                restyle(self, &|s| {
                    if let Some(p) = &mut s.picture {
                        (p.width, p.height) = ((p.width * k).round().max(4.0), (p.height * k).round().max(4.0));
                    }
                });
            }
            "bold" => {
                let on = !self.doc.style_at(bytes.start).bold;
                restyle(self, &|s| s.bold = on);
            }
            "italic" => {
                let on = !self.doc.style_at(bytes.start).italic;
                restyle(self, &|s| s.italic = on);
            }
            "underline" => {
                let on = !self.doc.style_at(bytes.start).underline;
                restyle(self, &|s| s.underline = on);
            }
            "strikeout" => {
                let on = !self.doc.style_at(bytes.start).strikeout;
                restyle(self, &|s| s.strikeout = on);
            }
            "align-left" | "align-center" | "align-right" | "align-justify" => {
                let a = match command {
                    "align-center" => Align::Center,
                    "align-right" => Align::Right,
                    "align-justify" => Align::Justify,
                    _ => Align::Left,
                };
                let base = self.doc.align;
                self.doc.set_paras(bytes, selection, |p| p.align = (a != base).then_some(a));
            }
            "list-bullet" | "list-number" => {
                let list = if command == "list-bullet" { doc::List::Bullet } else { doc::List::Number };
                // On if the first paragraph isn't in this kind of list yet.
                let on = self.doc.paras()[self.doc.para_index(bytes.start)].list != list;
                self.doc.set_paras(bytes, selection, |p| p.list = if on { list } else { doc::List::None });
            }
            c if c.starts_with("spell:") => {
                // A guess for the misspelled word, in its place.
                let guess = c.strip_prefix("spell:").and_then(|k| k.parse::<usize>().ok());
                if let (Some(k), Some((r, guesses))) = (guess, self.spell.take())
                    && let Some(g) = guesses.get(k)
                    && r.end <= self.doc.text().len()
                {
                    let change = self.doc.replace(r.clone(), g, selection);
                    self.changed(change, r.start + g.len()..r.start + g.len());
                }
            }
            c if c.starts_with("table:") => {
                let caret = self.editor.caret().min(self.doc.text().len());
                let i = self.doc.para_index(caret);
                let doc = &mut self.doc;
                let done = match c {
                    "table:insert" => table::insert(doc, i, 3, 3, selection),
                    "table:convert" => table::convert(doc, bytes, true, selection),
                    "table:text" => table::convert(doc, bytes, false, selection),
                    "table:header" => table::toggle_header(doc, i, selection),
                    "table:row-above" => table::add_row(doc, i, false, selection),
                    "table:row-below" => table::add_row(doc, i, true, selection),
                    "table:column-before" => table::add_column(doc, i, caret, false, selection),
                    "table:column-after" => table::add_column(doc, i, caret, true, selection),
                    "table:delete-row" => table::delete_rows(doc, i..i + 1, selection),
                    "table:delete-column" => table::delete_column(doc, i, caret, selection),
                    "table:delete" => table::table_of(doc, i).and_then(|t| table::delete_rows(doc, t, selection)),
                    "table:even" => table::even_columns(doc, i, selection),
                    _ => None,
                };
                if let Some((change, select)) = done {
                    self.changed(change, select);
                    // Rows set otherwise, beyond what the text change touched.
                    self.editor.relayout();
                }
            }
            "indent-more" => self.doc.set_paras(bytes, selection, |p| p.level = (p.level + 1).min(doc::MAX_LEVEL)),
            "indent-less" => self.doc.set_paras(bytes, selection, |p| p.level = p.level.saturating_sub(1)),
            "wrap" => {
                self.doc.wrap = !self.doc.wrap;
                // Layout changes are edits too (they're saved with the file).
                self.doc.generation += 1;
            }
            "ruler" => self.ruler = !self.ruler,
            "tool-bars" => self.tool_bars = !self.tool_bars,
            "page-view" => {
                self.page_view = !self.page_view;
                // (Set on pages, lines wrap.)
                self.doc.wrap |= self.page_view;
            }
            c if c.starts_with("kind:") => {
                let Some((kind, ..)) = doc::Kind::ALL.into_iter().find(|(_, _, id)| Some(*id) == c.strip_prefix("kind:")) else { return };
                let r = self.doc.paras_in(bytes.clone());
                let whole = self.doc.para_range(r.start).start..self.doc.para_range(r.end - 1).end;
                self.doc.set_kind(bytes, selection, kind);
                self.editor.restyled(whole);
            }
            c => {
                if let Some(size) = c.strip_prefix("size:").and_then(|s| s.parse::<f32>().ok()) {
                    restyle(self, &|s| s.size = size);
                } else if let Some(k) = c.strip_prefix("color:").and_then(|s| s.parse::<usize>().ok())
                    && let Some((_, rgb)) = COLORS.get(k)
                {
                    restyle(self, &|s| s.color = *rgb);
                } else if let Some(family) = c.strip_prefix("family:") {
                    restyle(self, &|s| s.family = family.to_owned());
                } else {
                    return;
                }
            }
        }
        cx.invalidate();
    }
}

/// What the step a command makes is called (Edit ▸ Undo *Bold*); None:
/// what the edit calls it.
fn step_name(command: &str) -> Option<&'static str> {
    Some(match command {
        "cut" => "Cut",
        "clear" => "Delete",
        "paste" => "Paste",
        "bold" => "Bold",
        "italic" => "Italic",
        "underline" => "Underline",
        "strikeout" => "Strikeout",
        "indent-more" | "indent-less" => "Indent",
        "list-bullet" | "list-number" => "List",
        c if c.starts_with("align-") => "Alignment",
        c if c.starts_with("kind:") => "Paragraph Style",
        c if c.starts_with("size:") => "Size",
        c if c.starts_with("color:") => "Color",
        c if c.starts_with("family:") => "Font",
        c if c.starts_with("table:") => "Table",
        c if c.starts_with("picture:") => "Picture Size",
        _ => return None,
    })
}

/// Font ▸ Size, checking the size where the selection starts.
fn size_menu(cur: &Style) -> Menu {
    let mut size = Menu::new("Size");
    for (s, (label, command)) in doc::SIZES.into_iter().zip(SIZE_ITEMS) {
        size = size.item(label, command).checked(cur.size == s);
    }
    size
}

/// Font ▸ Color, each with its swatch (Black the document's text colour,
/// `ink`, as it shows).
fn color_menu(cur: &Style, ink: olo_kit::Hsla) -> Menu {
    let mut color = Menu::new("Color");
    for (k, (name, rgb)) in COLORS.iter().enumerate() {
        color = color.item(*name, COLOR_COMMANDS[k]).checked(cur.color == *rgb).swatch(rgb.map_or(ink, rgb_color));
    }
    color
}

fn rgb_color([r, g, b]: [u8; 3]) -> olo_kit::Hsla {
    olo_kit::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into()
}

/// `menu` with the font families after it: the system font, those the
/// document uses, then all of them in alphabetical groups (menus don't
/// scroll), the one where the selection starts checked.
fn families(mut font: Menu, app: &Common, d: &DocView, cur: &Style) -> Menu {
    font = font.item("System Font", "family:").checked(cur.family.is_empty());
    let mut used: Vec<&str> = d.doc.runs().iter().map(|r| r.style.family.as_str()).filter(|f| !f.is_empty() && *f != doc::MONOSPACE).collect();
    used.sort_unstable();
    used.dedup();
    for f in &used {
        font = font.item(f.to_string(), format!("family:{f}")).checked(cur.family == *f);
    }
    if !app.fonts.groups.is_empty() {
        font = font.separator();
        for group in &app.fonts.groups {
            // (Made once, words shared: only the one where the selection
            // starts is checked here.)
            let mut group = group.clone();
            if !cur.family.is_empty() {
                for item in &mut group.items {
                    if let MenuItem::Action(a) = item {
                        a.checked = a.label == cur.family;
                    }
                }
            }
            font = font.submenu(group);
        }
    }
    font
}

/// The Font menu's groups of `families` (alphabetical, a group's title
/// its guide words), each family's item `family:` and its name.
fn family_groups(families: &[String]) -> Vec<Menu> {
    let chunks: Vec<&[String]> = families.chunks(FAMILY_GROUP).collect();
    chunks.iter().zip(guide_words(&chunks)).map(|(chunk, title)| chunk.iter().fold(Menu::new(title), |group, f| group.item(f.clone(), format!("family:{f}")))).collect()
}

/// The tool bars: along the top, the file's and editing's commands and
/// the text's style (wrapping to a second row in a narrow window); down
/// the side, what goes in and how the document shows. Every tool is one
/// of `menus`' commands.
fn tool_bars(cx: &Cx<DocView>, d: &DocView, menus: &[Menu], ink: olo_kit::Hsla) -> (ToolBar, ToolBar) {
    let editable = !d.read_only && !d.loading;
    let cur = d.doc.style_at(d.bytes().start);
    // (The pickers' menus are made as they open, from the document then.)
    fn style_now(d: &DocView) -> Style {
        d.doc.style_at(d.bytes().start)
    }
    let fonts = cx.asks(|d: &mut DocView, _: &(), _| families(Menu::new("Font"), &d.shared.common(), d, &style_now(d)));
    let sizes = cx.asks(|d: &mut DocView, _: &(), _| size_menu(&style_now(d)));
    let colors = cx.asks(move |d: &mut DocView, _: &(), _| color_menu(&style_now(d), ink));
    let family = if cur.family.is_empty() { "System Font".to_owned() } else { cur.family.clone() };
    let color = COLORS.iter().find(|(_, rgb)| *rgb == cur.color).map_or("Other", |(name, _)| name);
    let top = ToolBar::new("tools", "Tools", cx.listener(|d: &mut DocView, command: &str, cx| d.run(command, cx)))
        .command("new", "new-document")
        .command("open", "folder-open")
        .command("save", "save-document")
        .command("print", "printer")
        .separator()
        .command("undo", "undo")
        .command("redo", "redo")
        .separator()
        .command("cut", "cut")
        .command("copy", "copy")
        .command("paste", "paste")
        .separator()
        .command("find", "query")
        .toggle("spelling", "spelling")
        .separator()
        .toggle("bold", "format-bold")
        .toggle("italic", "format-italic")
        .toggle("underline", "format-underline")
        .toggle("strikeout", "format-strikeout")
        .separator()
        .picker("Font", family, 132.0, move |cx| fonts(&(), cx).unwrap_or_else(|| Menu::new("Font")))
        .enabled(editable)
        .picker("Size", format!("{}", cur.size), 52.0, move |cx| sizes(&(), cx).unwrap_or_else(|| Menu::new("Size")))
        .enabled(editable)
        .colors("Color", color, Some(cur.color.map_or(ink, rgb_color)), move |cx| colors(&(), cx).unwrap_or_else(|| Menu::new("Color")))
        .enabled(editable)
        .separator()
        .toggle("align-left", "align-left")
        .toggle("align-center", "align-center")
        .toggle("align-right", "align-right")
        .toggle("align-justify", "align-justify")
        .separator()
        .toggle("list-bullet", "list-bullets")
        .toggle("list-number", "list-numbers")
        .separator()
        .command("indent-less", "indent-less")
        .command("indent-more", "indent-more")
        .rows(2)
        .menus(menus);
    let side = ToolBar::new("insert", "Insert", cx.listener(|d: &mut DocView, command: &str, cx| d.run(command, cx)))
        .vertical()
        .command("insert-picture", "image")
        .command("table:insert", "insert-table")
        .command("margins", "header-footer")
        .separator()
        .toggle("page-view", "page-view")
        .toggle("ruler", "ruler")
        .toggle("wrap", "wrap-lines")
        .menus(menus);
    (top, side)
}

/// The menus a document's window has (`ink`: the document's text colour,
/// for Font ▸ Color's Black).
fn doc_menus(app: &Common, d: &DocView, ink: olo_kit::Hsla) -> Vec<Menu> {
    let has_sel = !d.bytes().is_empty();
    let editable = !d.read_only && !d.loading;
    // (Its settings: fonts, and which app opens which documents.)
    let file = Menu::new("File")
        .app_items(&["appearance", "default-apps"])
        .item("New", "new")
        .key("n")
        .item("Open…", "open")
        .key("o")
        .separator()
        .item("Save", "save")
        .key("s")
        .enabled(!d.saving && !d.loading && (d.doc.edited() || d.path.is_none()))
        .item("Save As…", "save-as")
        .shift_key("s")
        .enabled(!d.saving && !d.loading)
        .item("Revert to Saved…", "revert")
        .enabled(d.path.is_some() && d.doc.edited() && !d.saving)
        .separator()
        .item("Print…", "print")
        .key("p")
        .separator()
        .item("Import…", "import")
        .submenu(
            Menu::new("Export")
                .item("PDF…", "export:pdf")
                .item("Word Document…", "export:docx")
                .item("OpenDocument Text…", "export:odt")
                .item("Rich Text (RTF)…", "export:rtf")
                .separator()
                .item("Markdown…", "export:md")
                .item("Web Page (HTML)…", "export:html"),
        )
        .separator()
        .item(olo_kit::tracker::LABEL, olo_kit::tracker::COMMAND)
        .enabled(d.path.is_some())
        .close_quit();
    // The picture selected, alone.
    let picked = picture_selected(&d.doc, d.bytes());
    // (Each step named, as every app's: Undo Typing, Redo Bold.)
    let edit = olo_kit::undo::edit_items_named(Menu::new("Edit"), d.doc.undo_name().filter(|_| editable), d.doc.redo_name().filter(|_| editable))
        .separator()
        .item("Cut", "cut")
        .key("x")
        .enabled(editable && has_sel)
        .item("Copy", "copy")
        .key("c")
        .enabled(has_sel)
        .item("Paste", "paste")
        .key("v")
        .enabled(editable)
        .item("Clear", "clear")
        .enabled(editable && has_sel)
        .separator()
        .item("Insert Picture…", "insert-picture")
        .enabled(editable)
        .submenu(all_enabled(Menu::new("Picture Size").item("Fit Page Width", "picture:fit").item("Larger", "picture:larger").item("Smaller", "picture:smaller"), editable && picked.is_some()))
        .separator()
        .item("Select All", "select-all")
        .key("a")
        .separator()
        .item("Find…", "find")
        .key("f")
        .item("Find Again", "find-again")
        .key("g")
        .enabled(!app.search.find.is_empty())
        .item("Find Selection", "find-selection")
        .key("h")
        .enabled(has_sel)
        .item("Replace…", "replace")
        .key("r")
        .enabled(editable)
        .item("Replace Same", "replace-same")
        .key("t")
        .enabled(editable && !app.search.find.is_empty())
        .separator()
        .item("Check Spelling While Typing", "spelling")
        .checked(app.spelling);

    // The style where the selection starts (or what typing will use).
    let cur = d.doc.style_at(d.bytes().start);
    let font = Menu::new("Font")
        .submenu(size_menu(&cur))
        .submenu(color_menu(&cur, ink))
        .separator()
        .item("Bold", "bold")
        .key("b")
        .checked(cur.bold)
        .item("Italic", "italic")
        .key("i")
        .checked(cur.italic)
        .item("Underline", "underline")
        .key("u")
        .checked(cur.underline)
        .item("Strikeout", "strikeout")
        .checked(cur.strikeout)
        .separator();
    let font = families(font, app, d, &cur);
    // The paragraph the selection starts in.
    let para = d.doc.paras()[d.doc.para_index(d.bytes().start)].clone();
    let align = para.align.unwrap_or(d.doc.align);
    let mut kinds = Menu::new("Style");
    for ((k, label, _), command) in doc::Kind::ALL.into_iter().zip(KIND_COMMANDS) {
        kinds = kinds.item(label, command).checked(para.kind == k);
    }
    let paragraph = Menu::new("Paragraph")
        .submenu(kinds)
        .separator()
        .item("Bulleted List", "list-bullet")
        .checked(para.list == doc::List::Bullet)
        .item("Numbered List", "list-number")
        .checked(para.list == doc::List::Number)
        .separator()
        .item("Indent More", "indent-more")
        .enabled(para.level < doc::MAX_LEVEL)
        .item("Indent Less", "indent-less")
        .enabled(para.level > 0)
        .separator()
        .item("Align Left", "align-left")
        .checked(align == Align::Left)
        .item("Align Center", "align-center")
        .checked(align == Align::Center)
        .item("Align Right", "align-right")
        .checked(align == Align::Right)
        .item("Justify", "align-justify")
        .checked(align == Align::Justify);
    // The table the caret is in, if any.
    let here = d.doc.para_index(d.editor.caret().min(d.doc.text().len()));
    let in_table = table::table_of(&d.doc, here);
    let header = in_table.as_ref().is_some_and(|t| d.doc.paras()[t.start].row.as_ref().is_some_and(|r| r.header));
    let one_column = in_table.as_ref().is_none_or(|t| t.clone().all(|j| table::cells(&d.doc, j).len() <= 1));
    let t = in_table.is_some();
    let tables = Menu::new("Table")
        .item("Insert Table", "table:insert")
        .item(if t { "Convert to Text" } else { "Convert to Table" }, if t { "table:text" } else { "table:convert" })
        .separator()
        .item("Header Row", "table:header")
        .checked(header)
        .enabled(t)
        .separator()
        .item("Add Row Above", "table:row-above")
        .enabled(t)
        .item("Add Row Below", "table:row-below")
        .enabled(t)
        .item("Add Column Before", "table:column-before")
        .enabled(t)
        .item("Add Column After", "table:column-after")
        .enabled(t)
        .separator()
        .item("Delete Row", "table:delete-row")
        .enabled(t)
        .item("Delete Column", "table:delete-column")
        .enabled(t && !one_column)
        .item("Delete Table", "table:delete")
        .enabled(t)
        .separator()
        .item("Even Columns", "table:even")
        .enabled(t);
    let styled = d.mode.is_styled();
    let source = d.mode.is_source();
    let mut document = Menu::new("Document");
    if let Mode::Markdown(look) = &d.mode {
        document = document.item("Show Source", "source").key("/").checked(look.source).separator();
    }
    document = document
        .item("Wrap Lines", "wrap")
        .checked(d.doc.wrap)
        .item("Line Numbers", "line-numbers")
        .checked(source && d.line_numbers)
        .enabled(source)
        .item("Show Ruler", "ruler")
        .checked(d.ruler && d.doc.wrap && styled)
        .enabled(d.doc.wrap && styled)
        .item("Page View", "page-view")
        .checked(d.page_view && !source)
        .enabled(!source)
        .item("Show Toolbars", "tool-bars")
        .checked(d.tool_bars && styled)
        .enabled(styled)
        .separator()
        .submenu(language_menu(d))
        .separator()
        .item("Header & Footer…", "margins");
    // Markdown's own Paragraph menu: what its lines' marks make them.
    if matches!(d.mode, Mode::Markdown(_)) {
        let font = Menu::new("Font")
            .item("Bold", "bold")
            .key("b")
            .item("Italic", "italic")
            .key("i")
            .item("Strikeout", "strikeout")
            .item("Code", "md:code")
            .key("e")
            .separator()
            .item("Link", "md:link")
            .key("k");
        let mut kinds = Menu::new("Paragraph").item("Body", "md:body");
        for (label, command) in MD_HEADINGS {
            kinds = kinds.item(label, command);
        }
        let paragraph = kinds.separator().item("Quote", "md:quote").separator().item("Bulleted List", "list-bullet").item("Numbered List", "list-number").item("Task List", "md:tasks");
        return vec![file, edit, all_enabled(font, editable), all_enabled(paragraph, editable), all_enabled(document, editable)];
    }
    if !styled {
        return vec![file, edit, all_enabled(document, editable)];
    }
    // Styles and layout are saved with the file, so a read-only one
    // keeps its own (as Haiku's StyledEdit has it).
    vec![file, edit, all_enabled(font, editable), all_enabled(paragraph, editable), all_enabled(tables, editable), all_enabled(document, editable)]
}

/// Document ▸ Language: what the text is shown as (styled, Markdown, or
/// source code in a language). A document with styles of its own stays
/// styled (they'd be lost).
fn language_menu(d: &DocView) -> Menu {
    let plain = !d.mode.is_styled() || d.doc.is_plain();
    let current = match &d.mode {
        Mode::Styled => "styled",
        Mode::Markdown(_) => "markdown",
        Mode::Code { code, .. } => code.language().id,
    };
    let mut menu =
        Menu::new("Language").item("Styled Text", "language:styled").checked(current == "styled").item("Markdown", "language:markdown").checked(current == "markdown").enabled(plain).separator();
    for l in &code::syntax::LANGUAGES {
        menu = menu.item(l.name, format!("language:{}", l.id)).checked(current == l.id).enabled(plain);
    }
    menu
}

/// The text's changes since, given to what follows them: source code's
/// syntax, a big Markdown document's lines.
fn sync_mode(mode: &mut Mode, doc: &mut Doc) {
    match mode {
        Mode::Code { code, .. } => {
            for e in doc.take_edits() {
                code.edit(&e);
            }
        }
        Mode::Markdown(look) => {
            for e in doc.take_edits() {
                look.edited(e.removed(), e.inserted().len());
            }
        }
        Mode::Styled => {}
    }
}

/// A Markdown document's look, editor, text and generation (for
/// `markdown::parse` to find them again).
fn markdown_of(d: &mut DocView) -> Option<(&mut markdown::Look, &mut Editor, &str, u64)> {
    match &mut d.mode {
        Mode::Markdown(look) => Some((look, &mut d.editor, d.doc.text(), d.doc.generation)),
        _ => None,
    }
}

/// A document's source code and text, if it's source code (for
/// `code::parse` to find them again).
fn code_of(d: &mut DocView) -> Option<(&mut Code, &str)> {
    match &mut d.mode {
        Mode::Code { code, .. } => Some((code, d.doc.text())),
        _ => None,
    }
}

/// `menu` with every item (and its submenus') enabled only if `on`.
fn all_enabled(mut menu: Menu, on: bool) -> Menu {
    for item in &mut menu.items {
        match item {
            MenuItem::Action(a) => a.enabled &= on,
            MenuItem::Submenu(m) => *m = all_enabled(std::mem::take(m), on),
            MenuItem::Separator => {}
        }
    }
    menu
}

impl View for DocView {
    type Message = DocMsg;

    fn message(&mut self, message: DocMsg, cx: &mut Cx<Self>) {
        match message {
            DocMsg::Loaded(path, result) => self.loaded(path, result, cx),
            DocMsg::Saved(generation, path, result) => self.saved(generation, path, result, cx),
            DocMsg::OnDisk(paths) => {
                // Written by another program (not our own save): read again
                // if nothing here is unsaved, keeping the place in it.
                let Some(path) = self.path.clone().filter(|p| paths.contains(p)) else { return };
                let now = stamp(&path);
                if now.is_none() || now == self.on_disk || self.doc.edited() || self.saving || self.loading {
                    return;
                }
                self.on_disk = now;
                self.restore = Some((self.editor.selection(), self.editor.view()));
                self.load(path, cx);
            }
            DocMsg::Picture(path, px) => {
                let Some(px) = px else {
                    return self.error(format!("“{}” isn't a picture StyledEdit can read.", file_name(&path)), cx);
                };
                let (width, height) = pictures::fit(px, PICTURE_ROOM.0, PICTURE_ROOM.1);
                let alt = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let sel = self.bytes();
                let pic = doc::Picture { path: path.to_string_lossy().into_owned(), width, height, alt };
                let change = self.doc.insert_picture(sel.clone(), pic, sel.clone());
                let after = sel.start + change.inserted;
                self.changed(change, after..after);
                cx.invalidate();
            }
            DocMsg::PictureLoaded(l) => {
                self.pictures.borrow_mut().loaded(l);
                self.editor.relayout();
                cx.invalidate();
            }
            DocMsg::Exported(path, result) => {
                if let Err(e) = result {
                    self.error(format!("“{}” couldn't be written. {e}", file_name(&path)), cx);
                }
            }
            DocMsg::Printed(title, result) => {
                if let Err(e) = result {
                    self.error(format!("“{title}” couldn't be printed. {e}"), cx);
                }
            }
            DocMsg::Chosen(purpose, path) => self.chosen(purpose, path, cx),
            DocMsg::Print(choice) => {
                let Some(choice) = choice else { return };
                match choice.printer.clone() {
                    // A PDF: where to, first.
                    None => {
                        self.pdf_setup = Some((choice.paper, choice.pages));
                        self.ask_panel(Purpose::Export(Format::Pdf), cx);
                    }
                    Some(printer) => self.print(printer, choice, cx),
                }
            }
            DocMsg::Leave => self.leave(After::Close, cx),
            DocMsg::Close => cx.close_window(),
            DocMsg::Again(respell) => {
                if respell {
                    self.editor.respell();
                }
                cx.invalidate();
            }
        }
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// The window's close: asks about unsaved changes first.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        if self.doc.edited() {
            self.leave(After::Close, cx);
            return false;
        }
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.saving_said.store(false, Ordering::Relaxed);
        to_hub(ToHub::Closed(self.id));
    }

    /// The document, kept for an upgrade (unsaved work too).
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        let d = self;
        let dirty = d.doc.edited() || d.path.is_none();
        serde_json::to_string(&SavedDoc {
            path: d.path.clone(),
            untitled: d.untitled,
            edited: dirty.then(|| (d.doc.text().to_owned(), d.doc.runs().to_vec())),
            paras: if dirty { d.doc.paras().to_vec() } else { Vec::new() },
            align: d.doc.align,
            wrap: d.doc.wrap,
            selection: (d.bytes().start, d.bytes().end),
            view: d.editor.view(),
            ruler: d.ruler,
            page_view: d.page_view,
            tool_bars: d.tool_bars,
            page: d.doc.page.clone(),
            mode: Some(d.mode.id()),
            line_numbers: d.line_numbers,
        })
        .ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // The editor has the keyboard (else the window's frame does).
        self.editor.focus_handle().focus_if_none(cx);
        self.saving_said.store(self.saving, Ordering::Relaxed);
        let p = theme::palette();
        let ink = style::c(p.document_text);
        let (menus, spelling) = {
            let common = self.shared.common();
            (doc_menus(&common, self, ink), common.spelling)
        };
        let styled = self.mode.is_styled();
        let source = self.mode.is_source();
        // (Styles are a styled document's: its tool bars and ruler too.)
        let bars = (self.tool_bars && styled).then(|| tool_bars(cx, self, &menus, ink));
        let bar = MenuBar::new(cx, menus);
        let editable = !self.read_only && !self.loading;
        // Markdown: the lines the selection is on show their marks.
        if let Mode::Markdown(look) = &mut self.mode {
            look.set_reveal(self.editor.selection());
        }
        // Source code (a big Markdown document): parsed on a worker as it
        // changes.
        match &mut self.mode {
            Mode::Code { code, .. } => code::parse(cx, code, self.doc.text(), code_of),
            Mode::Markdown(look) => {
                look.sync(self.doc.text(), self.doc.generation);
                markdown::parse(cx, look, self.doc.text(), self.doc.generation, markdown_of);
            }
            Mode::Styled => {}
        }
        let options = editor::Options {
            editable,
            wrap: self.doc.wrap,
            align: if styled { self.doc.align.to_kit() } else { editor::Align::Left },
            justify: styled && self.doc.align == Align::Justify,
            margin: (m::TEXT_INSET, 4.0),
            background: Some(p.document_background),
            spelling: spelling && !source,
            // On the paper it prints on, with the margins it prints with.
            pages: (self.page_view && self.doc.wrap && !source).then(|| {
                let paper = olo_print::local_paper();
                editor::Pages { paper: (paper.width, paper.height), margin: pages::MARGIN, gap: PAGE_GAP }
            }),
            line_numbers: (source && self.line_numbers).then_some(markdown::CODE * 0.9),
        };
        // The ruler, over the text (when it wraps: the text is then the
        // view's width), showing the caret's paragraph's indents and tabs.
        let show_ruler = self.ruler && self.doc.wrap && styled;
        if show_ruler {
            // (Where the text was last drawn; drawn again when it moves.)
            let (origin, width) = self.editor.text_span().unwrap_or((m::TEXT_INSET, 400.0));
            let here = self.doc.para_index(self.editor.caret().min(self.doc.text().len()));
            let setting = self.doc.paras()[here].ruler();
            self.ruler_view.update(cx, |r, cx| r.set(&setting, origin, width, cx));
        }
        self.guide = self.ruler_view.read(cx).guide().filter(|_| show_ruler);
        let status = self.status();
        let body = editor::view(self, 0, options, cx);
        // Over the text: the header and footer on each sheet, as printed,
        // and where a marker held on the ruler goes.
        let overlay = cx
            .canvas(|d: &DocView, g, b| {
                let p = theme::palette();
                let (sheets, n) = d.editor.sheets();
                let ink = style::c(p.document_text).opacity(0.6);
                g.with_content_mask(Some(olo_kit::ContentMask { bounds: b }), |g| {
                    for (k, sheet) in sheets {
                        for (i, (text, y)) in pages::margin_lines(d.doc.page.as_ref(), *k, n).into_iter().enumerate() {
                            if text.trim().is_empty() {
                                continue;
                            }
                            let (x, align) = match i % 3 {
                                0 => (sheet.left() + px(pages::MARGIN), style::Align::Left),
                                1 => (sheet.center().x, style::Align::Center),
                                _ => (sheet.right() - px(pages::MARGIN), style::Align::Right),
                            };
                            // (Its baseline where print has it.)
                            let center = sheet.top() + px(y - pages::MARGIN_TEXT * style::CAP_HEIGHT / 2.0);
                            style::label(g, x, center, align, text, pages::MARGIN_TEXT, style::ui_font(FontWeight::NORMAL), ink);
                        }
                    }
                    if let Some(x) = d.guide {
                        style::vline(g, px(x), b.top(), b.bottom(), style::c(p.keyboard_navigation));
                    }
                });
            })
            .absolute()
            .inset_0();
        let status_line = canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
            style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.3));
            // Centred under the line along the top.
            let y = b.center().y + style::hairline(g) / 2.0;
            let at = style::label(g, b.left() + px(m::TIGHT + 2.0), y, style::Align::Left, status.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
            olo_kit::agent::record(g, olo_kit::agent::Node::label(status.clone(), at));
        })
        .w_full()
        .h(px(STATUS_H));
        // The side bar as tall as what's between the top one and the
        // status line.
        let (top, side) = match bars {
            Some((top, side)) => {
                let view = cx.viewport_size();
                let below = olo_kit::menu::BAR_HEIGHT + top.extent(f32::from(view.width)) + STATUS_H;
                let side = side.length(f32::from(view.height) - below);
                (Some(top), Some(side))
            }
            None => (None, None),
        };
        let scale = cx.scale_factor();
        let ruler_h = (ruler::HEIGHT * scale).round() / scale;
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_drop(cx.listener(|d: &mut DocView, paths: &ExternalPaths, cx| d.dropped(paths.paths().to_vec(), cx)))
            .child(bar)
            .children(top)
            .child(
                div().flex_1().min_h_0().flex().children(side).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(show_ruler.then(|| div().w_full().h(px(ruler_h)).child(&self.ruler_view)))
                        .child(div().flex_1().min_h_0().relative().child(body).child(overlay)),
                ),
            )
            .child(status_line)
            .into_any_element()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Purpose {
    Open,
    SaveAs,
    /// A formatted document (Markdown) in as a new one.
    Import,
    /// A copy in another format.
    Export(Format),
    /// A picture to put in.
    Picture,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Format {
    Markdown,
    Html,
    Word,
    OpenDocument,
    Rtf,
    Pdf,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Word => "docx",
            Format::OpenDocument => "odt",
            Format::Rtf => "rtf",
            Format::Pdf => "pdf",
        }
    }
}

/// The Find and Replace windows' settings, shared by every document.
#[derive(Default, Clone)]
struct Search {
    find: String,
    replace: String,
    case: bool,
    wrap: bool,
    backwards: bool,
    all: bool,
}

/// What every document's window reads and sets.
struct Common {
    search: Search,
    /// Misspelled words are underlined (Edit ▸ Check Spelling While
    /// Typing).
    spelling: bool,
    fonts: fonts::Fonts,
}

/// What the app and its windows (each on its own looper) share.
struct Shared {
    common: Mutex<Common>,
    /// New windows show the ruler (as the last one shown or hidden).
    ruler: AtomicBool,
    /// New windows show the tool bars (as the last one shown or hidden).
    tool_bars: AtomicBool,
    /// Each window's "a save is under way" (an upgrade waits for them).
    saving: Mutex<Vec<Weak<AtomicBool>>>,
}

impl Shared {
    fn common(&self) -> MutexGuard<'_, Common> {
        self.common.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A window open, as the app knows it.
struct Open {
    id: u64,
    path: Option<PathBuf>,
    window: WindowHandle<DocView>,
}

/// The app's looper: the windows open and the files they show, followed
/// as they change on disk.
struct Hub {
    docs: Vec<Open>,
    shared: Arc<Shared>,
    untitled: usize,
    /// The open documents' files, followed as they change: one changed
    /// by another program is read again if it has no unsaved changes here.
    watch: Option<olo_kit::watch::Watch>,
    next: u64,
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
    #[serde(default = "shown")]
    ruler: bool,
    #[serde(default)]
    page: Option<doc::Page>,
    #[serde(default)]
    page_view: bool,
    #[serde(default = "shown")]
    tool_bars: bool,
    /// How it showed (`Mode::id`): None, as its kind has it.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default = "shown")]
    line_numbers: bool,
}

fn shown() -> bool {
    true
}

/// Pages `range` (from 1, inclusive) of `pages`, or all of them.
fn chosen(pages: Vec<olo_print::Page>, range: Option<(u32, u32)>) -> Vec<olo_print::Page> {
    match range {
        Some((a, b)) => pages.into_iter().skip(a.saturating_sub(1) as usize).take((b.saturating_sub(a) + 1) as usize).collect(),
        None => pages,
    }
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

fn to_hub(message: ToHub) {
    olo_kit::proxy().send(message);
}

/// Says something went wrong, with no window to say it over.
fn app_error(message: String) {
    let _ = open_app_alert("StyledEdit", "warning", &message, &["OK"], |_| {});
}

impl Hub {
    fn new_doc(&mut self) {
        self.untitled += 1;
        let n = self.untitled;
        self.add_window(None, n, |_, _| {});
    }

    /// A window for a document (`setup` gives it its text, or starts its
    /// file's reading).
    fn add_window(&mut self, path: Option<PathBuf>, untitled: usize, setup: impl FnOnce(&mut DocView, &mut Cx<DocView>) + Send + 'static) {
        self.next += 1;
        let (id, shared, opened) = (self.next, self.shared.clone(), path.clone());
        let title = match &path {
            Some(p) => file_name(p),
            None => format!("Untitled {untitled}"),
        };
        let window = olo_kit::open_window(WindowOptions::new(title, 560.0, 440.0).min_size(280.0, 180.0).keep_size("document"), move |cx| {
            let mut d = DocView::new(id, shared, opened, untitled, cx);
            setup(&mut d, cx);
            d
        });
        self.docs.push(Open { id, path, window });
        self.follow_files();
    }

    /// Follow the open documents' files (as they are now).
    fn follow_files(&mut self) {
        let paths: Vec<PathBuf> = self.docs.iter().filter_map(|d| d.path.clone()).collect();
        match &self.watch {
            Some(w) => w.set(paths),
            None => self.watch = olo_kit::watch::watch(paths, |changed| to_hub(ToHub::OnDisk(changed.to_vec()))),
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        // Word, OpenDocument and RTF aren't plain text: in as a new
        // document.
        if olo_richtext::format_of(&path).is_some() {
            return self.import(path);
        }
        self.docs.retain(|d| d.window.is_open());
        if let Some(d) = self.docs.iter().find(|d| d.path.as_ref() == Some(&path)) {
            olo_kit::raise(Some(&d.window));
            return;
        }
        // A folder (dropped, named): said so, no window opened for it.
        if path.is_dir() {
            return app_error(format!("“{}” is a folder: StyledEdit opens files.", file_name(&path)));
        }
        let file = path.clone();
        self.add_window(Some(path), 0, move |d, cx| d.load(file, cx));
    }

    /// A formatted document read in (on a worker) as a new, untitled one:
    /// saving it writes a StyledEdit file, never over what it came from.
    fn import(&mut self, path: PathBuf) {
        std::thread::spawn(move || {
            let r = match olo_richtext::format_of(&path) {
                Some(_) => olo_richtext::read(&path).map(|doc| {
                    let page = convert::page_of(&doc);
                    let (text, runs, paras) = convert::from_rich(doc);
                    (text, runs, paras, page)
                }),
                None => std::fs::read(&path).map_err(|e| e.to_string()).map(|bytes| {
                    let (text, runs, paras) = convert::from_markdown_in(&String::from_utf8_lossy(&bytes), path.parent());
                    (text, runs, paras, None)
                }),
            };
            to_hub(ToHub::Imported(path, Box::new(r)));
        });
    }

    fn handle_args(&mut self, args: &[String], cwd: Option<&Path>) {
        let mut opened = false;
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(p);
            opened = true;
        }
        self.docs.retain(|d| d.window.is_open());
        // Tracker's New Window: always a new document.
        if !opened && args.iter().any(|a| a == "--new-window") {
            self.new_doc();
        } else if !opened {
            // A document there is, else a new one.
            match self.docs.first() {
                Some(d) => {
                    olo_kit::raise(Some(&d.window));
                }
                None => self.new_doc(),
            }
        }
    }

    fn tell_all(&mut self, message: impl Fn() -> DocMsg) {
        self.docs.retain(|d| d.window.is_open());
        for d in &self.docs {
            d.window.messenger().send(message());
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args, launch.cwd.as_deref()),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<ToHub>() else { return };
        match *msg {
            ToHub::Fonts(fonts::Event::Families(f)) => {
                let mut common = self.shared.common();
                common.fonts.groups = family_groups(&f);
                common.fonts.families = f;
                drop(common);
                self.tell_all(|| DocMsg::Again(false));
            }
            ToHub::Open(path) => self.open_file(path),
            ToHub::New => self.new_doc(),
            ToHub::Import(path) => self.import(path),
            ToHub::Imported(path, result) => match *result {
                Ok((text, runs, paras, page)) => {
                    self.untitled += 1;
                    let n = self.untitled;
                    self.add_window(None, n, move |d, _| {
                        let mut doc = Doc::with_paras(text, Some(runs), Some(paras));
                        doc.page = page;
                        // Not saved anywhere yet.
                        doc.saved_generation = 0;
                        d.reset(doc, 0..0);
                    });
                }
                Err(e) => app_error(format!("“{}” couldn't be imported. {e}", file_name(&path))),
            },
            ToHub::Showing(id, path) => {
                if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
                    d.path = path;
                }
                self.follow_files();
            }
            // A file that couldn't be read: said (first: the app stays
            // for it), and its window goes.
            ToHub::Failed(id, message) => {
                app_error(message);
                if let Some(d) = self.docs.iter().find(|d| d.id == id) {
                    d.window.messenger().send(DocMsg::Close);
                }
            }
            ToHub::Closed(id) => {
                self.docs.retain(|d| d.id != id);
                self.follow_files();
            }
            ToHub::OnDisk(paths) => self.tell_all(|| DocMsg::OnDisk(paths.clone())),
            ToHub::Again(respell) => self.tell_all(|| DocMsg::Again(respell)),
            ToHub::Quit => self.tell_all(|| DocMsg::Leave),
        }
    }

    fn restore(&mut self, docs: Vec<SavedDoc>) {
        for s in docs {
            self.untitled = self.untitled.max(s.untitled);
            match s.edited {
                Some((text, runs)) => {
                    let path = s.path.clone();
                    self.add_window(s.path, s.untitled, move |d, _| {
                        d.ruler = s.ruler;
                        d.page_view = s.page_view;
                        d.tool_bars = s.tool_bars;
                        d.line_numbers = s.line_numbers;
                        let styled = runs.iter().any(|r| r.style != Style::default()) || !s.paras.is_empty();
                        d.mode = s.mode.as_deref().and_then(|m| Mode::named(m, &text)).unwrap_or_else(|| Mode::for_file(path.as_deref().unwrap_or(Path::new("")), &text, styled));
                        let mut doc = Doc::with_paras(text, Some(runs), Some(s.paras).filter(|p| !p.is_empty()));
                        doc.align = s.align;
                        doc.wrap = s.wrap;
                        doc.page = s.page;
                        // Unsaved work stays unsaved (an empty new one is clean).
                        if !(doc.text().is_empty() && path.is_none()) {
                            doc.saved_generation = 0;
                        }
                        d.reset(doc, s.selection.0..s.selection.1);
                        d.editor.set_view(d.doc.text(), s.view);
                    });
                }
                None => {
                    let Some(path) = s.path else { continue };
                    let file = path.clone();
                    self.add_window(Some(path), s.untitled, move |d, cx| {
                        d.ruler = s.ruler;
                        d.page_view = s.page_view;
                        d.tool_bars = s.tool_bars;
                        d.line_numbers = s.line_numbers;
                        // (As it showed; its text, read, decides the rest.)
                        if let Some(m) = s.mode.as_deref().and_then(|m| Mode::named(m, "")) {
                            d.mode = m;
                        }
                        d.restore = Some((s.selection.0..s.selection.1, s.view));
                        d.load(file, cx);
                    });
                }
            }
        }
    }
}

/// What the document's window does with its file, its panels and its
/// tool windows.
impl DocView {
    fn error(&self, message: String, cx: &mut Cx<Self>) {
        let _ = open_alert(cx, "StyledEdit", "warning", &message, &["OK"], |_, _| {});
    }

    /// Reads `path` (on a worker) as this window's document.
    fn load(&mut self, path: PathBuf, cx: &mut Cx<Self>) {
        self.loading = true;
        cx.invalidate();
        let to = cx.messenger();
        std::thread::spawn(move || {
            let r = doc::load(&path);
            to.send(DocMsg::Loaded(path, r));
        });
    }

    fn loaded(&mut self, path: PathBuf, result: Result<doc::Loaded, String>, cx: &mut Cx<Self>) {
        if self.path.as_ref() != Some(&path) {
            return;
        }
        self.loading = false;
        cx.invalidate();
        match result {
            Ok(l) => {
                // How it shows: by its kind (Markdown and code as they
                // were, when it's read again).
                let styled = l.runs.is_some() || l.paras.is_some();
                self.mode = match (std::mem::replace(&mut self.mode, Mode::Styled), Mode::for_file(&path, &l.text, styled)) {
                    (Mode::Markdown(old), Mode::Markdown(_)) => Mode::Markdown(markdown::Look::new(old.source)),
                    (Mode::Code { code, .. }, _) if !styled => Mode::code(code.language(), &l.text),
                    (_, m) => m,
                };
                let mut doc = Doc::with_paras(l.text, l.runs, l.paras);
                doc.align = l.align;
                doc.wrap = l.wrap;
                doc.page = l.page;
                let (select, view) = self.restore.take().unwrap_or_default();
                self.reset(doc, select);
                self.editor.set_view(self.doc.text(), view);
                self.latin1 = l.latin1;
                self.read_only = l.read_only;
                self.on_disk = stamp(&path);
            }
            // A new file: it's made on the first save.
            Err(_) if !path.exists() => {}
            Err(e) => to_hub(ToHub::Failed(self.id, format!("“{}” couldn't be opened. {e}", file_name(&path)))),
        }
    }

    /// Write the document to `to` (its own file when None, or ask where).
    fn save(&mut self, to: Option<PathBuf>, after: After, cx: &mut Cx<Self>) {
        let Some(path) = to.or_else(|| self.path.clone().filter(|_| !self.read_only)) else {
            self.after_save = after;
            return self.ask_panel(Purpose::SaveAs, cx);
        };
        // (Source code and Markdown are their text alone.)
        let d = &*self;
        let styled = d.mode.is_styled();
        let (text, runs, paras, align, wrap, page, generation) = (
            d.doc.text().to_owned(),
            if styled { d.doc.runs().to_vec() } else { Vec::new() },
            if styled { d.doc.paras().to_vec() } else { Vec::new() },
            if styled { d.doc.align } else { Align::Left },
            d.doc.wrap,
            d.doc.page.clone(),
            d.doc.generation,
        );
        self.saving = true;
        self.saving_said.store(true, Ordering::Relaxed);
        self.after_save = after;
        cx.invalidate();
        let to = cx.messenger();
        std::thread::spawn(move || {
            let r = doc::save(&path, &text, &runs, &paras, align, wrap, page.as_ref());
            to.send(DocMsg::Saved(generation, path, r));
        });
    }

    fn saved(&mut self, generation: u64, path: PathBuf, result: Result<(), String>, cx: &mut Cx<Self>) {
        self.saving = false;
        self.saving_said.store(false, Ordering::Relaxed);
        cx.invalidate();
        let after = std::mem::replace(&mut self.after_save, After::Nothing);
        match result {
            Ok(()) => {
                self.on_disk = stamp(&path);
                if self.path.as_ref() != Some(&path) {
                    self.path = Some(path.clone());
                    self.read_only = false;
                    self.set_title(cx);
                    // Plain text saved as Markdown or code shows as that now.
                    if self.mode.is_styled() && self.doc.is_plain() {
                        let mode = Mode::for_file(&path, self.doc.text(), false);
                        if !mode.is_styled() {
                            self.set_mode(mode);
                        }
                    }
                }
                self.latin1 = false;
                self.doc.saved_generation = generation;
                to_hub(ToHub::Showing(self.id, self.path.clone()));
                self.finish(after, cx);
            }
            Err(e) => self.error(format!("“{}” couldn't be saved. {e}", file_name(&path)), cx),
        }
    }

    /// Close or revert, asking about unsaved changes.
    fn leave(&mut self, after: After, cx: &mut Cx<Self>) {
        if !self.doc.edited() {
            return self.finish(after, cx);
        }
        if self.asking {
            return cx.activate_window();
        }
        let title = self.title();
        let doing = match after {
            After::Revert => "reverting",
            _ => "closing",
        };
        self.asking = true;
        let doc = cx.view_ref();
        let _ = ask_to_save(cx, "StyledEdit", &title, doing, move |choice, cx| {
            let choice = *choice;
            doc.update(cx, move |d, cx| {
                d.asking = false;
                match choice {
                    Some(Unsaved::DontSave) => d.finish(after, cx),
                    Some(Unsaved::Save) => d.save(None, after, cx),
                    None => {}
                }
            });
        });
    }

    fn finish(&mut self, after: After, cx: &mut Cx<Self>) {
        match after {
            After::Nothing => {}
            // (The app hears as the window goes: `window_closing`.)
            After::Close => cx.close_window(),
            After::Revert => {
                if let Some(p) = self.path.clone() {
                    self.load(p, cx);
                }
            }
        }
    }

    /// A file panel for this document: what's chosen comes back to it
    /// (`DocMsg::Chosen`).
    fn ask_panel(&self, purpose: Purpose, cx: &mut Cx<Self>) {
        let title = self.title();
        let dir = self.path.as_deref().and_then(Path::parent).map_or_else(home, Path::to_path_buf);
        let (name, panel) = match purpose {
            Purpose::Open => ("Open", FilePanel::open(dir)),
            Purpose::SaveAs => ("Save As", FilePanel::save(dir, title)),
            Purpose::Import => (
                "Import",
                FilePanel::open(dir).filters(
                    vec![
                        Filter::new("All Documents").glob("*.md").glob("*.markdown").glob("*.mdown").glob("*.docx").glob("*.odt").glob("*.rtf").mime("text/markdown").mime("text/rtf"),
                        Filter::new("Markdown").glob("*.md").glob("*.markdown").glob("*.mdown").mime("text/markdown"),
                        Filter::new("Word").glob("*.docx"),
                        Filter::new("OpenDocument Text").glob("*.odt"),
                        Filter::new("Rich Text").glob("*.rtf").mime("text/rtf"),
                    ],
                    0,
                ),
            ),
            Purpose::Picture => (
                "Insert Picture",
                FilePanel::open(dir).filters(
                    vec![
                        Filter::new("Pictures")
                            .glob("*.png")
                            .glob("*.jpg")
                            .glob("*.jpeg")
                            .glob("*.gif")
                            .glob("*.webp")
                            .glob("*.bmp")
                            .glob("*.tif")
                            .glob("*.tiff")
                            .glob("*.heic")
                            .glob("*.avif")
                            .glob("*.jxl")
                            .mime("image/"),
                    ],
                    0,
                ),
            ),
            Purpose::Export(f) => {
                // The document's name, in the format's extension.
                let stem = Path::new(&title).file_stem().map_or(title.clone(), |s| s.to_string_lossy().into_owned());
                ("Export", FilePanel::save(dir, format!("{stem}.{}", f.extension())))
            }
        };
        let to = cx.messenger();
        file_panel::show(name, panel, move |answer| {
            to.send(DocMsg::Chosen(
                purpose,
                match answer.result {
                    PanelResult::Chosen(path) => Some(path),
                    _ => None,
                },
            ));
        });
    }

    /// What a file panel of this document's chose (None: cancelled).
    fn chosen(&mut self, purpose: Purpose, path: Option<PathBuf>, cx: &mut Cx<Self>) {
        let Some(path) = path else {
            // Cancelling a Save As drops what was to follow it.
            if !self.saving {
                self.after_save = After::Nothing;
            }
            return;
        };
        match purpose {
            Purpose::Open => to_hub(ToHub::Open(path)),
            Purpose::SaveAs => {
                let after = self.after_save;
                self.save(Some(path), after, cx);
            }
            Purpose::Picture => self.put_picture(path, cx),
            Purpose::Import => to_hub(ToHub::Import(path)),
            Purpose::Export(f) => self.export(path, f, cx),
        }
    }

    /// What a worker needs to set the document on pages (as its window
    /// draws it).
    fn page_input(&self) -> (pages::Input, String) {
        let d = self;
        let input = pages::Input {
            text: d.doc.text().to_owned(),
            runs: d.doc.runs().to_vec(),
            paras: d.doc.paras().to_vec(),
            align: d.doc.align,
            page: d.doc.page.clone(),
            text_system: olo_kit::text_system(),
            source: d.source(),
        };
        (input, d.title())
    }

    /// Print the document on `printer` (set, made a PDF and sent on a
    /// worker).
    fn print(&mut self, printer: String, choice: olo_print::panel::Choice, cx: &mut Cx<Self>) {
        let (input, title) = self.page_input();
        let to = cx.messenger();
        std::thread::spawn(move || {
            let pages = chosen(pages::pages(input, choice.paper), choice.pages);
            let r = olo_print::pdf(&pages, &title).and_then(|pdf| {
                let job = olo_print::ipp::Job { name: title.clone(), copies: choice.copies, media: Some(choice.paper.pwg.into()), pages: None, two_sided: false };
                olo_print::ipp::print(&printer, &pdf, &job)
            });
            to.send(DocMsg::Printed(title, r.map(|_| ())));
        });
    }

    /// A copy of the document in `format` at `to` (on a worker); the
    /// document stays as it is, and where.
    fn export(&mut self, to: PathBuf, format: Format, cx: &mut Cx<Self>) {
        let done = cx.messenger();
        if format == Format::Pdf {
            let (paper, range) = self.pdf_setup.take().unwrap_or((olo_print::local_paper(), None));
            let (input, title) = self.page_input();
            std::thread::spawn(move || {
                let pages = chosen(pages::pages(input, paper), range);
                let r = olo_print::pdf(&pages, &title).and_then(|pdf| std::fs::write(&to, pdf).map_err(|e| e.to_string()));
                done.send(DocMsg::Exported(to, r));
            });
            return;
        }
        let d = &*self;
        let (mut text, mut runs, mut paras, mut align, title) = (d.doc.text().to_owned(), d.doc.runs().to_vec(), d.doc.paras().to_vec(), d.doc.align, d.title());
        let page = d.doc.page.clone();
        let source = d.source();
        std::thread::spawn(move || {
            // Markdown as Markdown is its own text, as written.
            let own = matches!(source, Some(convert::Source::Markdown(_))) && format == Format::Markdown;
            if let Some(source) = source.as_ref().filter(|_| !own) {
                (text, runs, paras) = convert::styled_copy(&text, source);
                align = Align::Left;
            }
            let rich = || convert::with_page(convert::to_rich(&text, &runs, &paras, align), page.as_ref());
            let out = match format {
                Format::Markdown if own => Ok(text.clone().into_bytes()),
                Format::Markdown => Ok(convert::to_markdown(&text, &runs, &paras).into_bytes()),
                Format::Html => Ok(convert::to_html(&text, &runs, &paras, &title, align).into_bytes()),
                Format::Word => olo_richtext::write_docx(&rich()),
                Format::OpenDocument => olo_richtext::write_odt(&rich()),
                Format::Rtf => olo_richtext::write_rtf(&rich()),
                Format::Pdf => unreachable!("made above"),
            };
            let r = out.and_then(|bytes| std::fs::write(&to, bytes).map_err(|e| e.to_string()));
            done.send(DocMsg::Exported(to, r));
        });
    }

    /// The Find or Replace window for this document (one each).
    fn open_find(&mut self, replace: bool, cx: &mut Cx<Self>) {
        let k = usize::from(replace);
        if let Some(w) = &self.finds[k]
            && w.is_open(cx)
        {
            w.with(cx, |cx| cx.activate_window());
            return;
        }
        let (title, h) = if replace { ("Replace", REPLACE_H) } else { ("Find", FIND_H) };
        let (doc, shared) = (cx.view_ref(), self.shared.clone());
        self.finds[k] = olo_kit::open_window_here(cx, WindowOptions::new(title, FIND_W, h).min_size(FIND_W, h).child(), move |cx| FindView::new(shared, doc, replace, cx));
    }

    /// Document ▸ Header & Footer… (one window).
    fn open_margins(&mut self, cx: &mut Cx<Self>) {
        if let Some(w) = &self.margins
            && w.is_open(cx)
        {
            w.with(cx, |cx| cx.activate_window());
            return;
        }
        // Pages numbered at the foot until it's set otherwise.
        let page = self.doc.page.clone().unwrap_or_else(|| doc::Page { footer: [String::new(), "{page}".into(), String::new()], ..doc::Page::default() });
        let doc = cx.view_ref();
        self.margins = olo_kit::open_window_here(cx, WindowOptions::new("Header & Footer", MARGINS_W, MARGINS_H).dialog(), move |cx| MarginsView::new(doc, &page, cx));
    }

    /// Put the picture at `path` in the document (at its selection), its
    /// size read on a worker.
    fn put_picture(&mut self, path: PathBuf, cx: &mut Cx<Self>) {
        let to = cx.messenger();
        std::thread::spawn(move || {
            let px = pictures::pixels(&path);
            to.send(DocMsg::Picture(path, px));
        });
        cx.activate_window();
    }

    /// Find the search text from the selection; false if it isn't there.
    fn find_next(&mut self, cx: &mut Cx<Self>) -> bool {
        let s = self.shared.common().search.clone();
        let sel = self.bytes();
        let from = if s.backwards { sel.start } else { sel.end };
        let found = doc::find(self.doc.text(), &s.find, from, s.backwards, s.case, s.wrap);
        if let Some(r) = &found {
            self.select_bytes(r.clone());
            cx.invalidate();
        }
        found.is_some()
    }

    /// Replace the next match (or all of them), selecting what went in.
    fn replace(&mut self, cx: &mut Cx<Self>) {
        let s = self.shared.common().search.clone();
        if self.read_only || s.find.is_empty() {
            return;
        }
        let selection = self.bytes();
        if s.all {
            let ranges = doc::find_all(self.doc.text(), &s.find, s.case);
            if let Some(change) = self.doc.replace_all(&ranges, &s.replace, selection.clone()) {
                let at = selection.start.min(self.doc.text().len());
                self.changed(change, at..at);
            }
            cx.invalidate();
            return;
        }
        // The selection itself if it's a match, else the next one.
        let from = if s.backwards { selection.end } else { selection.start };
        let Some(r) = doc::find(self.doc.text(), &s.find, from, s.backwards, s.case, s.wrap) else { return };
        let change = self.doc.replace(r.clone(), &s.replace, selection);
        self.changed(change, r.start..r.start + s.replace.len());
        cx.invalidate();
    }

    /// A command from the menus, the tool bars or the text's right-click
    /// menu.
    fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
        match command {
            "new" => to_hub(ToHub::New),
            "open" => self.ask_panel(Purpose::Open, cx),
            "import" => self.ask_panel(Purpose::Import, cx),
            "export:pdf" => self.ask_panel(Purpose::Export(Format::Pdf), cx),
            "export:md" => self.ask_panel(Purpose::Export(Format::Markdown), cx),
            "export:html" => self.ask_panel(Purpose::Export(Format::Html), cx),
            "export:docx" => self.ask_panel(Purpose::Export(Format::Word), cx),
            "export:odt" => self.ask_panel(Purpose::Export(Format::OpenDocument), cx),
            "export:rtf" => self.ask_panel(Purpose::Export(Format::Rtf), cx),
            "insert-picture" => self.ask_panel(Purpose::Picture, cx),
            "print" => {
                let to = cx.messenger();
                olo_print::panel::kit::show(olo_print::panel::kit::window_options("Print"), None, move |choice| {
                    to.send(DocMsg::Print(choice));
                });
            }
            "margins" => self.open_margins(cx),
            "save" => self.save(None, After::Nothing, cx),
            "save-as" => self.ask_panel(Purpose::SaveAs, cx),
            "close" => self.leave(After::Close, cx),
            "quit" => to_hub(ToHub::Quit),
            olo_kit::tracker::COMMAND => {
                if let Some(path) = self.path.clone()
                    && let Err(e) = olo_kit::tracker::open(&path)
                {
                    self.error(format!("“{}” couldn't be shown in Tracker. {e}", file_name(&path)), cx);
                }
            }
            "revert" => {
                let message = format!("Revert to the last saved version of “{}”?", self.title());
                let doc = cx.view_ref();
                let _ = open_alert(cx, "StyledEdit", "warning", &message, &["Cancel", "Revert"], move |choice, cx| {
                    if *choice == Some(1) {
                        doc.update(cx, |d, cx| d.finish(After::Revert, cx));
                    }
                });
            }
            "find" => self.open_find(false, cx),
            "replace" => self.open_find(true, cx),
            "find-again" => {
                self.find_next(cx);
            }
            "replace-same" => self.replace(cx),
            "find-selection" => {
                self.shared.common().search.find = self.selected_text().to_owned();
                self.find_next(cx);
            }
            "spelling" => {
                {
                    let mut c = self.shared.common();
                    c.spelling = !c.spelling;
                }
                cx.invalidate();
                to_hub(ToHub::Again(false));
            }
            "spell:learn" | "spell:ignore" => {
                let word = self.spell.as_ref().map(|(r, _)| self.doc.text()[r.clone()].to_owned());
                if let (Some(c), Some(word)) = (olo_kit::spell::loaded(), word) {
                    if command == "spell:learn" { c.learn(&word) } else { c.ignore(&word) }
                }
                self.editor.respell();
                cx.invalidate();
                to_hub(ToHub::Again(true));
            }
            "ruler" => {
                self.edit_command(command, cx);
                self.shared.ruler.store(self.ruler, Ordering::Relaxed);
            }
            "tool-bars" => {
                self.edit_command(command, cx);
                self.shared.tool_bars.store(self.tool_bars, Ordering::Relaxed);
            }
            _ => self.edit_command(command, cx),
        }
    }

    /// Files dropped on the window: a picture goes in the document (at
    /// the caret); anything else opens.
    fn dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Self>) {
        let editable = !self.loading && !self.read_only;
        for p in paths {
            if editable && is_picture(&p) {
                self.put_picture(p, cx);
            } else {
                to_hub(ToHub::Open(p));
            }
        }
    }
}

/// The Find or Replace window.
struct FindView {
    shared: Arc<Shared>,
    doc: ViewRef<DocView>,
    replace: bool,
    find: Child<TextField>,
    with: Child<TextField>,
    s: Search,
}

impl FindView {
    fn new(shared: Arc<Shared>, doc: ViewRef<DocView>, replace: bool, cx: &mut Cx<Self>) -> FindView {
        let search = shared.common().search.clone();
        let find = text_field(cx, |f| f.name("Find"));
        let with = text_field(cx, |f| f.name("Replace with"));
        find.update(cx, |f, cx| {
            f.set_text(search.find.clone(), cx);
            f.select_all_text(cx);
        });
        find.focus(cx);
        with.update(cx, |f, cx| f.set_text(search.replace.clone(), cx));
        cx.subscribe(&find, |_: &mut FindView, _: &TextFieldEvent, cx| cx.invalidate());
        FindView { shared, doc, replace, find, with, s: search }
    }

    fn done(&mut self, go: bool, cx: &mut Cx<Self>) {
        if go && !self.find.read(cx).text().is_empty() {
            self.s.find = self.find.read(cx).text().to_owned();
            self.s.replace = self.with.read(cx).text().to_owned();
            self.shared.common().search = self.s.clone();
            let replace = self.replace;
            self.doc.update(cx, move |d, cx| {
                if replace {
                    d.replace(cx);
                } else {
                    d.find_next(cx);
                }
                cx.activate_window();
            });
        }
        cx.close_window();
    }
}

impl View for FindView {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let divider = if self.replace { 86.0 } else { 40.0 };
        let check = |label: &'static str, on: bool, set: fn(&mut Search, bool)| {
            CheckBox::new(label, on).on_toggle(cx.listener(move |v: &mut FindView, on: &bool, cx| {
                set(&mut v.s, *on);
                cx.invalidate();
            }))
        };
        let label = match (self.replace, self.s.all) {
            (false, _) => "Find",
            (true, false) => "Replace",
            (true, true) => "Replace All",
        };
        let ready = !self.find.read(cx).text().is_empty();
        let mut fields = div().flex().flex_col().gap(px(m::TIGHT)).child(TextControl::new(cx, &self.find).label("Find:").divider(divider));
        if self.replace {
            fields = fields.child(TextControl::new(cx, &self.with).label("Replace with:").divider(divider));
        }
        let mut checks = div().flex().flex_col().child(check("Case-sensitive", self.s.case, |s, on| s.case = on)).child(check("Wrap-around search", self.s.wrap, |s, on| s.wrap = on)).child(check(
            "Search backwards",
            self.s.backwards,
            |s, on| s.backwards = on,
        ));
        if self.replace {
            checks = checks.child(check("Replace all occurrences", self.s.all, |s, on| s.all = on));
        }
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|v: &mut FindView, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(fields)
            .child(div().h(px(m::SPACING)))
            .child(checks)
            .child(div().flex_1())
            .child(
                button_row()
                    .child(Button::new("Cancel").on_click(cx.listener(|v: &mut FindView, _: &ClickEvent, cx| v.done(false, cx))))
                    .child(Button::new(label).default(true).enabled(ready).on_click(cx.listener(|v: &mut FindView, _: &ClickEvent, cx| v.done(true, cx)))),
            )
            .into_any_element()
    }
}

/// Document ▸ Header & Footer…: three parts of each, and where `{page}`
/// and `{pages}` go.
struct MarginsView {
    doc: ViewRef<DocView>,
    fields: [Child<TextField>; 6],
}

impl MarginsView {
    fn new(doc: ViewRef<DocView>, page: &doc::Page, cx: &mut Cx<Self>) -> MarginsView {
        let names = ["Header Left", "Header Center", "Header Right", "Footer Left", "Footer Center", "Footer Right"];
        let texts: Vec<String> = page.header.iter().chain(page.footer.iter()).cloned().collect();
        let fields: [Child<TextField>; 6] = std::array::from_fn(|k| {
            let f = text_field(cx, |f| f.name(names[k]));
            f.update(cx, |f, cx| f.set_text(texts[k].clone(), cx));
            f
        });
        fields[0].focus(cx);
        MarginsView { doc, fields }
    }

    fn done(&mut self, ok: bool, cx: &mut Cx<Self>) {
        let t: Vec<String> = self.fields.iter().map(|f| f.read(cx).text().to_owned()).collect();
        let page = doc::Page { header: [t[0].clone(), t[1].clone(), t[2].clone()], footer: [t[3].clone(), t[4].clone(), t[5].clone()] };
        self.doc.update(cx, move |d, cx| {
            if ok && d.doc.page.as_ref() != Some(&page) {
                d.doc.page = Some(page);
                d.doc.generation += 1;
            }
            cx.invalidate();
            cx.activate_window();
        });
        cx.close_window();
    }
}

impl View for MarginsView {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let group = |title: &'static str, fields: &[Child<TextField>]| {
            let mut g = div().flex().flex_col().gap(px(m::TIGHT)).child(heading(title));
            for (f, label) in fields.iter().zip(["Left:", "Center:", "Right:"]) {
                g = g.child(TextControl::new(cx, f).label(label).divider(MARGINS_DIVIDER));
            }
            g
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .on_key_down(cx.listener(|v: &mut MarginsView, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(group("Header", &self.fields[..3]))
            .child(group("Footer", &self.fields[3..]))
            .child(div().text_size(px(style::SMALL_SIZE)).text_color(style::disabled_text(&p)).child("{page} is each page’s number, {pages} how many there are."))
            .child(div().flex_1())
            .child(
                button_row()
                    .child(Button::new("Cancel").on_click(cx.listener(|v: &mut MarginsView, _: &ClickEvent, cx| v.done(false, cx))))
                    .child(Button::new("OK").default(true).on_click(cx.listener(|v: &mut MarginsView, _: &ClickEvent, cx| v.done(true, cx)))),
            )
            .into_any_element()
    }
}

/// The picture that is the selection (its character alone), if it is one.
fn picture_selected(doc: &Doc, sel: Range<usize>) -> Option<doc::Picture> {
    (doc.text().get(sel.clone())? == editor::PICTURE.to_string()).then(|| doc.style_at(sel.end).picture).flatten()
}

/// A picture file, by its name.
fn is_picture(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "avif" | "jxl")
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

const STATUS_H: f32 = SCROLL_BAR + 2.0;
const FIND_W: f32 = 340.0;
const FIND_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + 3.0 * m::MARK_ROW + m::SPACING + m::CONTROL_HEIGHT;
const REPLACE_H: f32 = FIND_H + m::CONTROL_HEIGHT + m::TIGHT + m::MARK_ROW;
/// The Header & Footer window: two headed groups of three fields, a
/// note, and its buttons.
const MARGINS_W: f32 = 420.0;
const MARGINS_H: f32 = 2.0 * m::WINDOW_INSET + 2.0 * (m::MARK_ROW + 3.0 * (m::TIGHT + m::CONTROL_HEIGHT)) + m::SPACING + m::SPACING + m::MARK_ROW + m::SPACING + m::CONTROL_HEIGHT;
const MARGINS_DIVIDER: f32 = 56.0;
/// Between sheets in Page View (points).
const PAGE_GAP: f32 = 16.0;
/// The most room a picture put in takes at first (points): a US Letter
/// page's text across, and most of its height.
const PICTURE_ROOM: (f32, f32) = (468.0, 600.0);

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let fonts = fonts::Fonts::start(olo_kit::text_system(), |e| to_hub(ToHub::Fonts(e)));
        let shared = Arc::new(Shared {
            common: Mutex::new(Common { search: Search { wrap: true, ..Search::default() }, spelling: true, fonts }),
            ruler: AtomicBool::new(true),
            tool_bars: AtomicBool::new(true),
            saving: Mutex::default(),
        });
        let mut hub = Hub { docs: Vec::new(), shared: shared.clone(), untitled: 0, watch: None, next: 0 };
        // The documents open (unsaved work too), kept for an upgrade:
        // what each window keeps, as one list.
        olo_kit::on_upgrade(|windows| {
            let docs: Vec<serde_json::Value> = windows.iter().filter_map(|s| serde_json::from_str(s).ok()).collect();
            serde_json::to_string(&docs).ok()
        });
        // A save under way: the new version waits.
        let busy = shared.clone();
        olo_kit::upgrade_waits_while(move || busy.saving.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|b| b.upgrade().is_some_and(|b| b.load(Ordering::Relaxed))));
        let restored = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Vec<SavedDoc>>(&s).ok());
        match restored {
            Some(docs) if !docs.is_empty() => {
                hub.restore(docs);
                if hub.docs.is_empty() {
                    hub.new_doc();
                }
            }
            _ => hub.handle_args(&args, None),
        }
        olo_kit::message_handler(app, move |message, _| hub.message(message));
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn menus_say_the_commands_they_always_did() {
        let kinds: Vec<String> = super::doc::Kind::ALL.iter().map(|(_, _, id)| format!("kind:{id}")).collect();
        assert_eq!(kinds, super::KIND_COMMANDS);
        let sizes: Vec<(String, String)> = super::doc::SIZES.iter().map(|s| (format!("{s}"), format!("size:{s}"))).collect();
        assert_eq!(sizes, super::SIZE_ITEMS.map(|(l, c)| (l.to_owned(), c.to_owned())));
        let colors: Vec<String> = (0..super::COLORS.len()).map(|k| format!("color:{k}")).collect();
        assert_eq!(colors, super::COLOR_COMMANDS);
        let headings: Vec<(String, String)> = (1..=6).map(|n| (format!("Heading {n}"), format!("md:heading-{n}"))).collect();
        assert_eq!(headings, super::MD_HEADINGS.map(|(l, c)| (l.to_owned(), c.to_owned())));
    }

    #[test]
    fn guide_words() {
        let names: Vec<String> = ["Adwaita Sans", "Cantarell", "Jomolhari", "Noto Sans Arabic", "Noto Sans Bengali", "Z003"].map(String::from).to_vec();
        let chunks: Vec<&[String]> = names.chunks(2).collect();
        assert_eq!(super::guide_words(&chunks), ["Adwaita – C", "J – Noto Sans A", "Noto Sans B – Z003"]);
    }
}
