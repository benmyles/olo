//! Markdown documents, edited as they look (Typora's way) and as their
//! source. The text in the editor is the file's own Markdown, so what's
//! saved is what was written (its list markers, its emphasis characters,
//! its line breaks, front matter, HTML) and the place in it is the same
//! in both views; only how it shows differs.
//!
//! As it looks: each line set as its block is (a heading's size, a list
//! item's bullet or number hanging in its indent, a quotation's bar, a
//! code block's shaded ground, a rule across for `---`), inline styles
//! shown (emphasis, strong, struck, `code`, links, pictures), and the
//! marks that make them hidden, except on the lines the selection is on,
//! where they show (dimmed) as typed. As source: the same text in the
//! monospaced face, coloured by what each part is, with line numbers.
//! Code blocks are coloured by their fence's language in both
//! (`olo_ui::code`).
//!
//! The document is parsed again (pulldown-cmark, with each event's place
//! in the source) when its text changes, into each line's block and the
//! stretches of it that are content, styled, or marks; spans and
//! paragraph settings are made from that as lines show.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::Range,
    path::Path,
};

use olo_ui::{
    code::{self, Code},
    editor::{self, Span},
    gpui::{Font, FontStyle, FontWeight, Hsla},
    style,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A Markdown file, by its name.
pub fn is_markdown(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdwn" | "mdtxt" | "mdtext" | "mdx" | "ronn"))
}

/// The size body text is set at, as it looks.
pub const BODY: f32 = 12.0;
/// Source and code blocks' size.
pub const CODE: f32 = 11.5;
/// Each list or quotation level's indent, in points.
const INDENT: f32 = 22.0;
/// The largest code block coloured (bigger ones show plain: colouring
/// one is a parse on the spot).
const MOST_CODE: usize = 64 * 1024;

/// What a line is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Block {
    #[default]
    Blank,
    Text,
    /// An ATX heading (`## Title`), or the title of a Setext one.
    Heading(u8),
    /// A Setext heading's `===` or `---` under it.
    Underline,
    /// A code block's fence (` ``` `).
    Fence,
    Code,
    Html,
    Rule,
    Table {
        header: bool,
    },
    /// A table's `|---|---|` under its header.
    Delimiter,
    /// YAML front matter (`---` … `---` at the top).
    Front,
    /// Something not in any block (a link's definition): as typed.
    Other,
}

const EMPH: u8 = 1;
const STRONG: u8 = 2;
const STRIKE: u8 = 4;
const INLINE_CODE: u8 = 8;
const LINK: u8 = 16;
const HTML: u8 = 32;
const IMAGE: u8 = 64;
/// Not content: a mark (`#`, `*`, `>`, `](url)`).
const MARK: u8 = 128;

#[derive(Clone, Debug, Default)]
struct Line {
    start: usize,
    len: usize,
    block: Block,
    /// The lists and quotations it's in.
    lists: u8,
    quotes: u8,
    /// It starts a list item: what its marker shows (`•`, `1.`, `☐`).
    item: Option<String>,
    /// Edited since parsed (`Look::shift`): its newlines not yet found.
    dirty: bool,
    /// A table's row: its table (`Look::tables`) and each cell's bytes
    /// (start and length from the line's start, between its pipes).
    table: Option<usize>,
    cells: Vec<(usize, usize)>,
    /// Each byte's styles (`EMPH`…) and whether it's a mark.
    flags: Vec<u8>,
    /// The code block it's in (`Look::blocks`).
    code: Option<usize>,
    /// Pictures that start on it: the bytes of each one's `![alt](file)`
    /// (from the line's start) and its file.
    images: Vec<(Range<usize>, String)>,
}

/// A fenced or indented code block: its language and its text.
struct CodeBlock {
    lang: Option<&'static code::Language>,
    /// Its content's bytes in the document.
    range: Range<usize>,
    /// Its own text and syntax, made when it first shows (kept across
    /// parses while it's unchanged).
    code: Option<(String, Code)>,
}

/// Over this many bytes, a document is parsed on a worker (a parse is
/// about 9 ms a megabyte); smaller, at once.
const AT_ONCE: usize = 256 * 1024;

/// A Markdown document's look: what the parse found, and how it's shown.
#[derive(Default)]
pub struct Look {
    /// The document generation parsed (0: never).
    generation: u64,
    /// Parsed on a worker (`parse`): one is out, and the edits since the
    /// text it was given (each one's bytes taken away and how many put in).
    out: bool,
    since: Vec<(Range<usize>, usize)>,
    /// Its lines follow edits as they're made (too big to parse at once).
    big: bool,
    /// Some line is dirty (edited, not yet split at its newlines).
    dirty: bool,
    /// When it was last edited, and whether a parse waits for the typing
    /// to pause (`code::PAUSE`).
    touched: Option<std::time::Instant>,
    waiting: bool,
    lines: Vec<Line>,
    blocks: Vec<CodeBlock>,
    /// Each table's columns' shares of the width (by their longest cell).
    tables: Vec<Vec<f32>>,
    /// Code blocks' syntax by language and text, from the last parse (a
    /// block typed in is coloured again, the rest kept).
    kept: HashMap<(&'static str, u64), (String, Code)>,
    /// The bytes the selection touches: their lines show their marks.
    reveal: Range<usize>,
    /// Shown as source (else as it looks).
    pub source: bool,
}

impl Look {
    pub fn new(source: bool) -> Look {
        Look { source, ..Look::default() }
    }

    /// The selection is `sel`: whether a line's marks show changed (the
    /// editor lays those lines out again by itself).
    pub fn set_reveal(&mut self, sel: Range<usize>) -> bool {
        let changed = self.reveal != sel;
        self.reveal = sel;
        changed
    }

    /// Up to date with `text` (at `generation`): parsed again if it
    /// changed since and is small; a big one's edited lines found (its
    /// parse runs on a worker: [`parse`]).
    pub fn sync(&mut self, text: &str, generation: u64) {
        self.big = text.len() > AT_ONCE;
        if self.generation == generation {
            return;
        }
        if !self.big && !self.out {
            self.parse(text);
            self.generation = generation;
        } else {
            self.split_dirty(text);
        }
    }

    /// Bytes `removed` became `inserted` bytes (each change, in order): a
    /// big document's lines move with it until it's parsed again.
    pub fn edited(&mut self, removed: Range<usize>, inserted: usize) {
        self.touched = Some(std::time::Instant::now());
        if self.out {
            self.since.push((removed.clone(), inserted));
        }
        if self.big && !self.lines.is_empty() {
            self.shift(removed, inserted);
        }
    }

    /// The lines bytes `r` were on become one (dirty: split at its
    /// newlines by `split_dirty`), `n` bytes put in for `r`, styled as
    /// the byte before them; the lines after move along.
    fn shift(&mut self, r: Range<usize>, n: usize) {
        let delta = n as isize - r.len() as isize;
        let (a, b) = (self.line_at(r.start), self.line_at(r.end));
        let (first, last) = (&self.lines[a], &self.lines[b]);
        let start = first.start;
        let cut = (r.start - start).min(first.flags.len());
        let mut flags = Vec::with_capacity(first.len + n);
        flags.extend_from_slice(&first.flags[..cut]);
        let typed = flags.last().copied().unwrap_or(0) & !MARK;
        flags.extend(std::iter::repeat_n(typed, n));
        flags.extend_from_slice(&last.flags[(r.end.saturating_sub(last.start)).min(last.flags.len())..]);
        let line = Line { start, len: flags.len(), flags, dirty: true, images: Vec::new(), table: None, cells: Vec::new(), ..first.clone() };
        self.lines.splice(a..=b, [line]);
        self.dirty = true;
        for l in &mut self.lines[a + 1..] {
            l.start = (l.start as isize + delta) as usize;
        }
        for blk in &mut self.blocks {
            let at = |x: usize| if x >= r.end { (x as isize + delta) as usize } else { x.min(r.start) };
            if blk.range.end > r.start && blk.range.start < r.end.max(r.start + 1) {
                blk.code = None;
            }
            blk.range = at(blk.range.start)..at(blk.range.end).max(at(blk.range.start));
        }
    }

    /// Lines edited since parsed split at their newlines in `text`.
    fn split_dirty(&mut self, text: &str) {
        // (Asked for each line drawn: nothing to look through, mostly.)
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        // (Each in its place, from the last: the lines around stay put.)
        let dirty: Vec<usize> = self.lines.iter().enumerate().filter(|(_, l)| l.dirty).map(|(i, _)| i).collect();
        for i in dirty.into_iter().rev() {
            let l = &mut self.lines[i];
            l.dirty = false;
            if l.start + l.len > text.len() {
                continue;
            }
            let seg = &text[l.start..l.start + l.len];
            if !seg.contains('\n') {
                continue;
            }
            let l = l.clone();
            let mut at = l.start;
            let parts: Vec<Line> = seg
                .split('\n')
                .enumerate()
                .map(|(k, part)| {
                    let off = at - l.start;
                    let flags = l.flags.get(off..off + part.len()).map_or_else(|| vec![0; part.len()], <[u8]>::to_vec);
                    let line = Line { start: at, len: part.len(), flags, dirty: false, item: if k == 0 { l.item.clone() } else { None }, images: Vec::new(), ..l.clone() };
                    at += part.len() + 1;
                    line
                })
                .collect();
            self.lines.splice(i..=i, parts);
        }
    }

    /// The parse a big document wants (its text changed since and none is
    /// out), for [`parse`].
    fn job(&mut self, text: &str, generation: u64) -> Option<Job> {
        if self.out || self.generation == generation || text.len() <= AT_ONCE {
            return None;
        }
        self.out = true;
        self.since.clear();
        Some(Job { text: text.to_owned(), generation })
    }

    /// A worker's parse taken, the edits made meanwhile applied to it.
    /// The bytes whose lines it sets otherwise than before (None: none),
    /// and the lines it replaced (to let go of on a worker: a big
    /// document's are thousands of allocations).
    fn done(&mut self, parsed: Parsed, text: &str) -> (Option<Range<usize>>, Vec<Line>) {
        self.out = false;
        for b in self.blocks.drain(..) {
            if let (Some(lang), Some((t, c))) = (b.lang, b.code) {
                self.kept.insert((lang.id, hash(&t)), (t, c));
            }
        }
        let old = std::mem::replace(&mut self.lines, parsed.lines);
        self.blocks = parsed.blocks;
        self.tables = parsed.tables;
        self.generation = parsed.generation;
        self.dirty = false;
        for (r, n) in std::mem::take(&mut self.since) {
            self.shift(r, n);
        }
        self.split_dirty(text);
        // What differs: from the first line that does to the last.
        let new = &self.lines;
        let same = |a: &Line, b: &Line| a.start == b.start && a.len == b.len && a.block == b.block && a.lists == b.lists && a.quotes == b.quotes && a.item == b.item && a.flags == b.flags;
        let head = old.iter().zip(new).take_while(|(a, b)| same(a, b)).count();
        if head == old.len() && head == new.len() {
            return (None, old);
        }
        let tail = old.iter().rev().zip(new.iter().rev()).take(old.len().min(new.len()) - head).take_while(|(a, b)| same(a, b)).count();
        let first = new.get(head).map_or(text.len(), |l| l.start);
        let last = new.len().checked_sub(tail + 1).and_then(|i| new.get(i)).map_or(text.len(), |l| l.start + l.len);
        (Some(first.min(last)..last.max(first)), old)
    }

    fn line_at(&self, at: usize) -> usize {
        self.lines.partition_point(|l| l.start <= at).saturating_sub(1)
    }

    /// The lines bytes `r` cover (an empty `r`: its line).
    fn lines_of(&self, r: &Range<usize>) -> Range<usize> {
        let a = self.line_at(r.start);
        let b = self.line_at(r.end.saturating_sub(1).max(r.start));
        a..b + 1
    }

    /// `f` given each line `r` covers, and `r`'s bytes on it (from its start).
    fn each(&mut self, r: &Range<usize>, mut f: impl FnMut(&mut Line, Range<usize>)) {
        for i in self.lines_of(r) {
            let l = &mut self.lines[i];
            let (a, b) = (r.start.max(l.start), r.end.min(l.start + l.len));
            f(l, a.saturating_sub(l.start)..b.saturating_sub(l.start).max(a.saturating_sub(l.start)));
        }
    }

    fn parse(&mut self, text: &str) {
        // Each line, every byte a mark until found to be content.
        let mut lines = Vec::new();
        let mut at = 0;
        for l in text.split('\n') {
            lines.push(Line { start: at, len: l.len(), flags: vec![MARK; l.len()], ..Line::default() });
            at += l.len() + 1;
        }
        self.lines = lines;
        self.tables.clear();
        // (Blocks made again; the syntax of those shown found in `kept`
        // when they show again, the rest let go.)
        let mut kept = HashMap::new();
        for b in self.blocks.drain(..) {
            if let (Some(lang), Some((t, c))) = (b.lang, b.code) {
                kept.insert((lang.id, hash(&t)), (t, c));
            }
        }
        self.kept = kept;

        let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS | Options::ENABLE_GFM;
        // Lists open: each one's next number (None: bulleted).
        let mut lists: Vec<Option<u64>> = Vec::new();
        let mut code_block: Option<usize> = None;
        for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
            match ev {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => self.each(&r, |l, _| {
                        if l.block == Block::Blank {
                            l.block = Block::Text;
                        }
                    }),
                    Tag::Heading { level, .. } => {
                        let n = heading(level);
                        let atx = text[r.clone()].trim_start().starts_with('#');
                        let ls = self.lines_of(&r);
                        for i in ls.clone() {
                            self.lines[i].block = if !atx && i + 1 == ls.end && ls.len() > 1 { Block::Underline } else { Block::Heading(n) };
                        }
                    }
                    Tag::BlockQuote(_) => self.each(&r, |l, _| l.quotes = l.quotes.saturating_add(1)),
                    Tag::List(start) => lists.push(start),
                    Tag::Item => {
                        let depth = lists.len();
                        let marker = match lists.last_mut() {
                            Some(Some(n)) => {
                                let m = format!("{n}{}", text[r.clone()].trim_start().chars().find(|c| *c == '.' || *c == ')').unwrap_or('.'));
                                *n += 1;
                                m
                            }
                            _ => ["•", "◦", "▪"][(depth.max(1) - 1) % 3].to_owned(),
                        };
                        let first = self.line_at(r.start);
                        self.lines[first].item = Some(marker);
                        // (A tight list's items have no paragraph of their own.)
                        self.each(&r, |l, _| {
                            l.lists = l.lists.saturating_add(1);
                            if l.block == Block::Blank && l.len > 0 {
                                l.block = Block::Text;
                            }
                        });
                    }
                    Tag::CodeBlock(kind) => {
                        let ls = self.lines_of(&r);
                        let lang = match &kind {
                            CodeBlockKind::Fenced(info) => code::by_name(info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("")),
                            CodeBlockKind::Indented => None,
                        };
                        let fenced = matches!(kind, CodeBlockKind::Fenced(_));
                        let k = self.blocks.len();
                        // (Its content's bytes: its text's, as found.)
                        self.blocks.push(CodeBlock { lang, range: usize::MAX..0, code: None });
                        code_block = Some(k);
                        for i in ls.clone() {
                            let l = &mut self.lines[i];
                            let line = &text[l.start..l.start + l.len];
                            let fence = fenced && (i == ls.start || (i + 1 == ls.end && is_fence(line)));
                            l.block = if fence { Block::Fence } else { Block::Code };
                            l.code = Some(k);
                            // The opening fence's language shows (a label).
                            if fence && i == ls.start {
                                let t = line.trim_start();
                                let info = t.trim_start_matches(['`', '~']);
                                let at = line.len() - info.len();
                                let word = info.trim().len();
                                let lead = info.len() - info.trim_start().len();
                                for f in &mut l.flags[at + lead..at + lead + word] {
                                    *f &= !MARK;
                                }
                            }
                        }
                    }
                    Tag::HtmlBlock => self.each(&r, |l, _| l.block = Block::Html),
                    Tag::Table(_) => {
                        let ls = self.lines_of(&r);
                        let k = self.tables.len();
                        let mut widths: Vec<usize> = Vec::new();
                        for i in ls.clone() {
                            let l = &mut self.lines[i];
                            l.block = if i == ls.start + 1 { Block::Delimiter } else { Block::Table { header: false } };
                            if l.block != Block::Delimiter {
                                l.table = Some(k);
                                l.cells = row_cells(&text[l.start..l.start + l.len]);
                                for (c, (_, n)) in l.cells.iter().enumerate() {
                                    let chars = text[l.start..].get(l.cells[c].0..l.cells[c].0 + n).map_or(0, |t| t.chars().count());
                                    if widths.len() <= c {
                                        widths.push(0);
                                    }
                                    widths[c] = widths[c].max(chars);
                                }
                            }
                        }
                        self.tables.push(widths.into_iter().map(|w| w.clamp(4, 60) as f32).collect());
                    }
                    Tag::TableHead => self.each(&r, |l, _| l.block = Block::Table { header: true }),
                    Tag::MetadataBlock(_) => self.each(&r, |l, all| {
                        l.block = Block::Front;
                        for f in &mut l.flags[all] {
                            *f &= !MARK;
                        }
                    }),
                    Tag::Emphasis => self.style(&r, EMPH),
                    Tag::Strong => self.style(&r, STRONG),
                    Tag::Strikethrough => self.style(&r, STRIKE),
                    Tag::Link { .. } => self.style(&r, LINK),
                    Tag::Image { dest_url, .. } => {
                        self.style(&r, IMAGE);
                        let i = self.line_at(r.start);
                        let l = &mut self.lines[i];
                        let end = r.end.min(l.start + l.len);
                        l.images.push((r.start - l.start..end - l.start, dest_url.to_string()));
                    }
                    Tag::FootnoteDefinition(_) => self.each(&r, |l, _| {
                        if l.block == Block::Blank {
                            l.block = Block::Text;
                        }
                    }),
                    _ => {}
                },
                Event::End(TagEnd::List(_)) => {
                    lists.pop();
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some(k) = code_block.take()
                        && self.blocks[k].range.start == usize::MAX
                    {
                        self.blocks[k].range = r.end..r.end;
                    }
                }
                Event::Text(_) => {
                    if let Some(k) = code_block {
                        let b = &mut self.blocks[k];
                        b.range = b.range.start.min(r.start)..b.range.end.max(r.end);
                    }
                    self.content(&r, 0);
                }
                Event::Code(_) => {
                    // `code`: the text between its backticks (and the one
                    // space either side, if both).
                    let s = &text[r.clone()];
                    let ticks = s.len() - s.trim_start_matches('`').len();
                    let mut inner = r.start + ticks..r.end.saturating_sub(ticks).max(r.start + ticks);
                    let t = &text[inner.clone()];
                    if t.len() >= 2 && t.starts_with(' ') && t.ends_with(' ') && !t.trim().is_empty() {
                        inner = inner.start + 1..inner.end - 1;
                    }
                    self.content(&inner, INLINE_CODE);
                }
                Event::Html(_) | Event::InlineHtml(_) => self.content(&r, HTML),
                Event::InlineMath(_) | Event::DisplayMath(_) | Event::FootnoteReference(_) => self.content(&r, 0),
                Event::Rule => self.each(&r, |l, _| l.block = Block::Rule),
                Event::TaskListMarker(done) => {
                    let i = self.line_at(r.start);
                    self.lines[i].item = Some(if done { "☑" } else { "☐" }.to_owned());
                }
                _ => {}
            }
        }
        // Lines in no block but with something on them: as typed.
        for l in &mut self.lines {
            if l.block == Block::Blank && !text[l.start..l.start + l.len].trim().is_empty() {
                l.block = Block::Other;
                l.flags.fill(0);
            }
        }
    }

    /// Bytes `r` are content, styled `flags` too.
    fn content(&mut self, r: &Range<usize>, flags: u8) {
        self.each(r, |l, on| {
            for f in &mut l.flags[on] {
                *f = (*f & !MARK) | flags;
            }
        });
    }

    /// Bytes `r` (content and marks) are styled `flags`.
    fn style(&mut self, r: &Range<usize>, flags: u8) {
        self.each(r, |l, on| {
            for f in &mut l.flags[on] {
                *f |= flags;
            }
        });
    }

    /// Whether line `i`'s marks show (it's being edited, or it's source).
    fn revealed(&self, i: usize) -> bool {
        let l = &self.lines[i];
        self.source || (self.reveal.start <= l.start + l.len && self.reveal.end >= l.start)
    }

    /// How the paragraph at bytes `range` is set.
    pub fn paragraph(&mut self, range: Range<usize>, p: &olo_theme::Palette, pictures: &mut dyn FnMut(&str) -> Option<editor::Picture>) -> editor::Paragraph {
        let mut f = editor::Paragraph::default();
        if self.lines.is_empty() || self.source {
            return f;
        }
        let i = self.line_at(range.start);
        let shown = self.revealed(i);
        let l = &self.lines[i];
        let levels = (l.lists + l.quotes) as f32;
        f.left = levels * INDENT;
        if l.quotes > 0 {
            f.bar = Some(style::c(olo_theme::mix(p.document_text, p.document_background, 0.7)));
        }
        match l.block {
            Block::Heading(n) => {
                f.above = match n {
                    1 => 10.0,
                    2 => 8.0,
                    _ => 4.0,
                };
                f.below = if n <= 2 { 2.0 } else { 0.0 };
            }
            Block::Fence | Block::Code | Block::Front => {
                f.verbatim = true;
                f.shade = Some(code_ground(p));
                f.left = levels * INDENT + 8.0;
                f.right = 8.0;
                // A fence with nothing showing on it: a little room at the
                // block's end (or start).
                if l.block == Block::Fence && !shown && l.flags.iter().all(|f| f & MARK != 0) {
                    f.above = 3.0;
                    f.below = 3.0;
                }
            }
            Block::Table { header } => {
                f.verbatim = true;
                if let Some(columns) = l.table.and_then(|k| self.tables.get(k))
                    && !l.cells.is_empty()
                {
                    f.row = Some(editor::Row { columns: columns.clone(), header, cells: Some(l.cells.clone()) });
                }
            }
            Block::Html | Block::Other | Block::Delimiter => f.verbatim = true,
            Block::Rule if !shown => {
                f.rule = Some(style::c(olo_theme::mix(p.document_text, p.document_background, 0.75)));
                f.above = 6.0;
                f.below = 6.0;
            }
            _ => {}
        }
        if let Some(marker) = &l.item
            && !shown
        {
            f.marker = Some(marker.clone());
        } else if l.item.is_some() {
            // Its marker shows as typed, where the bullet hangs.
            f.left = (f.left - INDENT).max(0.0);
        }
        if !shown {
            for (r, src) in &l.images {
                if let Some(pic) = pictures(src) {
                    f.pictures.push((r.start, pic));
                }
            }
        }
        f
    }

    /// The spans of the paragraph at bytes `range` of `text`, in `p`'s
    /// colours; `picture` gives a picture's shown size, if it's loaded.
    pub fn spans(&mut self, text: &str, range: Range<usize>, p: &olo_theme::Palette, picture: &mut dyn FnMut(&str) -> Option<(f32, f32)>, out: &mut Vec<Span>) {
        if self.lines.is_empty() {
            out.push(Span::new(range.len(), style::ui_font(FontWeight::NORMAL), BODY));
            return;
        }
        let i = self.line_at(range.start);
        let shown = self.revealed(i);
        let source = self.source;
        let block = self.lines[i].block;
        // A code block's line: coloured by its language.
        if let Some(k) = self.lines[i].code
            && block == Block::Code
        {
            let size = CODE;
            if self.code_spans(text, k, range.clone(), p, out) {
                return;
            }
            out.push(Span::new(range.len(), style::mono_font(FontWeight::NORMAL), size));
            return;
        }
        let l = &self.lines[i];
        let dim = style::c(p.syntax(code::Class::Comment));
        let size = if source {
            CODE
        } else {
            match block {
                Block::Heading(1) => BODY * 2.0,
                Block::Heading(2) => BODY * 1.6,
                Block::Heading(3) => BODY * 1.3,
                Block::Heading(4) => BODY * 1.1,
                Block::Heading(_) => BODY,
                Block::Fence | Block::Front => CODE * 0.9,
                // (Blank lines between blocks: a little less than a line.)
                Block::Blank => BODY * 0.7,
                _ => BODY,
            }
        };
        // A table's row as a grid (its cells asked for one at a time).
        let grid = !source && l.table.is_some() && !l.cells.is_empty();
        let mono_block = source || matches!(block, Block::Fence | Block::Html | Block::Front | Block::Other | Block::Delimiter) || (matches!(block, Block::Table { .. }) && !grid);
        let off = range.start.saturating_sub(l.start).min(l.flags.len());
        if range.is_empty() || l.flags.len() == off {
            let font = if mono_block { style::mono_font(FontWeight::NORMAL) } else { style::ui_font(FontWeight::NORMAL) };
            out.push(Span::new(0, font, size));
            return;
        }
        let heading = matches!(block, Block::Heading(_));
        let mut k = off;
        let n = (off + range.len()).min(l.flags.len());
        while k < n {
            let fl = l.flags[k];
            let mut j = k + 1;
            while j < n && l.flags[j] == fl {
                j += 1;
            }
            let mark = fl & MARK != 0;
            let bold = fl & STRONG != 0 || heading || matches!(block, Block::Table { header: true });
            let italic = fl & EMPH != 0;
            let mono = mono_block || fl & (INLINE_CODE | HTML) != 0;
            let weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
            let mut font: Font = if mono { style::mono_font(weight) } else { style::ui_font(weight) };
            if italic {
                font.style = FontStyle::Italic;
            }
            let mut s = Span::new(j - k, font, if mono && !source && !mono_block { size * 0.92 } else { size });
            s.strike = fl & STRIKE != 0 && !mark;
            if source {
                s.color = Some(style::c(p.syntax(match () {
                    _ if mark => code::Class::Comment,
                    _ if heading => code::Class::Heading,
                    _ if fl & (LINK | IMAGE) != 0 => code::Class::Link,
                    _ if fl & INLINE_CODE != 0 => code::Class::String,
                    _ if fl & HTML != 0 => code::Class::Tag,
                    _ if matches!(block, Block::Fence | Block::Front) => code::Class::Comment,
                    _ => code::Class::Plain,
                })));
                if s.color == Some(style::c(p.document_text)) {
                    s.color = None;
                }
            } else {
                if fl & (LINK | IMAGE) != 0 && !mark {
                    s.color = Some(style::c(p.syntax(code::Class::Link)));
                    s.underline = fl & LINK != 0;
                }
                if fl & HTML != 0 || matches!(block, Block::Html | Block::Other | Block::Front | Block::Fence) {
                    s.color = Some(dim);
                }
                if l.quotes > 0 && s.color.is_none() {
                    s.color = Some(style::c(olo_theme::mix(p.document_text, p.document_background, 0.3)));
                }
                if fl & INLINE_CODE != 0 && !mark {
                    s.background = Some(code_ground(p));
                }
                if mark {
                    if shown || (matches!(block, Block::Table { .. }) && !grid) {
                        s.color = Some(dim);
                        s.underline = false;
                    } else {
                        s.hidden = true;
                    }
                }
            }
            out.push(s);
            k = j;
        }
        if n - off < range.len() {
            out.push(Span::new(range.len() - (n - off), style::ui_font(FontWeight::NORMAL), size));
        }
        // Pictures, as they look: each one's `![alt](file)` its picture.
        if !shown && !source && off == 0 {
            let images = self.lines[i].images.clone();
            for (r, src) in images {
                if let Some(size) = picture(&src) {
                    replace_with_picture(out, r, size);
                }
            }
        }
    }

    /// What Return types on a list item or in a quotation (its marks
    /// again on the next line: a list's next number, a task unchecked);
    /// in an empty one, its marks go (the list or quotation ends there).
    pub fn newline(&self, text: &str, sel: Range<usize>) -> Option<(Range<usize>, String, usize)> {
        if self.lines.is_empty() {
            return None;
        }
        let l = &self.lines[self.line_at(sel.start)];
        // A table's row: an empty one after it, the caret in its first cell.
        if matches!(l.block, Block::Table { .. }) && !l.cells.is_empty() && !self.source && sel.end <= l.start + l.len {
            let end = l.start + l.len;
            return Some((end..end, format!("\n|{}", "  |".repeat(l.cells.len())), 3));
        }
        if (l.item.is_none() && l.quotes == 0) || matches!(l.block, Block::Code | Block::Fence) || sel.end > l.start + l.len {
            return None;
        }
        // Its marks before its content (a list's, a quotation's).
        let lead = l.flags.iter().position(|f| f & MARK == 0).unwrap_or(l.len);
        if sel.start < l.start + lead {
            return None;
        }
        let line = &text[l.start..l.start + l.len];
        if lead == l.len {
            return Some((l.start..l.start + l.len, String::new(), 0));
        }
        let next = format!("\n{}", next_marker(&line[..lead]));
        let n = next.len();
        Some((sel, next, n))
    }

    /// Code block `k`'s spans for its line at `range`: false if it isn't
    /// coloured (no language, too big).
    fn code_spans(&mut self, text: &str, k: usize, range: Range<usize>, p: &olo_theme::Palette, out: &mut Vec<Span>) -> bool {
        let b = &mut self.blocks[k];
        let Some(lang) = b.lang else { return false };
        if b.range.len() > MOST_CODE || range.start < b.range.start || range.end > b.range.end {
            return false;
        }
        if b.code.is_none() {
            let own = text[b.range.clone()].to_owned();
            let made = self.kept.remove(&(lang.id, hash(&own))).unwrap_or_else(|| {
                // Small: parsed on the spot.
                let mut c = Code::new(lang, CODE);
                if let Some(job) = c.job(&own) {
                    c.done(job.run());
                }
                (own, c)
            });
            b.code = Some(made);
        }
        let Some((own, c)) = b.code.as_mut() else { return false };
        let at = range.start - b.range.start;
        c.spans(own, at..at + range.len(), p, out);
        true
    }
}

/// A big document's parse, to run on a worker.
struct Job {
    text: String,
    generation: u64,
}

/// What a worker's parse found.
struct Parsed {
    lines: Vec<Line>,
    blocks: Vec<CodeBlock>,
    tables: Vec<Vec<f32>>,
    generation: u64,
}

impl Job {
    fn run(self) -> Parsed {
        let mut l = Look::default();
        l.parse(&self.text);
        Parsed { lines: l.lines, blocks: l.blocks, tables: l.tables, generation: self.generation }
    }
}

/// Runs the parse a big document wants (if any) on a worker; when it's
/// back, `find` finds the look, editor, text and generation in the view
/// again (None: gone), the parse is taken (its lines measured again) and
/// the view drawn, and if the text changed meanwhile it's parsed again.
#[allow(clippy::type_complexity)]
pub fn parse<V: 'static>(cx: &mut olo_ui::gpui::Context<V>, look: &mut Look, text: &str, generation: u64, find: fn(&mut V) -> Option<(&mut Look, &mut editor::Editor, &str, u64)>) {
    // Being typed in: once the typing pauses.
    if text.len() > AT_ONCE
        && let Some(t) = look.touched
        && t.elapsed() < code::PAUSE
    {
        if !look.waiting {
            look.waiting = true;
            let wait = code::PAUSE - t.elapsed();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |v, cx| {
                    let Some((look, _, text, generation)) = find(v) else { return };
                    look.waiting = false;
                    parse(cx, look, text, generation, find);
                });
            })
            .detach();
        }
        return;
    }
    let Some(job) = look.job(text, generation) else { return };
    let task = cx.background_executor().spawn(async move { job.run() });
    cx.spawn(async move |this, cx| {
        let parsed = task.await;
        let _ = this.update(cx, |v, cx| {
            let Some((look, editor, text, generation)) = find(v) else { return };
            let (changed, old) = look.done(parsed, text);
            cx.background_executor().spawn(async move { drop(old) }).detach();
            if let Some(r) = changed {
                editor.restyled(r);
                cx.notify();
            }
            parse(cx, look, text, generation, find);
        });
    })
    .detach();
}

/// What a code block, front matter and inline code are set on.
pub fn code_ground(p: &olo_theme::Palette) -> Hsla {
    style::c(olo_theme::mix(p.document_background, p.document_text, if p.is_dark() { 0.08 } else { 0.05 }))
}

fn heading(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// A table row's cells: the bytes between its pipes (not one escaped or
/// in code), their spaces trimmed, as (start, length); the pipes at
/// either end leave no cell.
fn row_cells(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut pipes = Vec::new();
    let (mut i, mut code) = (0, false);
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'`' => code = !code,
            b'|' if !code => pipes.push(i),
            _ => {}
        }
        i += 1;
    }
    let mut segs = Vec::with_capacity(pipes.len() + 1);
    let mut from = 0;
    for p in pipes {
        segs.push((from, p));
        from = p + 1;
    }
    segs.push((from, b.len()));
    let blank = |&(a, e): &(usize, usize)| line[a..e].trim().is_empty();
    if segs.len() > 1 && segs.first().is_some_and(blank) {
        segs.remove(0);
    }
    if segs.len() > 1 && segs.last().is_some_and(blank) {
        segs.pop();
    }
    segs.into_iter()
        .map(|(a, e)| {
            let t = &line[a..e];
            let lead = t.len() - t.trim_start().len();
            let len = t.trim().len();
            if len == 0 { ((a + 1).min(e), 0) } else { (a + lead, len) }
        })
        .collect()
}

/// A code fence (` ``` ` or `~~~`, after up to three spaces).
fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    line.len() - t.len() <= 3 && (t.starts_with("```") || t.starts_with("~~~"))
}

/// What a line becomes (the Paragraph menu, as Markdown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Make {
    Body,
    Heading(u8),
    Quote,
    Bullets,
    Numbers,
    Tasks,
}

/// A line's own marks at its start: its indent, then a heading's `#`s, a
/// quotation's `>`s, a list item's marker and a task's box (each with the
/// space after), and the rest.
fn split_marks(line: &str) -> (&str, &str, &str) {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut rest = &line[indent..];
    let from = rest;
    loop {
        let t = rest;
        if let Some(r) = t.strip_prefix('>') {
            rest = r.strip_prefix(' ').unwrap_or(r);
        } else if t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') && t.len() - t.trim_start_matches('#').len() <= 6 {
            rest = &t.trim_start_matches('#')[1..];
        } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
            rest = r;
        } else if let Some(d) = t.find(|c: char| !c.is_ascii_digit())
            && d > 0
            && (t[d..].starts_with(". ") || t[d..].starts_with(") "))
        {
            rest = &t[d + 2..];
        } else if let Some(r) = t.strip_prefix("[ ] ").or_else(|| t.strip_prefix("[x] ")).or_else(|| t.strip_prefix("[X] ")) {
            rest = r;
        } else {
            break;
        }
    }
    (&line[..indent], &from[..from.len() - rest.len()], rest)
}

/// Lines `text` (whole lines, newlines between) made `make`: their own
/// marks replaced (their indent kept), each numbered on from 1 for a
/// numbered list. All of them already so: back to body text.
pub fn make_lines(text: &str, make: Make) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mark = |k: usize| match make {
        Make::Body => String::new(),
        Make::Heading(n) => format!("{} ", "#".repeat(n.clamp(1, 6) as usize)),
        Make::Quote => "> ".into(),
        Make::Bullets => "- ".into(),
        Make::Numbers => format!("{}. ", k + 1),
        Make::Tasks => "- [ ] ".into(),
    };
    let already = make != Make::Body
        && lines.iter().filter(|l| !l.trim().is_empty()).enumerate().all(|(k, l)| {
            let (_, m, _) = split_marks(l);
            match make {
                Make::Numbers => m.trim_end().ends_with('.') || m.trim_end().ends_with(')'),
                _ => m == mark(k),
            }
        });
    let mut k = 0;
    let out: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                return (*l).to_owned();
            }
            let (indent, _, rest) = split_marks(l);
            let m = if already { String::new() } else { mark(k) };
            k += 1;
            format!("{indent}{m}{rest}")
        })
        .collect();
    out.join("\n")
}

/// A list item's marks for the item after it: its number one more, a
/// task not yet done.
fn next_marker(marks: &str) -> String {
    if let Some(a) = marks.find(|c: char| c.is_ascii_digit()) {
        let b = marks[a..].find(|c: char| !c.is_ascii_digit()).map_or(marks.len(), |e| a + e);
        if let Ok(n) = marks[a..b].parse::<u64>() {
            return format!("{}{}{}", &marks[..a], n + 1, &marks[b..]);
        }
    }
    marks.replace("[x]", "[ ]").replace("[X]", "[ ]")
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Bytes `r` of the paragraph `out` styles become one hidden span, its
/// first character a picture `size` points big.
fn replace_with_picture(out: &mut Vec<Span>, r: Range<usize>, size: (f32, f32)) {
    let mut spans: Vec<Span> = Vec::with_capacity(out.len() + 2);
    let mut at = 0;
    let first = out.first().cloned();
    for s in out.drain(..) {
        let (a, b) = (at, at + s.len);
        at = b;
        // Before, after, or around the picture.
        if b <= r.start || a >= r.end {
            spans.push(s);
            continue;
        }
        if a < r.start {
            spans.push(Span { len: r.start - a, ..s.clone() });
        }
        if a <= r.start {
            spans.push(Span { len: r.len(), hidden: true, picture: Some(size), color: None, underline: false, background: None, ..s.clone() });
        }
        if b > r.end {
            spans.push(Span { len: b - r.end, ..s });
        }
    }
    if spans.is_empty()
        && let Some(f) = first
    {
        spans.push(f);
    }
    *out = spans;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(text: &str) -> Look {
        let mut l = Look::new(false);
        l.sync(text, 1);
        l
    }

    fn marks(l: &Look, i: usize, text: &str) -> String {
        let line = &l.lines[i];
        text[line.start..line.start + line.len].char_indices().map(|(k, c)| if line.flags[k] & MARK != 0 { '^' } else { c }).collect()
    }

    #[test]
    fn blocks_and_marks() {
        let text = "# Title\n\nSome *em* and **strong** and `code`.\n\n- one\n- [ ] two\n  more\n\n> quoted\n\n```rust\nfn a() {}\n```\n\n---\n[x]: http://x\n";
        let l = look(text);
        assert_eq!(l.lines[0].block, Block::Heading(1));
        assert_eq!(marks(&l, 0, text), "^^Title");
        assert_eq!(marks(&l, 2, text), "Some ^em^ and ^^strong^^ and ^code^.");
        assert_eq!(l.lines[4].item.as_deref(), Some("•"));
        assert_eq!(marks(&l, 4, text), "^^one");
        assert_eq!(l.lines[5].item.as_deref(), Some("☐"));
        assert_eq!(marks(&l, 5, text), "^^^^^^two");
        assert_eq!(l.lines[6].lists, 1);
        assert_eq!(marks(&l, 6, text), "^^more");
        assert_eq!(l.lines[8].quotes, 1);
        assert_eq!(marks(&l, 8, text), "^^quoted");
        assert_eq!(l.lines[10].block, Block::Fence);
        assert_eq!(l.lines[11].block, Block::Code);
        assert_eq!(l.lines[12].block, Block::Fence);
        assert_eq!(l.blocks[0].lang.map(|l| l.id), Some("rust"));
        assert_eq!(&text[l.blocks[0].range.clone()], "fn a() {}\n");
        assert_eq!(l.lines[14].block, Block::Rule);
        assert_eq!(l.lines[15].block, Block::Other);
    }

    #[test]
    fn table_rows_cells_between_pipes() {
        assert_eq!(row_cells("| a | bc |"), [(2, 1), (6, 2)]);
        assert_eq!(row_cells("a | b"), [(0, 1), (4, 1)]);
        assert_eq!(row_cells("|  | x |"), [(2, 0), (5, 1)]);
        assert_eq!(row_cells("| `a|b` | c\\|d |"), [(2, 5), (10, 4)]);
        let text = "| Name | Value |\n|------|------:|\n| a    | 1     |\n";
        let l = look(text);
        assert_eq!(l.lines[0].block, Block::Table { header: true });
        assert_eq!(l.lines[1].block, Block::Delimiter);
        assert_eq!(l.lines[2].cells, [(2, 1), (9, 1)]);
        assert_eq!(l.tables[0], [4.0, 5.0]);
    }

    #[test]
    fn numbered_lists_count() {
        let text = "3. a\n4. b\n5) c\n";
        let l = look(text);
        assert_eq!(l.lines[0].item.as_deref(), Some("3."));
        assert_eq!(l.lines[1].item.as_deref(), Some("4."));
    }

    #[test]
    fn setext_and_links() {
        let text = "Title\n=====\n\nSee [the site](http://x.org \"t\") now.\n";
        let l = look(text);
        assert_eq!(l.lines[0].block, Block::Heading(1));
        assert_eq!(l.lines[1].block, Block::Underline);
        assert_eq!(marks(&l, 3, text), "See ^the site^^^^^^^^^^^^^^^^^^^ now.");
        assert!(l.lines[3].flags[5] & LINK != 0);
    }

    #[test]
    fn marks_hide_off_the_caret_line_and_show_on_it() {
        let text = "a **b** c\n\nd *e*\n";
        let mut l = look(text);
        let p = olo_theme::Palette::light();
        let hidden = |l: &mut Look, at: usize| {
            let mut out = Vec::new();
            let i = l.line_at(at);
            let r = l.lines[i].start..l.lines[i].start + l.lines[i].len;
            l.spans(text, r, &p, &mut |_| None, &mut out);
            out.iter().filter(|s| s.hidden).map(|s| s.len).sum::<usize>()
        };
        l.set_reveal(0..0);
        assert_eq!(hidden(&mut l, 0), 0, "the caret's line shows its marks");
        assert_eq!(hidden(&mut l, 11), 2);
        l.set_reveal(12..12);
        assert_eq!(hidden(&mut l, 0), 4);
        assert_eq!(hidden(&mut l, 11), 0);
        // As source: nothing hidden.
        l.source = true;
        assert_eq!(hidden(&mut l, 0), 0);
    }

    #[test]
    fn spans_cover_each_line() {
        let text = "# H\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n![pic](x.png) after\n\n<div>\nhi\n</div>\n---\nfront: x\n---\n";
        let mut l = look(text);
        let p = olo_theme::Palette::light();
        for i in 0..l.lines.len() {
            let r = l.lines[i].start..l.lines[i].start + l.lines[i].len;
            let mut out = Vec::new();
            l.spans(text, r.clone(), &p, &mut |_| Some((40.0, 30.0)), &mut out);
            assert_eq!(out.iter().map(|s| s.len).sum::<usize>(), r.len(), "line {i}: {:?}", &text[r]);
        }
    }

    #[test]
    fn return_continues_lists() {
        let text = "- one\n3. two\n- [x] done\n> q\n- \npara\n";
        let l = look(text);
        let at = |s: &str| text.find(s).unwrap() + s.len();
        assert_eq!(l.newline(text, at("one")..at("one")), Some((at("one")..at("one"), "\n- ".into(), 3)));
        assert_eq!(l.newline(text, at("two")..at("two")).map(|t| t.1), Some("\n4. ".into()));
        assert_eq!(l.newline(text, at("done")..at("done")).map(|t| t.1), Some("\n- [ ] ".into()));
        assert_eq!(l.newline(text, at("q")..at("q")).map(|t| t.1), Some("\n> ".into()));
        // An empty item: its marker goes.
        let empty = text.find("- \n").unwrap();
        assert_eq!(l.newline(text, empty + 2..empty + 2), Some((empty..empty + 2, String::new(), 0)));
        assert_eq!(l.newline(text, at("para")..at("para")), None);
    }

    #[test]
    fn lines_made_headings_lists_and_back() {
        assert_eq!(make_lines("a\nb", Make::Bullets), "- a\n- b");
        assert_eq!(make_lines("- a\n- b", Make::Bullets), "a\nb");
        assert_eq!(make_lines("- a\n\n- b", Make::Numbers), "1. a\n\n2. b");
        assert_eq!(make_lines("## Title", Make::Heading(1)), "# Title");
        assert_eq!(make_lines("# Title", Make::Heading(1)), "Title");
        assert_eq!(make_lines("  - [x] done", Make::Body), "  done");
        assert_eq!(make_lines("text", Make::Quote), "> text");
        assert_eq!(make_lines("3. x", Make::Tasks), "- [ ] x");
    }

    #[test]
    fn big_documents_follow_edits_until_parsed() {
        let one = "# Head\n\nSome **bold** text.\n\n- item\n";
        let mut text = one.repeat(AT_ONCE / one.len() + 10);
        let mut l = Look::new(false);
        l.sync(&text, 1);
        assert!(l.lines.is_empty(), "big: not parsed at once");
        let job = l.job(&text, 1).unwrap();
        assert!(l.job(&text, 1).is_none(), "one at a time");
        // Typed meanwhile: a line and a word.
        text.insert_str(0, "new\n");
        l.edited(0..0, 4);
        let at = text.find("bold").unwrap();
        text.insert_str(at, "very ");
        l.edited(at..at, 5);
        assert!(l.done(job.run(), &text).0.is_some(), "first parse: all of it");
        l.sync(&text, 3);
        // Lines where the text has them, styles moved with their bytes.
        for (i, line) in text.split('\n').enumerate().take(20) {
            assert_eq!(l.lines[i].len, line.len(), "line {i}");
            assert_eq!(l.lines[i].flags.len(), line.len(), "line {i}");
        }
        assert_eq!(l.lines[1].block, Block::Heading(1));
        let i = l.line_at(at);
        let k = at - l.lines[i].start;
        assert!(l.lines[i].flags[k] & STRONG != 0, "typed in bold: bold");
        // The text changed since: parsed again, then all as parsed, and
        // only what the parse sets otherwise is laid out again.
        let job = l.job(&text, 3).expect("again");
        let changed = l.done(job.run(), &text).0.expect("the typed lines");
        assert!(changed.end < 200, "{changed:?}");
        assert!(l.job(&text, 3).is_none());
        assert_eq!(l.lines.len(), text.split('\n').count());
    }

    #[test]
    fn a_keystroke_in_a_big_document_is_quick() {
        let one = include_str!("../../../docs/HACKING.md");
        let mut text = one.repeat(3);
        let mut l = Look::new(false);
        l.sync(&text, 1);
        let job = l.job(&text, 1).unwrap();
        let _ = l.done(job.run(), &text);
        let p = olo_theme::Palette::light();
        let at = text.len() / 2;
        let i = l.line_at(at);
        let mut times = Vec::new();
        for k in 0..20 {
            let t = std::time::Instant::now();
            let pos = at + k;
            text.insert(pos, 'x');
            l.edited(pos..pos, 1);
            l.sync(&text, 2 + k as u64);
            let edit = t.elapsed();
            let t = std::time::Instant::now();
            let mut out = Vec::new();
            for j in i.saturating_sub(35)..i + 35 {
                let r = l.lines[j].start..l.lines[j].start + l.lines[j].len;
                out.clear();
                l.paragraph(r.clone(), &p, &mut |_| None);
                l.spans(&text, r, &p, &mut |_| None, &mut out);
            }
            times.push((edit, t.elapsed()));
        }
        eprintln!("edit+sync, 70 lines' spans: {:?}", &times[..4]);
    }

    #[test]
    fn big_documents_parse_quickly() {
        let one = include_str!("../../../docs/HACKING.md");
        let mut text = String::new();
        while text.len() < 4_000_000 {
            text.push_str(one);
        }
        // (On a worker, as a big one is.)
        let t = std::time::Instant::now();
        let p = Job { text: text.clone(), generation: 1 }.run();
        let took = t.elapsed();
        eprintln!("{} lines, {} bytes: parsed in {took:?}", p.lines.len(), text.len());
        assert_eq!(p.lines.len(), text.split('\n').count());
    }
}
