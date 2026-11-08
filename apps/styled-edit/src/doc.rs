//! A styled document: the text and runs of styles over it (byte lengths
//! that add up to the text's), edited through [`Doc::edit`] (the editor's
//! typing), plus undo. Positions and selections are bytes.
//!
//! On disk it is plain UTF-8 text; the styles, alignment and wrapping go in
//! extended attributes beside it, as BeOS kept them in BFS attributes, so
//! other programs see an ordinary text file.

use std::{
    ops::Range,
    path::Path,
    time::{Duration, Instant},
};

use olo_ui::{
    editor::{self, PICTURE, Span},
    gpui::{Font, Hsla, Rgba},
    widgets::ruler,
};
use serde::{Deserialize, Serialize};

/// The sizes the Font menu offers (any size can be stored).
pub const SIZES: [f32; 10] = [9.0, 10.0, 11.0, 12.0, 14.0, 18.0, 24.0, 36.0, 48.0, 72.0];
pub const DEFAULT_SIZE: f32 = 12.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// A font family; empty is the system font.
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// None is the document's text colour (black, or white in dark mode).
    pub color: Option<[u8; 3]>,
    /// Where the text links to (from an imported document; kept for
    /// exporting it again).
    pub link: Option<String>,
    /// A picture: the text is its character (`editor::PICTURE`).
    pub picture: Option<Picture>,
}

/// A picture in the text ([`Style::picture`]): its file and how big it
/// is shown.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Picture {
    /// Where its file is: from the document's folder when relative
    /// (`resolve`).
    pub path: String,
    /// Points across and down.
    pub width: f32,
    pub height: f32,
    /// What it shows, for whoever can't see it.
    pub alt: String,
}

impl Picture {
    /// Its file, a relative path taken from `folder` (the document's).
    pub fn resolve(&self, folder: Option<&Path>) -> std::path::PathBuf {
        let p = Path::new(&self.path);
        match folder {
            Some(f) if p.is_relative() => f.join(p),
            _ => p.to_path_buf(),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Style { family: String::new(), size: DEFAULT_SIZE, bold: false, italic: false, underline: false, strikeout: false, color: None, link: None, picture: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Bytes of text this style covers.
    pub len: usize,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    /// How the editor lines rows up (justified rows from the left).
    pub fn to_kit(self) -> editor::Align {
        match self {
            Align::Left | Align::Justify => editor::Align::Left,
            Align::Center => editor::Align::Center,
            Align::Right => editor::Align::Right,
        }
    }
}

/// What a paragraph is: body text, a title or heading, a quotation or
/// code. Each has its size and spacing (`Kind::style`), and is what it
/// says in Markdown or HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Kind {
    #[default]
    Body,
    Title,
    Heading1,
    Heading2,
    Heading3,
    Quote,
    Code,
}

impl Kind {
    pub const ALL: [(Kind, &'static str, &'static str); 7] = [
        (Kind::Body, "Body", "body"),
        (Kind::Title, "Title", "title"),
        (Kind::Heading1, "Heading 1", "heading-1"),
        (Kind::Heading2, "Heading 2", "heading-2"),
        (Kind::Heading3, "Heading 3", "heading-3"),
        (Kind::Quote, "Quote", "quote"),
        (Kind::Code, "Code", "code"),
    ];

    /// What choosing it does to the text's style.
    pub fn apply(self, s: &mut Style) {
        let (size, bold, italic) = match self {
            Kind::Body => (DEFAULT_SIZE, false, false),
            Kind::Title => (28.0, true, false),
            Kind::Heading1 => (22.0, true, false),
            Kind::Heading2 => (18.0, true, false),
            Kind::Heading3 => (14.0, true, false),
            Kind::Quote => (DEFAULT_SIZE, false, true),
            Kind::Code => (DEFAULT_SIZE, false, false),
        };
        s.size = size;
        s.bold = bold;
        s.italic = italic;
        s.family = if self == Kind::Code {
            MONOSPACE.into()
        } else if s.family == MONOSPACE {
            String::new()
        } else {
            std::mem::take(&mut s.family)
        };
    }

    /// Space above and below it, in points.
    pub fn spacing(self) -> (f32, f32) {
        match self {
            Kind::Body | Kind::Code => (0.0, 0.0),
            Kind::Title => (6.0, 10.0),
            Kind::Heading1 => (14.0, 6.0),
            Kind::Heading2 => (12.0, 4.0),
            Kind::Heading3 => (10.0, 2.0),
            Kind::Quote => (4.0, 4.0),
        }
    }

    /// A heading or title is followed by body text (Return at its end).
    fn next(self) -> Kind {
        match self {
            Kind::Title | Kind::Heading1 | Kind::Heading2 | Kind::Heading3 => Kind::Body,
            k => k,
        }
    }
}

/// The family Code paragraphs are set in.
pub const MONOSPACE: &str = "monospace";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum List {
    #[default]
    None,
    Bullet,
    Number,
}

/// How a paragraph is set: its kind, a list it is in, how far it is
/// indented, its own alignment (else the document's), and whether it is
/// a table's row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Para {
    pub kind: Kind,
    pub list: List,
    pub level: u8,
    pub align: Option<Align>,
    pub row: Option<Row>,
    /// Set on the ruler, in points: indents beyond what its kind and
    /// level give (`left` and `right` added to theirs; `first`, the first
    /// line's, further in from the left one), and tab stops from the
    /// text's left edge, left to right.
    pub left: f32,
    pub first: f32,
    pub right: f32,
    pub tabs: Vec<Tab>,
}

/// A tab stop: where, and how it lines up the text after its tab.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tab {
    pub x: f32,
    pub align: TabAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabAlign {
    #[default]
    Left,
    Center,
    Right,
    Decimal,
}

impl TabAlign {
    pub fn to_kit(self) -> editor::TabAlign {
        use editor::TabAlign as A;
        match self {
            TabAlign::Left => A::Left,
            TabAlign::Center => A::Center,
            TabAlign::Right => A::Right,
            TabAlign::Decimal => A::Decimal,
        }
    }

    pub fn from_kit(a: editor::TabAlign) -> TabAlign {
        use editor::TabAlign as A;
        match a {
            A::Left => TabAlign::Left,
            A::Center => TabAlign::Center,
            A::Right => TabAlign::Right,
            A::Decimal => TabAlign::Decimal,
        }
    }
}

/// Where tabs go without stops of a paragraph's own: every half inch.
pub const TAB_EVERY: f32 = 36.0;

/// A table's row: its cells are the paragraph's text, a tab apart (so
/// the file's text is the table's, tab-separated). A table is the rows
/// one after another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Row {
    /// Each column's share of the width (equal ones when there aren't as
    /// many as cells).
    pub columns: Vec<f32>,
    /// The table's header row.
    pub header: bool,
}

impl Para {
    /// The indents its kind, level and list give (left, right), before
    /// the ruler's.
    pub fn base_indents(&self) -> (f32, f32) {
        let list = self.list != List::None;
        let quote = self.kind == Kind::Quote;
        let left = self.level as f32 * INDENT + if list { MARKER_ROOM } else { 0.0 } + if quote { INDENT } else { 0.0 };
        (left, if quote { INDENT } else { 0.0 })
    }

    /// Where its tabs go: its own stops, then one every half inch; code
    /// as code editors have it (every four spaces).
    pub fn tab_stops(&self) -> Option<editor::TabStops> {
        if self.kind == Kind::Code && self.tabs.is_empty() {
            return None;
        }
        let stops = self.tabs.iter().map(|t| editor::TabStop { x: t.x, align: t.align.to_kit() }).collect();
        Some(editor::TabStops { stops, every: TAB_EVERY })
    }

    /// What the ruler shows of it: its indents in all (from the text's
    /// edges) and its tabs.
    pub fn ruler(&self) -> ruler::Setting {
        let (l, r) = self.base_indents();
        ruler::Setting {
            indents: ruler::Indents { left: l + self.left, first: self.first, right: r + self.right },
            tabs: self.tabs.iter().map(|t| editor::TabStop { x: t.x, align: t.align.to_kit() }).collect(),
        }
    }

    /// The ruler's setting made its own: indents beyond its kind's and
    /// level's (none below them), and tabs.
    pub fn set_ruler(&mut self, s: &ruler::Setting) {
        let (l, r) = self.base_indents();
        self.left = (s.indents.left - l).max(0.0);
        self.first = s.indents.first.max(0.0);
        self.right = (s.indents.right - r).max(0.0);
        self.tabs = s.tabs.iter().map(|t| Tab { x: t.x, align: TabAlign::from_kit(t.align) }).collect();
    }
}

/// The deepest indent.
pub const MAX_LEVEL: u8 = 8;
/// An indent level, and the room a list's marker takes, in points.
pub const INDENT: f32 = 24.0;
pub const MARKER_ROOM: f32 = INDENT;

/// A step to undo: bytes `at..at + now` of the text were `was`, and the
/// runs were `runs`. (Only what changed: a long document isn't copied
/// for every word typed.)
#[derive(Clone)]
struct State {
    at: usize,
    now: usize,
    was: String,
    runs: Vec<Run>,
    paras: Vec<Para>,
    /// Selection to restore.
    selection: Range<usize>,
    /// What to call it (Edit ▸ Undo *Typing*).
    name: String,
}

/// What an edit did to the text: bytes `removed` became `inserted` bytes
/// (for the editor to lay out again).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub removed: Range<usize>,
    pub inserted: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

pub struct Doc {
    text: String,
    runs: Vec<Run>,
    /// Where each run starts (to find one without walking them all).
    starts: Vec<usize>,
    /// Each paragraph's setting, and where it starts (a byte after a
    /// newline, or 0).
    paras: Vec<Para>,
    para_starts: Vec<usize>,
    pub align: Align,
    pub wrap: bool,
    /// What printed pages show above and below the text; None: they're
    /// numbered at the foot when there is more than one.
    pub page: Option<Page>,
    /// Style for text typed next at `typing.0` (bytes), set by a style
    /// command with nothing selected.
    pub typing: Option<(usize, Style)>,
    /// Text and styles last copied here, to paste with styles.
    clip: Option<(String, Vec<Run>)>,
    undo: Vec<State>,
    redo: Vec<State>,
    /// The next step's name, as the command making it says (`name_next`).
    naming: Option<String>,
    last_edit: Option<(EditKind, Instant, usize)>,
    /// Bumped on every change (for layout caching and "edited").
    pub generation: u64,
    /// The generation that was last saved or loaded.
    pub saved_generation: u64,
    /// Each change to the text since last taken, where its lines are
    /// (for source code's syntax: `take_edits`); None: not kept.
    edits: Option<Vec<olo_ui::code::Edit>>,
}

/// What each printed page shows above and below its text: left, centre
/// and right parts, where `{page}` and `{pages}` stand for the page's
/// number and how many there are (as `olo_richtext::Document` has them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub header: [String; 3],
    #[serde(default)]
    pub footer: [String; 3],
}

impl Page {
    /// Page `page` of `pages`'s text for `part`.
    pub fn fill(part: &str, page: usize, pages: usize) -> String {
        part.replace("{page}", &page.to_string()).replace("{pages}", &pages.to_string())
    }
}

/// Merge neighbouring runs of the same style and drop empty ones.
fn normalize(runs: &mut Vec<Run>) {
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs.drain(..) {
        if r.len == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.style == r.style => last.len += r.len,
            _ => out.push(r),
        }
    }
    *runs = out;
}

/// Split `runs` so a run boundary falls at byte `at`; returns the index of
/// the run starting there.
fn split_at(runs: &mut Vec<Run>, at: usize) -> usize {
    let mut pos = 0;
    for i in 0..runs.len() {
        if pos == at {
            return i;
        }
        let end = pos + runs[i].len;
        if at < end {
            let tail = Run { len: end - at, style: runs[i].style.clone() };
            runs[i].len = at - pos;
            runs.insert(i + 1, tail);
            return i + 1;
        }
        pos = end;
    }
    runs.len()
}

impl Doc {
    pub fn new(text: String, runs: Option<Vec<Run>>) -> Doc {
        Doc::with_paras(text, runs, None)
    }

    /// A document with its paragraphs set as `paras` says (dropped unless
    /// there is one for each).
    pub fn with_paras(text: String, runs: Option<Vec<Run>>, paras: Option<Vec<Para>>) -> Doc {
        let mut runs = runs.unwrap_or_default();
        // Styles that don't fit the text (edited elsewhere) are dropped.
        let fits = runs.iter().map(|r| r.len).sum::<usize>() == text.len() && {
            let mut pos = 0;
            runs.iter().all(|r| {
                pos += r.len;
                text.is_char_boundary(pos)
            })
        };
        if !fits {
            runs = vec![Run { len: text.len(), style: Style::default() }];
        }
        normalize(&mut runs);
        let count = text.bytes().filter(|&b| b == b'\n').count() + 1;
        let paras = paras.filter(|p| p.len() == count).unwrap_or_else(|| vec![Para::default(); count]);
        let mut d = Doc {
            text,
            runs,
            starts: Vec::new(),
            paras,
            para_starts: Vec::new(),
            align: Align::Left,
            wrap: true,
            page: None,
            typing: None,
            clip: None,
            undo: Vec::new(),
            redo: Vec::new(),
            naming: None,
            last_edit: None,
            generation: 1,
            saved_generation: 1,
            edits: None,
        };
        d.index_runs();
        d.index_paras();
        d
    }

    fn index_paras(&mut self) {
        self.para_starts.clear();
        self.para_starts.push(0);
        self.para_starts.extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
        debug_assert_eq!(self.para_starts.len(), self.paras.len());
    }

    pub fn paras(&self) -> &[Para] {
        &self.paras
    }

    /// Whether it's plain text: no styles, paragraph settings or
    /// alignment of its own (nothing lost shown as code or Markdown).
    pub fn is_plain(&self) -> bool {
        self.runs.iter().all(|r| r.style == Style::default()) && self.paras.iter().all(|p| *p == Para::default()) && self.align == Align::Left
    }

    /// Keep each change to the text, for [`take_edits`](Self::take_edits)
    /// (source code's syntax follows them), or stop.
    pub fn keep_edits(&mut self, on: bool) {
        self.edits = on.then(Vec::new);
    }

    /// The changes to the text since last taken, in order.
    pub fn take_edits(&mut self) -> Vec<olo_ui::code::Edit> {
        self.edits.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Bytes `removed` are about to become `inserted`: kept, if changes
    /// are, with where they are in lines (from the paragraphs' starts).
    fn note_edit(&mut self, removed: &Range<usize>, inserted: &str) {
        if self.edits.is_none() {
            return;
        }
        let place = |at: usize| {
            let i = self.para_index(at);
            (i, at - self.para_starts[i])
        };
        let e = olo_ui::code::Edit::new(removed.clone(), inserted, place(removed.start), place(removed.end));
        if let Some(v) = self.edits.as_mut() {
            v.push(e);
        }
    }

    /// The paragraph holding byte `at` (a newline belongs to the one it
    /// ends).
    pub fn para_index(&self, at: usize) -> usize {
        self.para_starts.partition_point(|&s| s <= at).saturating_sub(1)
    }

    /// Paragraph `i`'s bytes, without its newline.
    pub fn para_range(&self, i: usize) -> Range<usize> {
        let start = self.para_starts[i];
        let end = self.para_starts.get(i + 1).map_or(self.text.len(), |&n| n - 1);
        start..end
    }

    /// The paragraphs bytes `range` touch.
    pub fn paras_in(&self, range: Range<usize>) -> Range<usize> {
        self.para_index(range.start)..self.para_index(range.end) + 1
    }

    /// Paragraph `i`'s number in its list (1 for the first of a numbered
    /// list at its level, counting on past deeper items under it).
    pub fn number(&self, i: usize) -> usize {
        let level = self.paras[i].level;
        let mut n = 1;
        for p in self.paras[..i].iter().rev() {
            if p.list == List::None || p.level < level {
                break;
            }
            if p.level == level {
                if p.list != List::Number {
                    break;
                }
                n += 1;
            }
        }
        n
    }

    /// How paragraph `i` is drawn: indents, spacing, alignment, marker.
    pub fn format(&self, i: usize, align: Align) -> editor::Paragraph {
        let p = &self.paras[i];
        let (above, below) = p.kind.spacing();
        let (base_left, base_right) = p.base_indents();
        let left = base_left + p.left.max(0.0);
        let marker = match p.list {
            List::None => None,
            List::Bullet => Some(["•", "◦", "▪"][p.level as usize % 3].to_owned()),
            List::Number => Some(format!("{}.", self.number(i))),
        };
        let a = p.align.unwrap_or(align);
        let halign = Some(a.to_kit());
        if let Some(r) = &p.row {
            return editor::Paragraph {
                left: p.level as f32 * INDENT,
                align: halign,
                justify: Some(a == Align::Justify),
                row: Some(editor::Row { columns: r.columns.clone(), header: r.header, cells: None, aligns: Vec::new() }),
                ..Default::default()
            };
        }
        editor::Paragraph {
            left,
            first: p.first.max(0.0),
            right: base_right + p.right.max(0.0),
            above,
            below,
            align: halign,
            justify: Some(a == Align::Justify),
            marker,
            row: None,
            tabs: p.tab_stops(),
            // (Its pictures' images are the window's: `Styled::paragraph`.)
            pictures: Vec::new(),
            ..Default::default()
        }
    }

    /// Change the paragraphs bytes `range` touch with `f`, as one step to
    /// undo.
    pub fn set_paras(&mut self, range: Range<usize>, selection: Range<usize>, f: impl Fn(&mut Para)) {
        let r = self.paras_in(range);
        let mut changed = self.paras.clone();
        for p in &mut changed[r] {
            f(p);
        }
        if changed == self.paras {
            return;
        }
        self.push_undo(selection.start..selection.start, 0, selection, "Paragraph");
        self.last_edit = None;
        self.paras = changed;
        self.redo.clear();
        self.generation += 1;
    }

    /// Set the paragraphs bytes `range` touch as `kind`, their text
    /// restyled to match, as one step to undo.
    pub fn set_kind(&mut self, range: Range<usize>, selection: Range<usize>, kind: Kind) {
        let r = self.paras_in(range.clone());
        let bytes = self.para_range(r.start).start..self.para_range(r.end - 1).end;
        self.push_undo(selection.start..selection.start, 0, selection, "Paragraph Style");
        self.last_edit = None;
        for p in &mut self.paras[r] {
            p.kind = kind;
        }
        if !bytes.is_empty() {
            let start = split_at(&mut self.runs, bytes.start);
            let end = split_at(&mut self.runs, bytes.end);
            for run in &mut self.runs[start..end] {
                kind.apply(&mut run.style);
            }
            normalize(&mut self.runs);
            self.index_runs();
        }
        // Typed next in an empty one: in its style.
        if bytes.is_empty() {
            let mut style = self.style_at(bytes.start);
            kind.apply(&mut style);
            self.typing = Some((bytes.start, style));
        }
        self.redo.clear();
        self.generation += 1;
    }

    fn index_runs(&mut self) {
        self.starts.clear();
        let mut pos = 0;
        for r in &self.runs {
            self.starts.push(pos);
            pos += r.len;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn edited(&self) -> bool {
        self.generation != self.saved_generation
    }

    /// The style of the character before byte `at` (what typing there
    /// continues), or after it at the start.
    pub fn style_at(&self, at: usize) -> Style {
        if let Some((pos, style)) = &self.typing
            && *pos == at
        {
            return style.clone();
        }
        // The run with the character before `at` (the first, at 0).
        let i = if at == 0 { 0 } else { self.starts.partition_point(|&s| s < at).saturating_sub(1) };
        self.runs.get(i).or(self.runs.last()).map(|r| r.style.clone()).unwrap_or_default()
    }

    /// The text became `new` (a test's way of typing): what was replaced
    /// goes through [`edit`](Self::edit).
    #[cfg(test)]
    pub fn sync(&mut self, new: &str, selection: Range<usize>) {
        let (removed, inserted) = editor::difference(&self.text, new);
        let inserted = new[removed.start..removed.start + inserted].to_owned();
        self.edit(removed, &inserted, selection);
    }

    /// Replace bytes `removed` with `inserted`, as typed: the styles move
    /// to match, and typing (or deleting) runs together into one step to
    /// undo. `selection` is the selection before the change, for undo.
    pub fn edit(&mut self, removed: Range<usize>, inserted: &str, selection: Range<usize>) {
        if removed.is_empty() && inserted.is_empty() {
            return;
        }
        let p = removed.start;
        // Typing over a selection starts a step that what's typed next
        // joins, so one undo brings the selection back.
        let kind = match (removed.is_empty(), inserted.chars().count()) {
            (_, 1) if !inserted.contains(char::is_whitespace) => EditKind::Typing,
            (false, 0) if removed.len() <= 4 => EditKind::Deleting,
            _ => EditKind::Other,
        };
        let joins = matches!(self.last_edit, Some((k, t, at)) if k == kind && kind != EditKind::Other
            && t.elapsed() < Duration::from_secs(2)
            && (at == p || at == removed.end))
            && (kind != EditKind::Typing || removed.is_empty())
            && !self.undo.is_empty();
        if joins {
            self.extend_undo(removed.clone(), inserted.len());
        } else {
            let name = match kind {
                EditKind::Typing => "Typing",
                EditKind::Deleting => "Delete",
                EditKind::Other if inserted.is_empty() => "Delete",
                EditKind::Other => "Typing",
            };
            self.push_undo(removed.clone(), inserted.len(), selection, name);
        }
        self.last_edit = Some((kind, Instant::now(), p + inserted.len()));
        self.splice(removed, inserted);
        self.redo.clear();
        self.generation += 1;
    }

    /// Replace bytes `removed` with `inserted` in the text and the runs.
    /// Pasting what was copied here brings its styles; other text takes
    /// the pending typing style, the style of what it replaces, or the
    /// style of the character before it.
    fn splice(&mut self, removed: Range<usize>, inserted: &str) {
        // (Where its lines were, before they move.)
        self.note_edit(&removed, inserted);
        let p = removed.start;
        let added = match &self.clip {
            Some((text, runs)) if text == inserted => runs.clone(),
            _ => {
                let style = match &self.typing {
                    Some((pos, style)) if *pos == p => style.clone(),
                    _ => {
                        let mut s = if removed.is_empty() { self.style_at(p) } else { self.style_at(p + self.text[p..].chars().next().map_or(0, char::len_utf8)) };
                        // (Text typed beside a picture is text.)
                        s.picture = None;
                        s
                    }
                };
                vec![Run { len: inserted.len(), style }]
            }
        };
        let start = split_at(&mut self.runs, removed.start);
        let end = split_at(&mut self.runs, removed.end);
        self.runs.splice(start..end, added);
        normalize(&mut self.runs);
        // The paragraphs the removed bytes spanned become one (the
        // first's setting); newlines put back in their place start ones set
        // as those were (a table's rows stay rows when the text between
        // goes), and any more start ones set like the first (body text
        // after a heading's end).
        let (p0, p1) = (self.para_index(removed.start), self.para_index(removed.end));
        let first = self.paras[p0].clone();
        let at_end = self.para_range(p1).end == removed.end;
        let new_starts: Vec<usize> = inserted.match_indices('\n').map(|(i, _)| p + i + 1).collect();
        let next = if at_end && inserted.ends_with('\n') { Para { kind: first.kind.next(), ..first.clone() } } else { first.clone() };
        let were = &self.paras[p0 + 1..p1 + 1];
        let mut added_paras: Vec<Para> = (0..new_starts.len()).map(|k| were.get(k).cloned().unwrap_or_else(|| first.clone())).collect();
        if new_starts.len() > were.len()
            && let Some(last) = added_paras.last_mut()
        {
            *last = next;
        }
        let delta = inserted.len() as isize - removed.len() as isize;
        self.paras.splice(p0 + 1..p1 + 1, added_paras);
        let n = new_starts.len();
        self.para_starts.splice(p0 + 1..p1 + 1, new_starts);
        for s in &mut self.para_starts[p0 + 1 + n..] {
            *s = (*s as isize + delta) as usize;
        }
        self.text.replace_range(removed, inserted);
        if self.runs.is_empty() && !self.text.is_empty() {
            self.runs.push(Run { len: self.text.len(), style: Style::default() });
        }
        self.index_runs();
        debug_assert_eq!(self.runs.iter().map(|r| r.len).sum::<usize>(), self.text.len());
        // Typing at the remembered spot used the pending style; it moves on.
        self.typing = self.typing.take().filter(|(pos, _)| *pos == p && inserted.is_empty());
    }

    /// Replace every range (bytes, in order, not overlapping) with `with`,
    /// as one undoable step.
    pub fn replace_all(&mut self, ranges: &[Range<usize>], with: &str, selection: Range<usize>) -> Option<Change> {
        let (first, last) = (ranges.first()?, ranges.last()?);
        let removed = first.start..last.end;
        let inserted = removed.len() + ranges.len() * with.len() - ranges.iter().map(|r| r.len()).sum::<usize>();
        self.push_undo(removed.clone(), inserted, selection, if ranges.len() > 1 { "Replace All" } else { "Replace" });
        self.last_edit = None;
        self.typing = None;
        for r in ranges.iter().rev() {
            self.splice(r.clone(), with);
        }
        self.redo.clear();
        self.generation += 1;
        Some(Change { removed, inserted })
    }

    /// Before bytes `removed` become `inserted` bytes: a step to undo it.
    fn push_undo(&mut self, removed: Range<usize>, inserted: usize, selection: Range<usize>, name: &str) {
        // (Text alone, source code's or Markdown's, has no paragraph
        // settings to keep: a long one's aren't copied for each step.)
        let paras = if self.edits.is_some() { Vec::new() } else { self.paras.clone() };
        let name = self.naming.take().unwrap_or_else(|| name.to_owned());
        self.undo.push(State { at: removed.start, now: inserted, was: self.text[removed].to_owned(), runs: self.runs.clone(), paras, selection, name });
        // Plenty, without keeping every keystroke of a long session.
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    /// Before bytes `removed` become `inserted` bytes, next to (or in) what
    /// the last step changed: that step takes this in too.
    fn extend_undo(&mut self, removed: Range<usize>, inserted: usize) {
        let text = &self.text;
        let Some(top) = self.undo.last_mut() else { return };
        let (lo, hi) = (removed.start.min(top.at), removed.end.max(top.at + top.now));
        let mut was = String::with_capacity(hi - lo);
        was.push_str(&text[lo..top.at]);
        was.push_str(&top.was);
        was.push_str(&text[(top.at + top.now).min(hi)..hi]);
        top.was = was;
        top.now = hi - lo - removed.len() + inserted;
        top.at = lo;
    }

    /// Change the style over `range` (bytes) with `f`. With nothing
    /// selected, the change applies to what is typed next there.
    pub fn restyle(&mut self, range: Range<usize>, selection: Range<usize>, f: impl Fn(&mut Style)) {
        if range.is_empty() {
            let mut style = self.style_at(range.start);
            f(&mut style);
            self.typing = Some((range.start, style));
            return;
        }
        self.push_undo(range.start..range.start, 0, selection, "Style");
        self.last_edit = None;
        let start = split_at(&mut self.runs, range.start);
        let end = split_at(&mut self.runs, range.end);
        for r in &mut self.runs[start..end] {
            f(&mut r.style);
        }
        normalize(&mut self.runs);
        self.index_runs();
        self.redo.clear();
        self.generation += 1;
    }

    /// Move bytes `from` to byte `to` (of the text before the move),
    /// styles and all, as one undoable step (text dragged within the
    /// document). Where it went, in bytes.
    pub fn move_text(&mut self, from: Range<usize>, to: usize, selection: Range<usize>) -> Range<usize> {
        if (from.start..=from.end).contains(&to) {
            return from;
        }
        let text = self.text[from.clone()].to_owned();
        let clip = self.clip.take();
        self.copied(from.clone());
        let region = from.start.min(to)..from.end.max(to);
        self.push_undo(region.clone(), region.len(), selection, "Move");
        self.last_edit = None;
        self.typing = None;
        self.splice(from.clone(), "");
        let at = if to > from.end { to - from.len() } else { to };
        self.splice(at..at, &text);
        self.clip = clip;
        self.redo.clear();
        self.generation += 1;
        at..at + text.len()
    }

    /// Remember a copy, so pasting it here keeps its styles.
    pub fn copied(&mut self, range: Range<usize>) {
        let text = self.text[range.clone()].to_owned();
        let mut runs = self.runs.clone();
        let start = split_at(&mut runs, range.start);
        let end = split_at(&mut runs, range.end);
        self.clip = Some((text, runs[start..end].to_vec()));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// The step the command about to run makes, named (Edit ▸ Undo *Bold*)
    /// in place of what the edit would call it; `None` again after.
    pub fn name_next(&mut self, name: Option<&str>) {
        self.naming = name.map(str::to_owned);
    }

    /// What Undo and Redo would take back, by name.
    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|s| s.name.as_str())
    }

    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|s| s.name.as_str())
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back; returns the selection to show and what changed.
    pub fn undo(&mut self, selection: Range<usize>) -> Option<(Range<usize>, Change)> {
        let prev = self.undo.pop()?;
        let (to, change) = self.restore(prev, selection, false);
        Some((to, change))
    }

    pub fn redo(&mut self, selection: Range<usize>) -> Option<(Range<usize>, Change)> {
        let next = self.redo.pop()?;
        Some(self.restore(next, selection, true))
    }

    /// Put `s` back, keeping what it replaces on the other stack.
    fn restore(&mut self, s: State, selection: Range<usize>, redoing: bool) -> (Range<usize>, Change) {
        let removed = s.at..s.at + s.now;
        // (A step taken of text alone keeps no paragraph settings: there
        // is always one a line otherwise.)
        let text_only = s.paras.is_empty();
        let back = State {
            at: s.at,
            now: s.was.len(),
            was: self.text[removed.clone()].to_owned(),
            runs: std::mem::replace(&mut self.runs, s.runs),
            paras: if text_only { Vec::new() } else { std::mem::replace(&mut self.paras, s.paras) },
            selection,
            name: s.name,
        };
        self.note_edit(&removed, &s.was);
        self.text.replace_range(removed.clone(), &s.was);
        self.index_runs();
        if text_only {
            // One plain paragraph a line, as many as there are now.
            let n = self.text.bytes().filter(|&b| b == b'\n').count() + 1;
            self.paras.resize(n, Para::default());
        }
        self.index_paras();
        if redoing {
            self.undo.push(back)
        } else {
            self.redo.push(back)
        }
        self.last_edit = None;
        self.typing = None;
        self.generation += 1;
        (s.selection, Change { removed, inserted: s.was.len() })
    }

    /// Put picture `pic` in place of bytes `range`: its character in the
    /// style around it, as one step to undo.
    pub fn insert_picture(&mut self, range: Range<usize>, pic: Picture, selection: Range<usize>) -> Change {
        let mut style = self.style_at(range.start);
        style.picture = Some(pic);
        style.link = None;
        let ch = PICTURE.to_string();
        self.push_undo(range.clone(), ch.len(), selection, "Insert Picture");
        self.last_edit = None;
        let clip = self.clip.take();
        self.typing = Some((range.start, style));
        self.splice(range.clone(), &ch);
        self.typing = None;
        self.clip = clip;
        self.redo.clear();
        self.generation += 1;
        Change { removed: range, inserted: ch.len() }
    }

    /// The pictures in bytes `range` (a paragraph): each one's character's
    /// place from `range.start`, and the picture.
    pub fn pictures_in(&self, range: Range<usize>) -> Vec<(usize, &Picture)> {
        let mut out = Vec::new();
        let mut i = self.starts.partition_point(|&s| s <= range.start).saturating_sub(1);
        while i < self.runs.len() && self.starts[i] < range.end {
            if let Some(pic) = &self.runs[i].style.picture {
                let (a, b) = (self.starts[i].max(range.start), (self.starts[i] + self.runs[i].len).min(range.end));
                for (k, c) in self.text[a..b].char_indices() {
                    if c == PICTURE {
                        out.push((a + k - range.start, pic));
                    }
                }
            }
            i += 1;
        }
        out
    }

    /// Replace `range` (bytes) with `with`, as one undoable step (Replace).
    pub fn replace(&mut self, range: Range<usize>, with: &str, selection: Range<usize>) -> Change {
        self.replace_all(std::slice::from_ref(&range), with, selection).unwrap_or(Change { removed: range.start..range.start, inserted: 0 })
    }

    /// Replace every range (bytes, in order, not overlapping; `with` each)
    /// and then set the paragraphs bytes `after` (of the text then) touch,
    /// `set` given each one's place among them: one step to undo (a
    /// table's rows made, added or taken away).
    pub fn replace_paras(&mut self, ranges: &[Range<usize>], with: &str, selection: Range<usize>, after: impl Fn(&Doc) -> Range<usize>, set: impl Fn(usize, &mut Para)) -> Option<Change> {
        let change = if ranges.iter().all(|r| r.is_empty()) && with.is_empty() {
            // Only the paragraphs: a step of its own still.
            self.push_undo(selection.start..selection.start, 0, selection, "Table");
            self.redo.clear();
            self.generation += 1;
            Change { removed: 0..0, inserted: 0 }
        } else {
            self.replace_all(ranges, with, selection)?
        };
        self.last_edit = None;
        let r = self.paras_in(after(self));
        for (k, i) in r.enumerate() {
            set(k, &mut self.paras[i]);
        }
        Some(change)
    }

    /// The styles of bytes `range` (one paragraph) as the editor's spans,
    /// in `out`. `font` gives the face a style is drawn in; a span with no
    /// colour of its own is in the text's.
    pub fn spans(&self, range: Range<usize>, font: &mut dyn FnMut(&Style) -> Font, out: &mut Vec<Span>) {
        if range.is_empty() {
            // An empty line still needs a height (for the caret).
            out.push(span(0, &self.style_at(range.start), font));
            return;
        }
        let mut i = self.starts.partition_point(|&s| s <= range.start).saturating_sub(1);
        while i < self.runs.len() && self.starts[i] < range.end {
            let (a, b) = (self.starts[i].max(range.start), (self.starts[i] + self.runs[i].len).min(range.end));
            if b > a {
                let style = &self.runs[i].style;
                match &style.picture {
                    // A picture's character as big as the picture (and any
                    // other character in its run as text).
                    Some(pic) => {
                        let text = Style { picture: None, ..style.clone() };
                        for (_, c) in self.text[a..b].char_indices() {
                            if c == PICTURE {
                                out.push(Span { picture: Some((pic.width, pic.height)), ..span(c.len_utf8(), style, font) });
                            } else {
                                out.push(span(c.len_utf8(), &text, font));
                            }
                        }
                    }
                    None => out.push(span(b - a, style, font)),
                }
            }
            i += 1;
        }
    }
}

/// A run's span (`len` bytes) as the editor draws it.
fn span(len: usize, s: &Style, font: &mut dyn FnMut(&Style) -> Font) -> Span {
    Span {
        len,
        font: font(s),
        size: s.size,
        color: s.color.map(|[r, g, b]| Hsla::from(Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 })),
        underline: s.underline,
        strike: s.strikeout,
        picture: None,
        hidden: false,
        background: None,
    }
}

/// Find `needle` in `hay` from byte `from`: forwards (the first match
/// starting at or after it) or backwards (the last one ending at or before
/// it), wrapping round the end if asked. Byte range of the match.
pub fn find(hay: &str, needle: &str, from: usize, backwards: bool, case: bool, wrap: bool) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let needle: Vec<char> = needle.chars().map(fold).collect();
    // The match starting at byte `i`, if there is one.
    let at = |i: usize| -> Option<Range<usize>> {
        let mut it = hay[i..].char_indices();
        for n in &needle {
            let (_, c) = it.next()?;
            if fold(c) != *n {
                return None;
            }
        }
        Some(i..it.next().map_or(hay.len(), |(j, _)| i + j))
    };
    let starts: Vec<usize> = hay.char_indices().map(|(i, _)| i).collect();
    let from = from.min(hay.len());
    if backwards {
        let before = starts.iter().rev().filter(|&&i| i < from).find_map(|&i| at(i).filter(|r| r.end <= from));
        before.or_else(|| if wrap { starts.iter().rev().find_map(|&i| at(i)) } else { None })
    } else {
        let after = starts.iter().filter(|&&i| i >= from).find_map(|&i| at(i));
        after.or_else(|| if wrap { starts.iter().find_map(|&i| at(i)) } else { None })
    }
}

/// Every match of `needle`, in order, not overlapping.
pub fn find_all(hay: &str, needle: &str, case: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(r) = find(hay, needle, from, false, case, false) {
        from = r.end;
        out.push(r);
    }
    out
}

// ---- files ----------------------------------------------------------------

const ATTR_STYLES: &str = "user.olo.styles";
const ATTR_ALIGN: &str = "user.olo.alignment";
const ATTR_WRAP: &str = "user.olo.wrap";
const ATTR_PARAS: &str = "user.olo.paragraphs";
const ATTR_PAGE: &str = "user.olo.page";

/// What a file holds.
pub struct Loaded {
    pub text: String,
    pub runs: Option<Vec<Run>>,
    pub paras: Option<Vec<Para>>,
    pub align: Align,
    pub wrap: bool,
    pub page: Option<Page>,
    /// Not UTF-8: read as Latin-1 (saved back as UTF-8).
    pub latin1: bool,
    /// We may not write it.
    pub read_only: bool,
}

/// Attribute `name`, or what it was called before the rename (Tabula:
/// `user.tabula.…`) on a file last saved then.
fn get_attr(path: &Path, name: &str) -> Option<Vec<u8>> {
    attr(path, name).or_else(|| attr(path, &name.replacen("user.olo.", "user.tabula.", 1)))
}

fn attr(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let n = std::ffi::CString::new(name).ok()?;
    // SAFETY: valid C strings; a null buffer asks for the size.
    let size = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: buf has room for `size` bytes.
    let got = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (got >= 0).then(|| {
        buf.truncate(got as usize);
        buf
    })
}

fn set_attr(path: &Path, name: &str, value: &[u8]) {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(p), Ok(n)) = (std::ffi::CString::new(path.as_os_str().as_bytes()), std::ffi::CString::new(name)) else {
        return;
    };
    // SAFETY: valid C strings and buffer. Failure (a filesystem without
    // user attributes) just loses the styles.
    unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
}

/// Read a file (on a worker thread).
pub fn load(path: &Path) -> Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (text, latin1) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (e.into_bytes().iter().map(|&b| b as char).collect(), true),
    };
    let mut runs = get_attr(path, ATTR_STYLES).and_then(|v| serde_json::from_slice::<Vec<Run>>(&v).ok());
    if let Some(runs) = &mut runs {
        crate::pictures::absolute(runs, path);
    }
    let paras = get_attr(path, ATTR_PARAS).and_then(|v| serde_json::from_slice::<Vec<Para>>(&v).ok());
    let align = get_attr(path, ATTR_ALIGN).and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or_default();
    let wrap = get_attr(path, ATTR_WRAP).is_none_or(|v| v != b"false");
    let page = get_attr(path, ATTR_PAGE).and_then(|v| serde_json::from_slice(&v).ok());
    let read_only = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(path.as_os_str().as_bytes())
            // SAFETY: a valid C string.
            .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } != 0)
    };
    Ok(Loaded { text, runs: if latin1 { None } else { runs }, paras: if latin1 { None } else { paras }, align, wrap, page, latin1, read_only })
}

/// Write a file whole (beside it, then renamed over it, keeping its
/// permissions), styles in its attributes. On a worker thread.
pub fn save(path: &Path, text: &str, runs: &[Run], paras: &[Para], align: Align, wrap: bool, page: Option<&Page>) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    let name = path.file_name().ok_or("no name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.saving"));
    let res = (|| {
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        // Plain text needs no styles attribute.
        let plain = runs.len() <= 1 && runs.first().is_none_or(|r| r.style == Style::default());
        if !plain {
            set_attr(&tmp, ATTR_STYLES, &serde_json::to_vec(&crate::pictures::settle(runs, path)).unwrap_or_default());
        }
        if align != Align::Left {
            set_attr(&tmp, ATTR_ALIGN, &serde_json::to_vec(&align).unwrap_or_default());
        }
        if paras.iter().any(|p| *p != Para::default()) {
            set_attr(&tmp, ATTR_PARAS, &serde_json::to_vec(paras).unwrap_or_default());
        }
        if !wrap {
            set_attr(&tmp, ATTR_WRAP, b"false");
        }
        if let Some(page) = page {
            set_attr(&tmp, ATTR_PAGE, &serde_json::to_vec(page).unwrap_or_default());
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Style {
        Style { bold: true, ..Style::default() }
    }

    fn lens(d: &Doc) -> Vec<(usize, bool)> {
        d.runs.iter().map(|r| (r.len, r.style.bold)).collect()
    }

    #[test]
    fn typing_continues_the_style_before() {
        let mut d = Doc::new("hello world".into(), None);
        d.restyle(0..5, 0..0, |s| s.bold = true);
        assert_eq!(lens(&d), [(5, true), (6, false)]);
        d.sync("hello! world", 5..5);
        assert_eq!(lens(&d), [(6, true), (6, false)]);
        d.sync("Xhello! world", 0..0);
        assert_eq!(lens(&d), [(7, true), (6, false)]);
    }

    #[test]
    fn deleting_across_runs() {
        let mut d = Doc::new("abcdef".into(), None);
        d.restyle(2..4, 0..0, |s| s.bold = true);
        assert_eq!(lens(&d), [(2, false), (2, true), (2, false)]);
        d.sync("af", 1..5);
        assert_eq!(lens(&d), [(2, false)]);
        assert_eq!(d.text(), "af");
    }

    #[test]
    fn pending_style_for_typing() {
        let mut d = Doc::new("ab".into(), None);
        d.restyle(1..1, 1..1, |s| s.bold = true);
        d.sync("aXb", 1..1);
        assert_eq!(lens(&d), [(1, false), (1, true), (1, false)]);
        // It sticks for the next character too (typing continues it).
        d.sync("aXYb", 2..2);
        assert_eq!(lens(&d), [(1, false), (2, true), (1, false)]);
    }

    #[test]
    fn multibyte_edits() {
        let mut d = Doc::new("héllo".into(), None);
        d.restyle(0..3, 0..0, |s| s.bold = true);
        d.sync("hèllo", 1..2);
        assert_eq!(d.text(), "hèllo");
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), "hèllo".len());
        d.sync("h€llo", 1..2);
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), "h€llo".len());
    }

    #[test]
    fn undo_redo_and_coalescing() {
        let mut d = Doc::new(String::new(), None);
        for (i, t) in ["h", "he", "hel", "hell", "hello"].iter().enumerate() {
            d.sync(t, i..i);
        }
        d.sync("hello ", 5..5);
        d.sync("hello w", 6..6);
        // "hello" was one step; the space and "w" are more.
        d.undo(7..7);
        d.undo(6..6);
        assert_eq!(d.text(), "hello");
        d.undo(5..5);
        assert_eq!(d.text(), "");
        assert!(!d.can_undo());
        d.redo(0..0);
        assert_eq!(d.text(), "hello");
        d.restyle(0..5, 0..5, |s| *s = bold());
        d.undo(0..5);
        assert_eq!(lens(&d), [(5, false)]);
        d.redo(0..5);
        assert_eq!(lens(&d), [(5, true)]);
    }

    /// Each step named: typing, a command's own (Bold), a deletion; the
    /// names going back and forth with Undo and Redo, and a name given
    /// for one step only.
    #[test]
    fn steps_are_named() {
        let mut d = Doc::new(String::new(), None);
        d.edit(0..0, "h", 0..0);
        d.edit(1..1, "i", 1..1);
        assert_eq!(d.undo_name(), Some("Typing"));
        d.name_next(Some("Bold"));
        d.restyle(0..2, 0..2, |s| *s = bold());
        d.name_next(None);
        assert_eq!(d.undo_name(), Some("Bold"));
        d.edit(1..2, "", 2..2);
        assert_eq!(d.undo_name(), Some("Delete"));
        d.undo(1..1);
        assert_eq!((d.undo_name(), d.redo_name()), (Some("Bold"), Some("Delete")));
        d.undo(2..2);
        assert_eq!((d.undo_name(), d.redo_name()), (Some("Typing"), Some("Bold")));
        d.redo(2..2);
        assert_eq!(d.redo_name(), Some("Delete"));
        d.edit(2..2, "!", 2..2);
        assert_eq!(d.undo_name(), Some("Typing"), "a name given once, not kept");
    }

    #[test]
    fn undo_keeps_just_what_changed() {
        let mut d = Doc::new("one two".into(), None);
        d.edit(7..7, "s", 7..7);
        d.edit(8..8, "x", 8..8);
        // Backspacing over both: a step of its own.
        d.edit(8..9, "", 9..9);
        d.edit(7..8, "", 8..8);
        assert_eq!(d.text(), "one two");
        d.undo(7..7);
        assert_eq!(d.text(), "one twosx");
        d.undo(9..9);
        assert_eq!(d.text(), "one two");
        d.redo(7..7);
        assert_eq!(d.text(), "one twosx");
        // Forward deletes run together too, and say what they put back.
        let mut d = Doc::new("abcdef".into(), None);
        d.edit(2..3, "", 2..2);
        d.edit(2..3, "", 2..2);
        assert_eq!(d.text(), "abef");
        let (sel, change) = d.undo(2..2).unwrap();
        assert_eq!((d.text(), sel, change), ("abcdef", 2..2, Change { removed: 2..2, inserted: 2 }));
        // Typing over a selection undoes in one step, selection and all.
        let mut d = Doc::new("one two".into(), None);
        d.edit(4..7, "a", 4..7);
        d.edit(5..5, "b", 5..5);
        assert_eq!(d.text(), "one ab");
        let (sel, _) = d.undo(6..6).unwrap();
        assert_eq!((d.text(), sel), ("one two", 4..7));
    }

    #[test]
    fn paste_keeps_copied_styles() {
        let mut d = Doc::new("ab cd".into(), None);
        d.restyle(0..2, 0..0, |s| s.bold = true);
        d.copied(0..3);
        d.sync("ab cdab ", 5..5);
        assert_eq!(lens(&d), [(2, true), (3, false), (2, true), (1, false)]);
    }

    #[test]
    fn moving_keeps_styles() {
        let mut d = Doc::new("ab cd ef".into(), None);
        d.restyle(0..2, 0..0, |s| s.bold = true);
        assert_eq!(d.move_text(0..3, 8, 0..3), 5..8);
        assert_eq!(d.text(), "cd efab ");
        assert_eq!(lens(&d), [(5, false), (2, true), (1, false)]);
        d.undo(5..8);
        assert_eq!(d.text(), "ab cd ef");
    }

    #[test]
    fn mismatched_styles_are_dropped() {
        let runs = vec![Run { len: 99, style: bold() }];
        let d = Doc::new("short".into(), Some(runs));
        assert_eq!(lens(&d), [(5, false)]);
    }

    #[test]
    fn replacing_takes_the_replaced_style() {
        let mut d = Doc::new("ab cd".into(), None);
        d.restyle(3..5, 0..0, |s| s.bold = true);
        d.sync("ab X", 3..5);
        assert_eq!(lens(&d), [(3, false), (1, true)]);
    }

    #[test]
    fn replace_all_keeps_styles_between() {
        let mut d = Doc::new("cat dog cat".into(), None);
        d.restyle(4..7, 0..0, |s| s.bold = true);
        let ranges = find_all(d.text(), "CAT", false);
        assert_eq!(ranges, [0..3, 8..11]);
        d.replace_all(&ranges, "mouse", 0..0);
        assert_eq!(d.text(), "mouse dog mouse");
        assert_eq!(lens(&d), [(6, false), (3, true), (6, false)]);
        d.undo(0..0);
        assert_eq!(d.text(), "cat dog cat");
    }

    #[test]
    fn finding() {
        let t = "Straße café CAFÉ";
        assert_eq!(find(t, "café", 0, false, true, false), Some(8..13));
        assert_eq!(find(t, "café", 9, false, false, false), Some(14..19));
        assert_eq!(find(t, "café", 9, false, true, false), None);
        assert_eq!(find(t, "café", 9, false, true, true), Some(8..13));
        assert_eq!(find(t, "café", 19, true, false, false), Some(14..19));
        assert_eq!(find(t, "café", 14, true, false, false), Some(8..13));
        assert_eq!(find(t, "ß", 0, false, true, false), Some(4..6));
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("styled-edit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.txt");
        let mut d = Doc::new("Hello, world".into(), None);
        d.restyle(0..5, 0..0, |s| {
            s.bold = true;
            s.color = Some([200, 0, 0]);
        });
        let page = Page { header: [String::new(), "Notes".into(), String::new()], footer: [String::new(), String::new(), "{page} of {pages}".into()] };
        save(&path, d.text(), d.runs(), d.paras(), Align::Center, false, Some(&page)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Hello, world");
        let l = load(&path).unwrap();
        assert_eq!(l.text, "Hello, world");
        // Some filesystems (tmpfs on old kernels) refuse user attributes.
        if let Some(runs) = l.runs {
            assert_eq!(runs, d.runs);
            assert_eq!(l.align, Align::Center);
            assert!(!l.wrap);
            assert_eq!(l.page, Some(page));
        }
        std::fs::write(&path, [b'c', b'a', b'f', 0xE9]).unwrap();
        let l = load(&path).unwrap();
        assert_eq!(l.text, "café");
        assert!(l.latin1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn styles_saved_before_the_rename_are_read() {
        let dir = std::env::temp_dir().join(format!("styled-edit-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.txt");
        std::fs::write(&path, "centred").unwrap();
        set_attr(&path, "user.tabula.alignment", b"\"Center\"");
        // Where the filesystem keeps user attributes.
        if attr(&path, "user.tabula.alignment").is_some() {
            assert_eq!(load(&path).unwrap().align, Align::Center);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn kinds(d: &Doc) -> Vec<Kind> {
        d.paras().iter().map(|p| p.kind).collect()
    }

    #[test]
    fn paragraphs_follow_the_text() {
        let mut d = Doc::new("Title\nbody".into(), None);
        d.set_kind(0..0, 0..0, Kind::Heading1);
        assert_eq!(kinds(&d), [Kind::Heading1, Kind::Body]);
        assert!(d.runs()[0].style.bold && d.runs()[0].style.size == 22.0);
        // Return at a heading's end: body text after it.
        d.sync("Title\n\nbody", 5..5);
        assert_eq!(kinds(&d), [Kind::Heading1, Kind::Body, Kind::Body]);
        // In its middle: both halves stay headings.
        d.sync("Ti\ntle\n\nbody", 2..2);
        assert_eq!(kinds(&d), [Kind::Heading1, Kind::Heading1, Kind::Body, Kind::Body]);
        // Joining paragraphs keeps the first's setting.
        d.sync("Title\n\nbody", 3..3);
        assert_eq!(kinds(&d), [Kind::Heading1, Kind::Body, Kind::Body]);
        assert_eq!((d.para_range(0), d.para_range(1), d.para_range(2)), (0..5, 6..6, 7..11));
        // Undo brings the paragraphs back with the text.
        d.undo(0..0);
        assert_eq!(kinds(&d), [Kind::Heading1, Kind::Heading1, Kind::Body, Kind::Body]);
        assert_eq!(d.para_range(1), 3..6);
    }

    #[test]
    fn lists_number_and_nest() {
        let mut d = Doc::new("a\nb\nsub\nc\nafter".into(), None);
        d.set_paras(0..8, 0..0, |p| p.list = List::Number);
        d.set_paras(4..5, 0..0, |p| p.level = 1);
        assert_eq!([d.number(0), d.number(1), d.number(2), d.number(3)], [1, 2, 1, 3]);
        assert_eq!(d.format(2, Align::Left).marker.as_deref(), Some("1."));
        assert_eq!(d.format(4, Align::Left).marker, None);
        // A new item from Return (at the end of the last) is in the list.
        d.edit(9..9, "\n", 9..9);
        d.edit(10..10, "d", 10..10);
        assert_eq!(d.text(), "a\nb\nsub\nc\nd\nafter");
        assert_eq!(d.paras()[4].list, List::Number);
        assert_eq!(d.number(4), 4);
        let f = d.format(2, Align::Left);
        assert_eq!(f.left, INDENT + MARKER_ROOM);
    }

    #[test]
    fn paragraphs_saved_and_loaded() {
        let dir = std::env::temp_dir().join(format!("olo-se-paras-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.txt");
        let mut d = Doc::new("Head\nitem".into(), None);
        d.set_kind(0..0, 0..0, Kind::Title);
        d.set_paras(5..5, 0..0, |p| p.list = List::Bullet);
        save(&path, d.text(), d.runs(), d.paras(), Align::Left, true, None).unwrap();
        let l = load(&path).unwrap();
        if l.paras.is_some() {
            let back = Doc::with_paras(l.text, l.runs, l.paras);
            assert_eq!(back.paras(), d.paras());
        }
        // Edited elsewhere (a paragraph more): the setting no longer fits.
        let other = Doc::with_paras("one\ntwo\nthree".into(), None, Some(d.paras().to_vec()));
        assert!(other.paras().iter().all(|p| *p == Para::default()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    #[test]
    fn typing_in_a_long_document_is_quick() {
        let line = "    let x = foo(bar, baz); // a comment that goes on\n";
        let text = line.repeat(100_000);
        let mut d = Doc::new(text, None);
        d.keep_edits(true);
        let mid = d.text().len() / 2;
        let t = std::time::Instant::now();
        d.edit(mid..mid, "a", mid..mid);
        let first = t.elapsed();
        let t = std::time::Instant::now();
        d.edit(mid + 1..mid + 1, "b", mid + 1..mid + 1);
        let next = t.elapsed();
        eprintln!("first key {first:?}, next {next:?}");
        // Undone and done again, text alone.
        d.edit(mid..mid, "line\nbreak\n", mid..mid);
        let n = d.paras().len();
        d.undo(mid..mid);
        assert_eq!(d.paras().len(), n - 2);
        d.redo(mid..mid);
        assert_eq!(d.paras().len(), n);
        assert!(d.text().contains("line\nbreak\n"));
        // A step taken as text alone, undone once styled again.
        d.edit(0..0, "a\nb\n", 0..0);
        d.keep_edits(false);
        d.undo(0..0);
        assert_eq!(d.paras().len(), d.text().split('\n').count());
        d.redo(0..0);
        assert_eq!(d.paras().len(), d.text().split('\n').count());
    }
}
