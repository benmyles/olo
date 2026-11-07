//! StyledEdit: BeOS's text editor. Plain text files with fonts, sizes,
//! colours and styles kept beside the text in attributes (doc.rs), so
//! other programs still see plain text. One window per document; Find and
//! Replace windows as in Haiku; unsaved work survives a reload. On Olo's
//! GPUI: the document in the kit's editor (`olo_ui::editor`), its
//! paragraphs' indents and tabs on the kit's ruler.
//!
//! A file's kind sets how it's shown ([`Mode`]): source code (by its name
//! or `#!` line) in the kit's code view (`olo_ui::code`: coloured by its
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
    rc::Rc,
};

use doc::{Align, Change, Doc, Run, Style};
use olo_ui::{
    Frame, Menu, MenuBar, MenuItem, WindowOptions,
    code::{self, Code},
    editor::{self, EditKey, Editor, EditorHost},
    file_panel::{FilePanel, Filter, PanelResult},
    gpui::{
        self, App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, EntityId, ExternalPaths, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
        Pixels, Point, Render, Styled as _, Subscription, WeakEntity, Window, WindowHandle, canvas, div, px,
    },
    instance::{self, Launch},
    metrics as m, style, theme,
    widgets::{
        Button, CheckBox, SCROLL_BAR, TextControl, TextField, TextFieldEvent, ToolBar, ToolBarView, Unsaved, ask_to_save, button_row, heading, open_alert,
        ruler::{self, Ruler, RulerEvent},
        text_field,
    },
};
use serde::{Deserialize, Serialize};

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

/// An imported document: its text, runs, paragraphs, header and footer.
type Imported = (String, Vec<Run>, Vec<doc::Para>, Option<doc::Page>);

enum Msg {
    Fonts(fonts::Event),
    Loaded(EntityId, PathBuf, Result<doc::Loaded, String>),
    /// Saved the document as it was at generation `.1` to `.2`.
    Saved(EntityId, u64, PathBuf, Result<(), String>),
    /// Open documents' files changed on disk (another program wrote one).
    OnDisk(Vec<PathBuf>),
    /// A formatted document read in: its text, runs and paragraphs.
    Imported(PathBuf, Result<Imported, String>),
    /// A picture to put in document `.0` at its selection: its file and
    /// its size in pixels.
    Picture(EntityId, PathBuf, Option<(u32, u32)>),
    /// A copy written in another format.
    Exported(PathBuf, Result<(), String>),
    /// A document sent to a printer (its title).
    Printed(String, Result<(), String>),
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
    app: WeakEntity<StyledEdit>,
    win: Option<WindowHandle<Frame>>,
    /// None until first saved.
    path: Option<PathBuf>,
    /// "Untitled N" for a new document.
    untitled: usize,
    doc: Doc,
    /// Shows and edits `doc` (a paragraph at a time: documents of any
    /// length stay quick).
    editor: Editor,
    ruler_view: Entity<Ruler>,
    /// The tool bars, along the top and down the side, each drawn only as
    /// what it shows changes.
    bar_views: (Entity<ToolBarView>, Entity<ToolBarView>),
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
    pictures: Rc<RefCell<pictures::Pictures>>,
    common: Rc<RefCell<Common>>,
    /// A picture pasted (its bytes), to put in once kept.
    pasted: Option<Vec<u8>>,
    /// Styled, source code or Markdown.
    mode: Mode,
    /// The link right-clicked in a Markdown document (Open Link).
    link: Option<String>,
    /// Source shows line numbers (Document ▸ Line Numbers).
    line_numbers: bool,
    _subs: Vec<Subscription>,
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
    fn editor(&mut self, _: usize, cx: &mut Context<Self>, f: &mut dyn FnMut(&mut Editor, &mut dyn editor::Document, &mut Context<Self>)) {
        let editable = !self.read_only && !self.loading;
        let folder = self.path.as_deref().and_then(Path::parent);
        let palette = theme::palette(cx);
        let width = self.editor.text_span().map_or(400.0, |s| s.1);
        let mut styled = Styled { doc: &mut self.doc, pictures: &self.pictures, folder, pasted: &mut self.pasted, editable, mode: &mut self.mode, palette: &palette, width };
        f(&mut self.editor, &mut styled, cx);
        styled.sync();
    }

    fn text_moved(&mut self, _: usize, (origin, width): (f32, f32), cx: &mut Context<Self>) {
        self.ruler_view.update(cx, |r, _| r.follow(origin, width));
    }

    fn edited(&mut self, _: usize, cx: &mut Context<Self>) {
        // A picture pasted: kept as a file (on a worker), then put in.
        if let Some(data) = self.pasted.take() {
            let (proxy, id) = (olo_ui::proxy(cx), cx.entity_id());
            std::thread::spawn(move || {
                if let Some(path) = pictures::hold(&data) {
                    let px = pictures::pixels(&path);
                    proxy.send(Msg::Picture(id, path, px));
                }
            });
        }
        cx.notify();
    }

    fn open_link(&mut self, _: usize, link: String, _: &mut Window, _: &mut Context<Self>) {
        self.open(link);
    }

    fn context_menu(&mut self, _: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let editable = !self.read_only && !self.loading;
        let spelling = self.common.borrow().spelling;
        // On a misspelled word (the editor has put the caret in it): what
        // it might be, first.
        self.spell = None;
        if spelling
            && editable
            && let Some(checker) = olo_ui::spell::checker(cx)
            && let Some(r) = olo_ui::spell::word_at(self.doc.text(), self.editor.caret())
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
                menu = menu.item(g.as_str(), format!("spell:{k}"));
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
        let (app, me) = (self.app.clone(), cx.entity());
        olo_ui::menu::context_menu(window, cx, menu, at, move |cmd, window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |a, cx| a.doc_command(&me, cmd, window, cx));
            }
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

    fn new(app: WeakEntity<StyledEdit>, path: Option<PathBuf>, untitled: usize, pictures: Rc<RefCell<pictures::Pictures>>, common: Rc<RefCell<Common>>, cx: &mut Context<Self>) -> DocView {
        let ruler_view = cx.new(|_| Ruler::new());
        let sub = cx.subscribe(&ruler_view, |d: &mut DocView, _, e: &RulerEvent, cx| d.ruled(e, cx));
        DocView {
            app,
            win: None,
            path,
            untitled,
            doc: Doc::new(String::new(), None),
            editor: Editor::new(cx),
            ruler_view,
            bar_views: (cx.new(|_| ToolBarView::default()), cx.new(|_| ToolBarView::default())),
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
            ruler: true,
            page_view: false,
            tool_bars: true,
            pictures,
            common,
            pasted: None,
            mode: Mode::Styled,
            link: None,
            line_numbers: true,
            _subs: vec![sub],
        }
    }

    /// The ruler changed the caret's paragraph's indents and tabs: the
    /// selected paragraphs take them (one step to undo).
    fn ruled(&mut self, e: &RulerEvent, cx: &mut Context<Self>) {
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
        cx.notify();
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
    fn set_title(&self, cx: &mut Context<Self>) {
        if let Some(win) = self.win {
            let title = self.title();
            cx.defer(move |cx| {
                let _ = win.update(cx, |f, window, _| f.set_title(title, window));
            });
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
    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            "source" => {
                if let Mode::Markdown(look) = &mut self.mode {
                    look.source = !look.source;
                    self.editor.relayout();
                }
                return cx.notify();
            }
            "line-numbers" => {
                self.line_numbers = !self.line_numbers;
                return cx.notify();
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
                return cx.notify();
            }
            _ => {}
        }
        if matches!(self.mode, Mode::Markdown(_)) && self.markdown_command(command) {
            return cx.notify();
        }
        // (Styles are a styled document's alone.)
        let style_command = ["bold", "italic", "underline", "strikeout", "indent-more", "indent-less", "list-bullet", "list-number"].contains(&command)
            || ["align-", "kind:", "size:", "color:", "family:", "table:", "picture:"].iter().any(|p| command.starts_with(p));
        if !self.mode.is_styled() && style_command {
            return;
        }
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
                cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text().to_owned()));
                if command == "cut" && !self.read_only {
                    let change = self.doc.replace(bytes, "", selection.clone());
                    self.changed(change, selection.start..selection.start);
                }
            }
            "paste" => {
                let me = cx.entity();
                window.defer(cx, move |window, cx| editor::edit(&me, 0, editor::Edit::Paste, window, cx));
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
        cx.notify();
    }
}

/// Font ▸ Size, checking the size where the selection starts.
fn size_menu(cur: &Style) -> Menu {
    let mut size = Menu::new("Size");
    for s in doc::SIZES {
        size = size.item(format!("{s}"), format!("size:{s}")).checked(cur.size == s);
    }
    size
}

/// Font ▸ Color, each with its swatch (Black the document's text colour,
/// `ink`, as it shows).
fn color_menu(cur: &Style, ink: gpui::Hsla) -> Menu {
    let mut color = Menu::new("Color");
    for (k, (name, rgb)) in COLORS.iter().enumerate() {
        color = color.item(*name, format!("color:{k}")).checked(cur.color == *rgb).swatch(rgb.map_or(ink, rgb_color));
    }
    color
}

fn rgb_color([r, g, b]: [u8; 3]) -> gpui::Hsla {
    gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into()
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
        font = font.item(*f, format!("family:{f}")).checked(cur.family == *f);
    }
    let families = &app.fonts.families;
    if !families.is_empty() {
        font = font.separator();
        let chunks: Vec<&[String]> = families.chunks(FAMILY_GROUP).collect();
        for (chunk, title) in chunks.iter().zip(guide_words(&chunks)) {
            let mut group = Menu::new(title);
            for f in chunk.iter() {
                group = group.item(f.as_str(), format!("family:{f}")).checked(cur.family == *f);
            }
            font = font.submenu(group);
        }
    }
    font
}

/// The tool bars: along the top, the file's and editing's commands and
/// the text's style (wrapping to a second row in a narrow window); down
/// the side, what goes in and how the document shows. Every tool is one
/// of `menus`' commands.
fn tool_bars(me: &Entity<DocView>, d: &DocView, menus: &[Menu], ink: gpui::Hsla, on_command: impl Fn(&str, &mut Window, &mut App) + Clone + 'static) -> (ToolBar, ToolBar) {
    let editable = !d.read_only && !d.loading;
    let cur = d.doc.style_at(d.bytes().start);
    // (The pickers' menus are made as they open, from the document then.)
    let style_now = |me: &Entity<DocView>, cx: &App| {
        let d = me.read(cx);
        d.doc.style_at(d.bytes().start)
    };
    let (fonts, common, sizes, colors) = (me.clone(), d.common.clone(), me.clone(), me.clone());
    let family = if cur.family.is_empty() { "System Font".to_owned() } else { cur.family.clone() };
    let color = COLORS.iter().find(|(_, rgb)| *rgb == cur.color).map_or("Other", |(name, _)| name);
    let top = ToolBar::new("tools", "Tools", on_command.clone())
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
        .picker("Font", family, 132.0, move |cx| families(Menu::new("Font"), &common.borrow(), fonts.read(cx), &style_now(&fonts, cx)))
        .enabled(editable)
        .picker("Size", format!("{}", cur.size), 52.0, move |cx| size_menu(&style_now(&sizes, cx)))
        .enabled(editable)
        .colors("Color", color, Some(cur.color.map_or(ink, rgb_color)), move |cx| color_menu(&style_now(&colors, cx), ink))
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
    let side = ToolBar::new("insert", "Insert", on_command)
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
fn doc_menus(app: &Common, d: &DocView, ink: gpui::Hsla) -> Vec<Menu> {
    let has_sel = !d.bytes().is_empty();
    let editable = !d.read_only && !d.loading;
    let file = Menu::new("File")
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
        .item(olo_ui::tracker::LABEL, olo_ui::tracker::COMMAND)
        .enabled(d.path.is_some())
        .separator()
        .item("Close", "close")
        .key("w")
        .item("Quit", "quit")
        .key("q");
    // The picture selected, alone.
    let picked = picture_selected(&d.doc, d.bytes());
    let edit = Menu::new("Edit")
        .item("Undo", "undo")
        .key("z")
        .enabled(editable && d.doc.can_undo())
        .item("Redo", "redo")
        .shift_key("z")
        .enabled(editable && d.doc.can_redo())
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
    for (k, label, id) in doc::Kind::ALL {
        kinds = kinds.item(label, format!("kind:{id}")).checked(para.kind == k);
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
        for n in 1..=6u8 {
            kinds = kinds.item(format!("Heading {n}"), format!("md:heading-{n}"));
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

impl Render for DocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            self.editor.focus(window);
        }
        let p = theme::palette(cx);
        let me = cx.entity();
        let ink = style::c(p.document_text);
        let menus = doc_menus(&self.common.borrow(), self, ink);
        let spelling = self.common.borrow().spelling;
        let (weak, doc) = (self.app.clone(), me.clone());
        let on_command = move |cmd: &str, window: &mut Window, cx: &mut App| {
            if let Some(app) = weak.upgrade() {
                app.update(cx, |a, cx| a.doc_command(&doc, cmd, window, cx));
            }
        };
        let styled = self.mode.is_styled();
        let source = self.mode.is_source();
        // (Styles are a styled document's: its tool bars and ruler too.)
        let bars = (self.tool_bars && styled).then(|| tool_bars(&me, self, &menus, ink, on_command.clone()));
        let bar = MenuBar::new(menus, on_command);
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
        let status = self.status();
        let body = editor::view(&me, 0, &self.editor, options, window);
        // Over the text: the header and footer on each sheet, as printed,
        // and where a marker held on the ruler goes.
        let (over, ruler_view) = (me.clone(), self.ruler_view.clone());
        let overlay = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let d = over.read(cx);
                let (sheets, n) = d.editor.sheets();
                let page = d.doc.page.clone();
                let sheets: Vec<_> = sheets.to_vec();
                let ink = style::c(p.document_text).opacity(0.6);
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for (k, sheet) in &sheets {
                        for (i, (text, y)) in pages::margin_lines(page.as_ref(), *k, n).into_iter().enumerate() {
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
                            style::label(window, cx, x, center, align, text, pages::MARGIN_TEXT, style::ui_font(FontWeight::NORMAL), ink);
                        }
                    }
                    if let Some(x) = ruler_view.read(cx).guide() {
                        style::vline(window, px(x), b.top(), b.bottom(), style::c(p.keyboard_navigation));
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        let status_line = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, style::c(p.panel_background));
                style::hline(window, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.3));
                // Centred under the line along the top.
                let y = b.center().y + style::hairline(window) / 2.0;
                let at = style::label(window, cx, b.left() + px(m::TIGHT + 2.0), y, style::Align::Left, status.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(status.clone(), at));
            },
        )
        .w_full()
        .h(px(STATUS_H));
        // The side bar as tall as what's between the top one and the
        // status line.
        let (top, side) = match bars {
            Some((top, side)) => {
                let view = window.viewport_size();
                let below = olo_ui::menu::BAR_HEIGHT + top.extent(window, f32::from(view.width)) + STATUS_H;
                let side = side.length(f32::from(view.height) - below);
                let (top_view, side_view) = self.bar_views.clone();
                (Some(top.cached(&top_view, window, cx)), Some(side.cached(&side_view, window, cx)))
            }
            None => (None, None),
        };
        let (dropper, weak) = (me.clone(), self.app.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let paths: Vec<PathBuf> = paths.paths().to_vec();
                if let Some(app) = weak.upgrade() {
                    app.update(cx, |a, cx| a.dropped(&dropper, paths, cx));
                }
            })
            .child(bar)
            .children(top)
            .child(
                div().flex_1().min_h_0().flex().children(side).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(show_ruler.then(|| div().w_full().h(style::snap_coord(window, px(ruler::HEIGHT))).child(self.ruler_view.clone())))
                        .child(div().flex_1().min_h_0().relative().child(body).child(overlay)),
                ),
            )
            .child(status_line)
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

/// What the app and its documents' windows both read (a window drawn
/// while the app is busy can't read the app itself).
struct Common {
    search: Search,
    /// Misspelled words are underlined (Edit ▸ Check Spelling While
    /// Typing).
    spelling: bool,
    fonts: fonts::Fonts,
}

struct StyledEdit {
    docs: Vec<Entity<DocView>>,
    common: Rc<RefCell<Common>>,
    untitled: usize,
    /// The open documents' files, followed as they change: one changed
    /// by another program is read again if it has no unsaved changes here.
    watch: Option<olo_ui::watch::Watch>,
    /// The paper and pages the next PDF is made with (the Print panel's,
    /// when it chose a PDF).
    pdf_setup: Option<(olo_print::Paper, Option<(u32, u32)>)>,
    /// New windows show the ruler (as the last one shown or hidden).
    ruler: bool,
    /// New windows show the tool bars (as the last one shown or hidden).
    tool_bars: bool,
    /// The documents' pictures.
    pictures: Rc<RefCell<pictures::Pictures>>,
    /// File panels and alerts open with no document (to quit when they go).
    panels: usize,
    /// Each document's Find and Replace windows, and its Header & Footer
    /// one (`true` for Replace; None for Header & Footer).
    tools: Vec<(EntityId, Option<bool>, WindowHandle<Frame>)>,
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

impl StyledEdit {
    fn doc(&self, id: EntityId) -> Option<Entity<DocView>> {
        self.docs.iter().find(|d| d.entity_id() == id).cloned()
    }

    fn new_doc(&mut self, cx: &mut Context<Self>) -> Entity<DocView> {
        self.untitled += 1;
        let n = self.untitled;
        self.add_window(None, n, |_| {}, cx)
    }

    /// A window for a document (`setup` gives it its text).
    fn add_window(&mut self, path: Option<PathBuf>, untitled: usize, setup: impl FnOnce(&mut DocView), cx: &mut Context<Self>) -> Entity<DocView> {
        let (app, pictures, common, ruler, tool_bars) = (cx.weak_entity(), self.pictures.clone(), self.common.clone(), self.ruler, self.tool_bars);
        let doc = cx.new(|cx| {
            let mut d = DocView::new(app, path, untitled, pictures, common, cx);
            d.ruler = ruler;
            d.tool_bars = tool_bars;
            setup(&mut d);
            d
        });
        let title = doc.read(cx).title();
        let view = doc.clone();
        let win = olo_ui::open_window(cx, WindowOptions::new(title, 560.0, 440.0).min_size(280.0, 180.0).keep_size("document"), move |window, cx| {
            // The editor has the keyboard (else the window's frame does).
            view.read(cx).editor.focus(window);
            view
        });
        doc.update(cx, |d, _| d.win = win);
        if let Some(win) = win {
            // The window's close: asks about unsaved changes first.
            let (closing, me) = (doc.clone(), cx.entity());
            let _ = win.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    me.update(cx, |a, cx| a.leave(&closing, After::Close, cx));
                    false
                });
            });
        }
        self.docs.push(doc.clone());
        doc
    }

    /// Follow the open documents' files (as they are now).
    fn follow_files(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.docs.iter().filter_map(|d| d.read(cx).path.clone()).collect();
        match &self.watch {
            Some(w) => w.set(paths),
            None => {
                let proxy = olo_ui::proxy(cx);
                self.watch = olo_ui::watch::watch(paths, move |changed| proxy.send(Msg::OnDisk(changed.to_vec())));
            }
        }
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Word, OpenDocument and RTF aren't plain text: in as a new
        // document.
        if olo_richtext::format_of(&path).is_some() {
            return self.import(path, cx);
        }
        if let Some(d) = self.docs.iter().find(|d| d.read(cx).path.as_ref() == Some(&path)) {
            activate(d, cx);
            return;
        }
        // A folder (dropped, named): said so, no window opened for it
        // first (one flashed open and shut, its alert after it).
        if path.is_dir() {
            return self.show_error(format!("“{}” is a folder: StyledEdit opens files.", file_name(&path)), cx);
        }
        let doc = self.add_window(Some(path.clone()), 0, |d| d.loading = true, cx);
        self.load(&doc, path, cx);
    }

    fn load(&mut self, doc: &Entity<DocView>, path: PathBuf, cx: &mut Context<Self>) {
        doc.update(cx, |d, cx| {
            d.loading = true;
            cx.notify();
        });
        let (proxy, id) = (olo_ui::proxy(cx), doc.entity_id());
        std::thread::spawn(move || {
            let r = doc::load(&path);
            proxy.send(Msg::Loaded(id, path, r));
        });
    }

    /// A document's window goes (and its Find, Replace and Header & Footer
    /// windows with it).
    fn close(&mut self, doc: &Entity<DocView>, cx: &mut Context<Self>) {
        let id = doc.entity_id();
        if let Some(win) = doc.read(cx).win {
            cx.defer(move |cx| {
                let _ = win.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.docs.retain(|d| d.entity_id() != id);
        for (_, _, w) in self.tools.iter().filter(|(d, ..)| *d == id) {
            let w = *w;
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.tools.retain(|(d, ..)| *d != id);
        self.quit_if_done(cx);
        self.follow_files(cx);
    }

    /// Nothing open any more: StyledEdit quits.
    fn quit_if_done(&self, cx: &mut Context<Self>) {
        if self.docs.is_empty() && self.panels == 0 {
            cx.quit();
        }
    }

    fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.panels += 1;
        let me = cx.entity();
        let _ = open_alert(cx, "StyledEdit", "warning", &message, &["OK"], move |_, cx| {
            me.update(cx, |a, cx| {
                a.panels = a.panels.saturating_sub(1);
                a.quit_if_done(cx);
            })
        });
    }

    fn open_panel(&mut self, doc: Option<Entity<DocView>>, purpose: Purpose, cx: &mut Context<Self>) {
        let d = doc.as_ref().map(|d| d.read(cx));
        let path = d.and_then(|d| d.path.clone());
        let title = d.map(|d| d.title()).unwrap_or_default();
        let dir = path.as_deref().and_then(Path::parent).map_or_else(home, Path::to_path_buf);
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
        self.panels += 1;
        let me = cx.entity();
        olo_ui::file_panel::show(cx, name, panel, move |answer, cx| {
            me.update(cx, |a, cx| {
                a.panels = a.panels.saturating_sub(1);
                let PanelResult::Chosen(path) = answer.result else {
                    // Cancelling a Save As drops what was to follow it.
                    if let Some(d) = &doc {
                        d.update(cx, |d, _| d.after_save = After::Nothing);
                    }
                    a.quit_if_done(cx);
                    return;
                };
                match (purpose, doc) {
                    (Purpose::Open, _) => a.open_file(path, cx),
                    (Purpose::SaveAs, Some(doc)) => {
                        let after = doc.read(cx).after_save;
                        a.save(&doc, Some(path), after, cx);
                    }
                    (Purpose::SaveAs, None) | (Purpose::Export(_), None) | (Purpose::Picture, None) => {}
                    (Purpose::Picture, Some(doc)) => a.put_picture(&doc, path, None, cx),
                    (Purpose::Import, _) => a.import(path, cx),
                    (Purpose::Export(f), Some(doc)) => a.export(&doc, path, f, cx),
                }
            })
        });
    }

    /// A formatted document read in (on a worker) as a new, untitled one:
    /// saving it writes a StyledEdit file, never over what it came from.
    fn import(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let proxy = olo_ui::proxy(cx);
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
            proxy.send(Msg::Imported(path, r));
        });
    }

    /// What a worker needs to set document `doc` on pages (as its window
    /// draws it).
    fn page_input(&self, doc: &Entity<DocView>, cx: &App) -> (pages::Input, String) {
        let d = doc.read(cx);
        let input = pages::Input {
            text: d.doc.text().to_owned(),
            runs: d.doc.runs().to_vec(),
            paras: d.doc.paras().to_vec(),
            align: d.doc.align,
            page: d.doc.page.clone(),
            text_system: cx.text_system().clone(),
            source: d.source(),
        };
        (input, d.title())
    }

    /// Print document `doc` on `printer` (set, made a PDF and sent on a
    /// worker).
    fn print(&mut self, doc: &Entity<DocView>, printer: String, choice: olo_print::panel::Choice, cx: &mut Context<Self>) {
        let (input, title) = self.page_input(doc, cx);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let pages = chosen(pages::pages(input, choice.paper), choice.pages);
            let r = olo_print::pdf(&pages, &title).and_then(|pdf| {
                let job = olo_print::ipp::Job { name: title.clone(), copies: choice.copies, media: Some(choice.paper.pwg.into()), pages: None, two_sided: false };
                olo_print::ipp::print(&printer, &pdf, &job)
            });
            proxy.send(Msg::Printed(title, r.map(|_| ())));
        });
    }

    /// A copy of the document in `format` at `to` (on a worker); the
    /// document stays as it is, and where.
    fn export(&mut self, doc: &Entity<DocView>, to: PathBuf, format: Format, cx: &mut Context<Self>) {
        let proxy = olo_ui::proxy(cx);
        if format == Format::Pdf {
            let (paper, range) = self.pdf_setup.take().unwrap_or((olo_print::local_paper(), None));
            let (input, title) = self.page_input(doc, cx);
            std::thread::spawn(move || {
                let pages = chosen(pages::pages(input, paper), range);
                let r = olo_print::pdf(&pages, &title).and_then(|pdf| std::fs::write(&to, pdf).map_err(|e| e.to_string()));
                proxy.send(Msg::Exported(to, r));
            });
            return;
        }
        let d = doc.read(cx);
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
            proxy.send(Msg::Exported(to, r));
        });
    }

    /// Write the document to `to` (its own file when None, or ask where).
    fn save(&mut self, doc: &Entity<DocView>, to: Option<PathBuf>, after: After, cx: &mut Context<Self>) {
        let d = doc.read(cx);
        let Some(path) = to.or_else(|| d.path.clone().filter(|_| !d.read_only)) else {
            doc.update(cx, |d, _| d.after_save = after);
            return self.open_panel(Some(doc.clone()), Purpose::SaveAs, cx);
        };
        // (Source code and Markdown are their text alone.)
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
        doc.update(cx, |d, cx| {
            d.saving = true;
            d.after_save = after;
            cx.notify();
        });
        let (proxy, id) = (olo_ui::proxy(cx), doc.entity_id());
        std::thread::spawn(move || {
            let r = doc::save(&path, &text, &runs, &paras, align, wrap, page.as_ref());
            proxy.send(Msg::Saved(id, generation, path, r));
        });
    }

    /// Close or revert a document, asking about unsaved changes.
    fn leave(&mut self, doc: &Entity<DocView>, after: After, cx: &mut Context<Self>) {
        let d = doc.read(cx);
        if !d.doc.edited() {
            return self.finish(doc, after, cx);
        }
        if d.asking {
            return activate(doc, cx);
        }
        let title = d.title();
        let doing = match after {
            After::Revert => "reverting",
            _ => "closing",
        };
        doc.update(cx, |d, _| d.asking = true);
        let (me, doc) = (cx.entity(), doc.clone());
        let _ = ask_to_save(cx, "StyledEdit", &title, doing, move |choice, cx| {
            doc.update(cx, |d, _| d.asking = false);
            me.update(cx, |a, cx| match choice {
                Some(Unsaved::DontSave) => a.finish(&doc, after, cx),
                Some(Unsaved::Save) => a.save(&doc, None, after, cx),
                None => {}
            })
        });
    }

    fn finish(&mut self, doc: &Entity<DocView>, after: After, cx: &mut Context<Self>) {
        match after {
            After::Nothing => {}
            After::Close => self.close(doc, cx),
            After::Revert => {
                if let Some(p) = doc.read(cx).path.clone() {
                    self.load(doc, p, cx);
                }
            }
        }
    }

    /// The Find or Replace window for document `doc` (one each).
    fn open_find(&mut self, doc: &Entity<DocView>, replace: bool, cx: &mut Context<Self>) {
        if self.bring_tool(doc.entity_id(), Some(replace), cx) {
            return;
        }
        let (title, h) = if replace { ("Replace", REPLACE_H) } else { ("Find", FIND_H) };
        let (app, d, search) = (cx.weak_entity(), doc.downgrade(), self.common.borrow().search.clone());
        let win = olo_ui::open_window(cx, WindowOptions::new(title, FIND_W, h).min_size(FIND_W, h).child(), move |window, cx| cx.new(|cx| FindView::new(app, d, replace, &search, window, cx)));
        if let Some(win) = win {
            self.tools.push((doc.entity_id(), Some(replace), win));
        }
    }

    /// Brings document `doc`'s tool window forward, if it's still open.
    fn bring_tool(&mut self, doc: EntityId, kind: Option<bool>, cx: &mut Context<Self>) -> bool {
        let Some(k) = self.tools.iter().position(|(d, t, _)| *d == doc && *t == kind) else { return false };
        let win = self.tools[k].2;
        if olo_ui::raise(Some(win), cx) {
            return true;
        }
        self.tools.remove(k);
        false
    }

    /// Document ▸ Header & Footer… for document `doc` (one window each).
    fn open_margins(&mut self, doc: &Entity<DocView>, cx: &mut Context<Self>) {
        if self.bring_tool(doc.entity_id(), None, cx) {
            return;
        }
        // Pages numbered at the foot until it's set otherwise.
        let page = doc.read(cx).doc.page.clone().unwrap_or_else(|| doc::Page { footer: [String::new(), "{page}".into(), String::new()], ..doc::Page::default() });
        let d = doc.downgrade();
        let win = olo_ui::open_window(cx, WindowOptions::new("Header & Footer", MARGINS_W, MARGINS_H).dialog(), move |window, cx| cx.new(|cx| MarginsView::new(d, &page, window, cx)));
        if let Some(win) = win {
            self.tools.push((doc.entity_id(), None, win));
        }
    }

    /// Put the picture at `path` in document `doc` (at its selection), its
    /// size read on a worker; `px` if known already.
    fn put_picture(&mut self, doc: &Entity<DocView>, path: PathBuf, px: Option<(u32, u32)>, cx: &mut Context<Self>) {
        let (proxy, id) = (olo_ui::proxy(cx), doc.entity_id());
        std::thread::spawn(move || {
            let px = px.or_else(|| pictures::pixels(&path));
            proxy.send(Msg::Picture(id, path, px));
        });
        activate(doc, cx);
    }

    /// Find the search text from the selection; false if it isn't there.
    fn find_next(&mut self, doc: &Entity<DocView>, cx: &mut Context<Self>) -> bool {
        let s = self.common.borrow().search.clone();
        doc.update(cx, |d, cx| {
            let sel = d.bytes();
            let from = if s.backwards { sel.start } else { sel.end };
            let found = doc::find(d.doc.text(), &s.find, from, s.backwards, s.case, s.wrap);
            if let Some(r) = &found {
                d.select_bytes(r.clone());
                cx.notify();
            }
            found.is_some()
        })
    }

    /// Replace the next match (or all of them), selecting what went in.
    fn replace(&mut self, doc: &Entity<DocView>, cx: &mut Context<Self>) {
        let s = self.common.borrow().search.clone();
        doc.update(cx, |d, cx| {
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
                cx.notify();
                return;
            }
            // The selection itself if it's a match, else the next one.
            let from = if s.backwards { selection.end } else { selection.start };
            let Some(r) = doc::find(d.doc.text(), &s.find, from, s.backwards, s.case, s.wrap) else { return };
            let change = d.doc.replace(r.clone(), &s.replace, selection);
            d.changed(change, r.start..r.start + s.replace.len());
            cx.notify();
        });
    }

    /// A command from a document's menus (or its text's right-click menu).
    fn doc_command(&mut self, doc: &Entity<DocView>, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            "new" => {
                self.new_doc(cx);
            }
            "open" => self.open_panel(Some(doc.clone()), Purpose::Open, cx),
            "import" => self.open_panel(Some(doc.clone()), Purpose::Import, cx),
            "export:pdf" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::Pdf), cx),
            "export:md" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::Markdown), cx),
            "export:html" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::Html), cx),
            "export:docx" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::Word), cx),
            "export:odt" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::OpenDocument), cx),
            "export:rtf" => self.open_panel(Some(doc.clone()), Purpose::Export(Format::Rtf), cx),
            "insert-picture" => self.open_panel(Some(doc.clone()), Purpose::Picture, cx),
            "print" => {
                let (me, d) = (cx.entity(), doc.clone());
                olo_print::panel::view::show(cx, olo_print::panel::view::window_options("Print"), None, move |choice, cx| {
                    let Some(choice) = choice else { return };
                    me.update(cx, |a, cx| match choice.printer.clone() {
                        // A PDF: where to, first.
                        None => {
                            a.pdf_setup = Some((choice.paper, choice.pages));
                            a.open_panel(Some(d.clone()), Purpose::Export(Format::Pdf), cx);
                        }
                        Some(printer) => a.print(&d, printer, choice, cx),
                    });
                });
            }
            "margins" => self.open_margins(doc, cx),
            "save" => self.save(doc, None, After::Nothing, cx),
            "save-as" => self.open_panel(Some(doc.clone()), Purpose::SaveAs, cx),
            "close" => self.leave(doc, After::Close, cx),
            "quit" => {
                for d in self.docs.clone() {
                    self.leave(&d, After::Close, cx);
                }
            }
            olo_ui::tracker::COMMAND => {
                if let Some(path) = doc.read(cx).path.clone()
                    && let Err(e) = olo_ui::tracker::open(&path)
                {
                    self.show_error(format!("“{}” couldn't be shown in Tracker. {e}", file_name(&path)), cx);
                }
            }
            "revert" => {
                let message = format!("Revert to the last saved version of “{}”?", doc.read(cx).title());
                let (me, d) = (cx.entity(), doc.clone());
                let _ = open_alert(cx, "StyledEdit", "warning", &message, &["Cancel", "Revert"], move |choice, cx| {
                    if choice == Some(1) {
                        me.update(cx, |a, cx| a.finish(&d, After::Revert, cx));
                    }
                });
            }
            "find" => self.open_find(doc, false, cx),
            "replace" => self.open_find(doc, true, cx),
            "find-again" => {
                self.find_next(doc, cx);
            }
            "replace-same" => self.replace(doc, cx),
            "find-selection" => {
                self.common.borrow_mut().search.find = doc.read(cx).selected_text().to_owned();
                self.find_next(doc, cx);
            }
            "spelling" => {
                let mut c = self.common.borrow_mut();
                c.spelling = !c.spelling;
                drop(c);
                for d in &self.docs {
                    d.update(cx, |_, cx| cx.notify());
                }
            }
            "spell:learn" | "spell:ignore" => {
                let checker = olo_ui::spell::loaded(cx);
                let word = doc.read(cx).spell.as_ref().map(|(r, _)| doc.read(cx).doc.text()[r.clone()].to_owned());
                if let (Some(c), Some(word)) = (checker, word) {
                    if command == "spell:learn" { c.learn(&word) } else { c.ignore(&word) }
                }
                for d in &self.docs {
                    d.update(cx, |d, cx| {
                        d.editor.respell();
                        cx.notify();
                    });
                }
            }
            "ruler" => {
                doc.update(cx, |d, cx| d.command(command, window, cx));
                self.ruler = doc.read(cx).ruler;
            }
            "tool-bars" => {
                doc.update(cx, |d, cx| d.command(command, window, cx));
                self.tool_bars = doc.read(cx).tool_bars;
            }
            _ => doc.update(cx, |d, cx| d.command(command, window, cx)),
        }
    }

    /// Files dropped on document `doc`'s window: a picture goes in it (at
    /// the caret); anything else opens.
    fn dropped(&mut self, doc: &Entity<DocView>, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let editable = {
            let d = doc.read(cx);
            !d.loading && !d.read_only
        };
        for p in paths {
            if editable && is_picture(&p) {
                self.put_picture(doc, p, None, cx);
            } else {
                self.open_file(p, cx);
            }
        }
    }

    fn handle_args(&mut self, args: &[String], cwd: Option<&Path>, cx: &mut Context<Self>) {
        let mut opened = false;
        for a in args.iter().filter(|a| !a.starts_with("--")) {
            let p = PathBuf::from(a);
            let p = match (p.is_absolute(), cwd) {
                (false, Some(cwd)) => cwd.join(p),
                _ => p,
            };
            self.open_file(p, cx);
            opened = true;
        }
        // Tracker's New Window: always a new document.
        if !opened && args.iter().any(|a| a == "--new-window") {
            self.new_doc(cx);
        } else if !opened {
            // A document there is, else a new one.
            match self.docs.first().cloned() {
                Some(d) => activate(&d, cx),
                None => {
                    self.new_doc(cx);
                }
            }
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(&launch.args, launch.cwd.as_deref(), cx),
            Err(m) => m,
        };
        // A picture decoded: drawn from now on.
        let message = match message.downcast::<pictures::Loaded>() {
            Ok(l) => {
                self.pictures.borrow_mut().loaded(*l);
                for d in &self.docs {
                    d.update(cx, |_, cx| cx.notify());
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Fonts(fonts::Event::Families(f)) => {
                self.common.borrow_mut().fonts.families = f;
                for d in &self.docs {
                    d.update(cx, |_, cx| cx.notify());
                }
            }
            Msg::Loaded(id, path, result) => {
                let Some(doc) = self.doc(id) else { return };
                if doc.read(cx).path.as_ref() != Some(&path) {
                    return;
                }
                let failed = doc.update(cx, |d, cx| {
                    d.loading = false;
                    cx.notify();
                    match result {
                        Ok(l) => {
                            // How it shows: by its kind (Markdown and code
                            // as they were, when it's read again).
                            let styled = l.runs.is_some() || l.paras.is_some();
                            d.mode = match (std::mem::replace(&mut d.mode, Mode::Styled), Mode::for_file(&path, &l.text, styled)) {
                                (Mode::Markdown(old), Mode::Markdown(_)) => Mode::Markdown(markdown::Look::new(old.source)),
                                (Mode::Code { code, .. }, _) if !styled => Mode::code(code.language(), &l.text),
                                (_, m) => m,
                            };
                            let mut doc = Doc::with_paras(l.text, l.runs, l.paras);
                            doc.align = l.align;
                            doc.wrap = l.wrap;
                            doc.page = l.page;
                            let (select, view) = d.restore.take().unwrap_or_default();
                            d.reset(doc, select);
                            d.editor.set_view(d.doc.text(), view);
                            d.latin1 = l.latin1;
                            d.read_only = l.read_only;
                            d.on_disk = stamp(&path);
                            None
                        }
                        // A new file: it's made on the first save.
                        Err(_) if !path.exists() => None,
                        Err(e) => Some(format!("“{}” couldn't be opened. {e}", file_name(&path))),
                    }
                });
                if let Some(message) = failed {
                    self.close(&doc, cx);
                    self.show_error(message, cx);
                    return;
                }
                self.follow_files(cx);
            }
            Msg::Saved(id, generation, path, result) => {
                let Some(doc) = self.doc(id) else { return };
                let after = doc.update(cx, |d, cx| {
                    d.saving = false;
                    cx.notify();
                    std::mem::replace(&mut d.after_save, After::Nothing)
                });
                match result {
                    Ok(()) => {
                        doc.update(cx, |d, cx| {
                            d.on_disk = stamp(&path);
                            if d.path.as_ref() != Some(&path) {
                                d.path = Some(path.clone());
                                d.read_only = false;
                                d.set_title(cx);
                                // Plain text saved as Markdown or code shows as that now.
                                if d.mode.is_styled() && d.doc.is_plain() {
                                    let mode = Mode::for_file(&path, d.doc.text(), false);
                                    if !mode.is_styled() {
                                        d.set_mode(mode);
                                    }
                                }
                            }
                            d.latin1 = false;
                            d.doc.saved_generation = generation;
                        });
                        self.follow_files(cx);
                        self.finish(&doc, after, cx);
                    }
                    Err(e) => self.show_error(format!("“{}” couldn't be saved. {e}", file_name(&path)), cx),
                }
            }
            Msg::Imported(path, result) => match result {
                Ok((text, runs, paras, page)) => {
                    self.untitled += 1;
                    let n = self.untitled;
                    self.add_window(
                        None,
                        n,
                        move |d| {
                            let mut doc = Doc::with_paras(text, Some(runs), Some(paras));
                            doc.page = page;
                            // Not saved anywhere yet.
                            doc.saved_generation = 0;
                            d.reset(doc, 0..0);
                        },
                        cx,
                    );
                }
                Err(e) => self.show_error(format!("“{}” couldn't be imported. {e}", file_name(&path)), cx),
            },
            Msg::Picture(id, path, px) => {
                let Some(doc) = self.doc(id) else { return };
                let Some(px) = px else {
                    return self.show_error(format!("“{}” isn't a picture StyledEdit can read.", file_name(&path)), cx);
                };
                let (width, height) = pictures::fit(px, PICTURE_ROOM.0, PICTURE_ROOM.1);
                let alt = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                doc.update(cx, |d, cx| {
                    let sel = d.bytes();
                    let pic = doc::Picture { path: path.to_string_lossy().into_owned(), width, height, alt };
                    let change = d.doc.insert_picture(sel.clone(), pic, sel.clone());
                    let after = sel.start + change.inserted;
                    d.changed(change, after..after);
                    cx.notify();
                });
            }
            Msg::Printed(title, result) => {
                if let Err(e) = result {
                    self.show_error(format!("“{title}” couldn't be printed. {e}"), cx);
                }
            }
            Msg::Exported(path, result) => {
                if let Err(e) = result {
                    self.show_error(format!("“{}” couldn't be written. {e}", file_name(&path)), cx);
                }
            }
            Msg::OnDisk(paths) => {
                // Written by another program (not our own save): read again
                // where nothing here is unsaved, keeping the place in it.
                let mut reload = Vec::new();
                for doc in &self.docs {
                    let path = doc.update(cx, |d, _| {
                        let path = d.path.clone().filter(|p| paths.contains(p))?;
                        let now = stamp(&path);
                        if now.is_none() || now == d.on_disk || d.doc.edited() || d.saving || d.loading {
                            return None;
                        }
                        d.on_disk = now;
                        d.restore = Some((d.editor.selection(), d.editor.view()));
                        Some(path)
                    });
                    if let Some(p) = path {
                        reload.push((doc.clone(), p));
                    }
                }
                for (doc, path) in reload {
                    self.load(&doc, path, cx);
                }
            }
        }
    }

    /// What's open, kept for an upgrade (unsaved work too).
    fn save_state(&self, cx: &App) -> Option<String> {
        let docs: Vec<SavedDoc> = self
            .docs
            .iter()
            .map(|d| {
                let d = d.read(cx);
                let dirty = d.doc.edited() || d.path.is_none();
                SavedDoc {
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
                }
            })
            .collect();
        serde_json::to_string(&docs).ok()
    }

    fn restore(&mut self, docs: Vec<SavedDoc>, cx: &mut Context<Self>) {
        for s in docs {
            self.untitled = self.untitled.max(s.untitled);
            match s.edited {
                Some((text, runs)) => {
                    let path = s.path.clone();
                    self.add_window(
                        s.path,
                        s.untitled,
                        move |d| {
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
                        },
                        cx,
                    );
                }
                None => {
                    let Some(path) = s.path else { continue };
                    let doc = self.add_window(
                        Some(path.clone()),
                        s.untitled,
                        |d| {
                            d.ruler = s.ruler;
                            d.page_view = s.page_view;
                            d.tool_bars = s.tool_bars;
                            d.line_numbers = s.line_numbers;
                            // (As it showed; its text, read, decides the rest.)
                            if let Some(m) = s.mode.as_deref().and_then(|m| Mode::named(m, "")) {
                                d.mode = m;
                            }
                            d.loading = true;
                            d.restore = Some((s.selection.0..s.selection.1, s.view));
                        },
                        cx,
                    );
                    self.load(&doc, path, cx);
                }
            }
        }
    }
}

/// Brings document `doc`'s window forward.
fn activate(doc: &Entity<DocView>, cx: &mut App) {
    if let Some(win) = doc.read(cx).win {
        olo_ui::raise(Some(win), cx);
    }
}

/// The Find or Replace window.
struct FindView {
    app: WeakEntity<StyledEdit>,
    doc: WeakEntity<DocView>,
    replace: bool,
    find: Entity<TextField>,
    with: Entity<TextField>,
    s: Search,
    _subs: Vec<Subscription>,
}

impl FindView {
    fn new(app: WeakEntity<StyledEdit>, doc: WeakEntity<DocView>, replace: bool, search: &Search, window: &mut Window, cx: &mut Context<Self>) -> FindView {
        let find = text_field(cx, |f| f.name("Find"));
        let with = text_field(cx, |f| f.name("Replace with"));
        find.update(cx, |f, cx| {
            f.set_text(search.find.clone(), cx);
            f.select_all_text(cx);
            f.focus(window);
        });
        with.update(cx, |f, cx| f.set_text(search.replace.clone(), cx));
        let subs = vec![cx.subscribe(&find, |_, _, _: &TextFieldEvent, cx| cx.notify())];
        FindView { app, doc, replace, find, with, s: search.clone(), _subs: subs }
    }

    fn done(&mut self, go: bool, window: &mut Window, cx: &mut Context<Self>) {
        if go && !self.find.read(cx).text().is_empty() {
            self.s.find = self.find.read(cx).text().to_owned();
            self.s.replace = self.with.read(cx).text().to_owned();
            let (s, replace) = (self.s.clone(), self.replace);
            if let (Some(app), Some(doc)) = (self.app.upgrade(), self.doc.upgrade()) {
                cx.defer(move |cx| {
                    app.update(cx, |a, cx| {
                        a.common.borrow_mut().search = s;
                        if replace {
                            a.replace(&doc, cx);
                        } else {
                            a.find_next(&doc, cx);
                        }
                    });
                    activate(&doc, cx);
                });
            }
        }
        window.remove_window();
    }
}

impl Render for FindView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let divider = if self.replace { 86.0 } else { 40.0 };
        let check = |label: &'static str, on: bool, set: fn(&mut Search, bool)| {
            let me = me.clone();
            CheckBox::new(label, on).on_toggle(move |on, _, cx| {
                me.update(cx, |v, cx| {
                    set(&mut v.s, on);
                    cx.notify();
                })
            })
        };
        let label = match (self.replace, self.s.all) {
            (false, _) => "Find",
            (true, false) => "Replace",
            (true, true) => "Replace All",
        };
        let ready = !self.find.read(cx).text().is_empty();
        let (go, cancel) = (me.clone(), me.clone());
        let mut fields = div().flex().flex_col().gap(px(m::TIGHT)).child(TextControl::new(self.find.clone()).label("Find:").divider(divider));
        if self.replace {
            fields = fields.child(TextControl::new(self.with.clone()).label("Replace with:").divider(divider));
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
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(fields)
            .child(div().h(px(m::SPACING)))
            .child(checks)
            .child(div().flex_1())
            .child(
                button_row()
                    .child(Button::new("Cancel").on_click(move |window, cx| cancel.update(cx, |v, cx| v.done(false, window, cx))))
                    .child(Button::new(label).default(true).enabled(ready).on_click(move |window, cx| go.update(cx, |v, cx| v.done(true, window, cx)))),
            )
    }
}

/// Document ▸ Header & Footer…: three parts of each, and where `{page}`
/// and `{pages}` go.
struct MarginsView {
    doc: WeakEntity<DocView>,
    fields: [Entity<TextField>; 6],
}

impl MarginsView {
    fn new(doc: WeakEntity<DocView>, page: &doc::Page, window: &mut Window, cx: &mut Context<Self>) -> MarginsView {
        let names = ["Header Left", "Header Center", "Header Right", "Footer Left", "Footer Center", "Footer Right"];
        let texts: Vec<String> = page.header.iter().chain(page.footer.iter()).cloned().collect();
        let fields: [Entity<TextField>; 6] = std::array::from_fn(|k| {
            let f = text_field(cx, |f| f.name(names[k]));
            f.update(cx, |f, cx| f.set_text(texts[k].clone(), cx));
            f
        });
        fields[0].update(cx, |f, _| f.focus(window));
        MarginsView { doc, fields }
    }

    fn done(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.doc.upgrade() {
            let t: Vec<String> = self.fields.iter().map(|f| f.read(cx).text().to_owned()).collect();
            let page = doc::Page { header: [t[0].clone(), t[1].clone(), t[2].clone()], footer: [t[3].clone(), t[4].clone(), t[5].clone()] };
            doc.update(cx, |d, cx| {
                if ok && d.doc.page.as_ref() != Some(&page) {
                    d.doc.page = Some(page);
                    d.doc.generation += 1;
                }
                cx.notify();
            });
            cx.defer(move |cx| activate(&doc, cx));
        }
        window.remove_window();
    }
}

impl Render for MarginsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let p = theme::palette(cx);
        let group = |title: &'static str, fields: &[Entity<TextField>]| {
            let mut g = div().flex().flex_col().gap(px(m::TIGHT)).child(heading(title));
            for (f, label) in fields.iter().zip(["Left:", "Center:", "Right:"]) {
                g = g.child(TextControl::new(f.clone()).label(label).divider(MARGINS_DIVIDER));
            }
            g
        };
        let (ok, cancel) = (me.clone(), me.clone());
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(group("Header", &self.fields[..3]))
            .child(group("Footer", &self.fields[3..]))
            .child(div().text_size(px(style::SMALL_SIZE)).text_color(style::disabled_text(&p)).child("{page} is each page’s number, {pages} how many there are."))
            .child(div().flex_1())
            .child(
                button_row()
                    .child(Button::new("Cancel").on_click(move |window, cx| cancel.update(cx, |v, cx| v.done(false, window, cx))))
                    .child(Button::new("OK").default(true).on_click(move |window, cx| ok.update(cx, |v, cx| v.done(true, window, cx)))),
            )
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
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let proxy = olo_ui::proxy(cx);
        let fonts = fonts::Fonts::start(cx.text_system().clone(), move |e| proxy.send(Msg::Fonts(e)));
        let mut pictures = pictures::Pictures::default();
        pictures.set_proxy(olo_ui::proxy(cx));
        let app = cx.new(|_| StyledEdit {
            docs: Vec::new(),
            common: Rc::new(RefCell::new(Common { search: Search { wrap: true, ..Search::default() }, spelling: true, fonts })),
            untitled: 0,
            watch: None,
            pdf_setup: None,
            ruler: true,
            tool_bars: true,
            pictures: Rc::new(RefCell::new(pictures)),
            panels: 0,
            tools: Vec::new(),
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |a, cx| a.message(message, cx)));
        // The documents open (unsaved work too), kept for an upgrade.
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state(cx));
        // A save under way: the new version waits.
        let busy = app.clone();
        olo_ui::upgrade_waits_while(cx, move |cx| busy.read(cx).docs.iter().any(|d| d.read(cx).saving));
        let restored = olo_ui::restored_state().and_then(|s| serde_json::from_str::<Vec<SavedDoc>>(&s).ok());
        app.update(cx, |a, cx| match restored {
            Some(docs) if !docs.is_empty() => a.restore(docs, cx),
            _ => a.handle_args(&args, None, cx),
        });
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
