//! A text editor for documents of any length: BeOS's BTextView, as
//! StyledEdit's window has it. egui's `TextEdit` lays out, copies and
//! hashes the whole text every frame, which a long document can't afford
//! at 120 frames a second (HACKING.md, the responsiveness rule). Here each
//! paragraph is a galley of its own, laid out only while it shows (egui
//! keeps it while it is drawn). Paragraphs out of sight keep an estimated
//! height; those within a screen of the view are measured before they
//! come into it (a millisecond's worth a frame), and the view stays on the
//! paragraph at its top while heights above it settle, so nothing that
//! shows ever jumps. The rest are never laid out at all.
//!
//! The text and its styles are the app's [`Document`]: the editor reads
//! them a paragraph at a time and makes every edit through
//! [`Document::replace`], so the app's undo and styles follow. Changes the
//! app makes itself (undo, Replace All, loading) it reports with
//! [`Editor::changed`] or [`Editor::reset`], and new styles or fonts with
//! [`Editor::restyled`] or [`Editor::relayout`]. Positions are bytes of
//! the text.
//!
//! Keys are egui's (the field's): arrows, ⌥ or ⌘ with ←/→ by word, ⌘↑/↓
//! and ⌘Home/End to either end, Home/End along the row, Page Up/Down;
//! Shift extends.
//!
//! A paragraph can be a table's row ([`Paragraph::row`]): its text is its
//! cells, a tab between each, each laid out in its column and ruled; the
//! rows of a table are paragraphs one after another. Tab and ⇧Tab go from
//! cell to cell (Tab in the last cell adds a row), Return adds a row
//! below, and edits keep a table's tabs and newlines: what is typed or
//! pasted into a cell is one line, and deleting across cells or rows
//! empties them rather than joining them. A press on the selection drags its text (as
//! [`text_drag`](crate::text_drag)); text dropped on the editor goes in
//! where it lands, and moves when it came from here.
//!
//! ```ignore
//! let out = editor.show(ui, rect, &mut doc, &editor::Options::default());
//! if out.changed { … }
//! ```

use std::{
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    Align, CursorIcon, Galley, Id, Pos2, Rect, Response, Sense, Ui, Vec2,
    accesskit,
    epaint::text::{TabStop, TabStops},
    text::{ByteIndex, CCursor, CharIndex, LayoutJob, LayoutSection, TextFormat},
    vec2,
};

use crate::{dnd, metrics as m, style, text_drag, widgets::ScrollView};

/// The text an [`Editor`] shows and edits, with its styles.
pub trait Document {
    fn text(&self) -> &str;
    /// The styles of bytes `range` (one paragraph, without its newline) as
    /// sections of `job`, whose text is that paragraph (its byte 0 is
    /// `range.start`). An empty paragraph needs one, empty, for its height.
    fn sections(&mut self, range: Range<usize>, job: &mut LayoutJob);
    /// How paragraph `range` (without its newline) is set: indents,
    /// space around it, its alignment, a list's marker. Plain by default.
    fn paragraph(&mut self, _range: Range<usize>) -> Paragraph {
        Paragraph::default()
    }
    /// Replace bytes `range` with `text` (typing, deleting, pasting, a
    /// drop); the selection was `selection` (for undo).
    fn replace(&mut self, range: Range<usize>, text: &str, selection: Range<usize>);
    /// Bytes `range` were copied or cut (to paste them back with styles).
    fn copied(&mut self, _range: Range<usize>) {}
    /// A table's rule was dragged: the row at bytes `range` (and the rest
    /// of its table) should have columns of these widths (shares of the
    /// text's width). Ignored by default.
    fn set_columns(&mut self, _range: Range<usize>, _columns: Vec<f32>) {}
    /// Move bytes `from` to byte `to` (of the text before the move), as
    /// text dragged within the editor does. Where it went.
    fn move_text(&mut self, from: Range<usize>, to: usize, selection: Range<usize>) -> Range<usize> {
        let text = self.text()[from.clone()].to_owned();
        self.replace(from.clone(), "", selection.clone());
        let at = if to > from.end { to - from.len() } else { to };
        self.replace(at..at, &text, selection);
        at..at + text.len()
    }
}

/// How one paragraph is set ([`Document::paragraph`]), in points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paragraph {
    /// Every row starts this far in from the text's left edge.
    pub left: f32,
    /// The first row starts this much further in.
    pub first: f32,
    /// Rows wrap this far in from the right edge.
    pub right: f32,
    /// Space above and below it.
    pub above: f32,
    pub below: f32,
    /// Its own alignment and justification (else the editor's).
    pub align: Option<Align>,
    pub justify: Option<bool>,
    /// A list's bullet or number, drawn before the first row (in the
    /// space `left` leaves), in the paragraph's first style.
    pub marker: Option<String>,
    /// A table's row: its cells, a tab apart, side by side.
    pub row: Option<Row>,
    /// Where its tabs go, from the text's left edge (as a ruler shows
    /// them); None: a stop every four spaces, as in code.
    pub tabs: Option<TabStops>,
}

impl Paragraph {
    /// Its tab stops for its galley, which starts at its indent; `space`
    /// is a space's width in its first font.
    pub fn galley_tabs(&self, space: f32) -> TabStops {
        let left = self.left.max(0.0);
        match &self.tabs {
            Some(t) => TabStops { stops: t.stops.iter().filter(|s| s.x > left).map(|s| TabStop { x: s.x - left, ..*s }).collect(), every: t.every },
            None => TabStops::every(4.0 * space),
        }
    }
}

/// A table's row ([`Paragraph::row`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Each column's share of the text's width (equal shares when there
    /// aren't as many as cells).
    pub columns: Vec<f32>,
    /// A header row: shaded.
    pub header: bool,
}

/// Between a cell's rules and its text, in points.
pub const CELL_INSET: Vec2 = Vec2 { x: 6.0, y: 3.0 };
/// The narrowest a column is dragged to, in points.
const MIN_COLUMN: f32 = 24.0;

/// A table's inner rule being dragged: its row (paragraph), which rule
/// (left of cell `k`), the columns' widths when it was taken hold of,
/// where the press was and how far it has gone.
#[derive(Clone, Debug)]
struct ColumnDrag {
    para: usize,
    k: usize,
    widths: Vec<f32>,
    press: f32,
    by: f32,
}

impl ColumnDrag {
    /// How far the rule has gone, kept to columns no narrower than
    /// [`MIN_COLUMN`].
    fn moved(&self) -> f32 {
        self.by.clamp(-(self.widths[self.k - 1] - MIN_COLUMN).max(0.0), (self.widths[self.k] - MIN_COLUMN).max(0.0))
    }
}

/// The left edge and width of each of `n` cells of `row` across `width`
/// (whole points, so rules fall on pixels).
pub fn cell_columns(row: &Row, n: usize, width: f32) -> Vec<(f32, f32)> {
    let n = n.max(1);
    let shares: Vec<f32> = if row.columns.len() == n && row.columns.iter().all(|w| *w > 0.0) { row.columns.clone() } else { vec![1.0; n] };
    let total: f32 = shares.iter().sum();
    let mut sum = 0.0;
    let mut x = 0.0;
    shares
        .iter()
        .map(|w| {
            sum += w;
            let right = (sum / total * width).round();
            let c = (x, right - x);
            x = right;
            c
        })
        .collect()
}

/// Bytes of `para` (a row's text) in each cell, as (start, length).
pub fn cells(para: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, _) in para.match_indices('\t') {
        out.push((from, i - from));
        from = i + 1;
    }
    out.push((from, para.len() - from));
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Typing changes it (else it can be read, selected and copied).
    pub editable: bool,
    /// Rows wrap at the view's width (else it scrolls sideways).
    pub wrap: bool,
    pub align: Align,
    /// Wrapped rows fill the width (all but each paragraph's last).
    pub justify: bool,
    /// Between the view's edges and the text.
    pub margin: Vec2,
    pub background: Option<olo_theme::Rgba>,
    /// Misspelled words are underlined ([`crate::spell`]), all but the
    /// one being typed.
    pub spelling: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { editable: true, wrap: true, align: Align::Min, justify: false, margin: vec2(m::TEXT_INSET, 4.0), background: None, spelling: false }
    }
}

pub struct Output {
    pub response: Response,
    /// The text changed this frame (by typing, pasting, a drop).
    pub changed: bool,
}

/// One paragraph: bytes `start..start + len` of the text (its newline
/// follows), and its size as last laid out or estimated.
#[derive(Clone, Copy, Debug)]
struct Para {
    start: usize,
    len: usize,
    height: f32,
    /// Its rows' widths added up (its width unwrapped).
    natural: f32,
    row_h: f32,
    /// The layout it was measured for (`Editor::key`); `NEVER` or `STALE`.
    key: u64,
    /// How it was set when last laid out.
    set: Set,
}

/// What of a [`Paragraph`] places its galley: indents, the space above
/// (and below, in its height) and its alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Set {
    left: f32,
    right: f32,
    above: f32,
    below: f32,
    align: Option<Align>,
    /// A table's row: its columns and header, hashed.
    row: Option<u64>,
}

/// A paragraph laid out: one galley, or one a cell of a table's row.
#[derive(Clone)]
struct Laid {
    parts: Vec<Part>,
    /// A row's height (its cells' rules) and whether it is a header.
    row: Option<(f32, bool)>,
}

/// A galley of a [`Laid`] paragraph: where it is from the paragraph's
/// origin, the bytes it holds (from the paragraph's start), and its
/// cell's left edge and width.
#[derive(Clone)]
struct Part {
    g: Arc<Galley>,
    at: Vec2,
    start: usize,
    len: usize,
    frame: (f32, f32),
}

impl Laid {
    /// The part (cell) at `x` across the paragraph.
    fn part_at_x(&self, x: f32) -> usize {
        self.parts.iter().rposition(|q| q.frame.0 <= x).unwrap_or(0)
    }

    /// The part holding byte `rel` of the paragraph (a cell's end before
    /// the next's start).
    fn part_of(&self, rel: usize) -> usize {
        self.parts.iter().position(|q| rel <= q.start + q.len).unwrap_or(self.parts.len() - 1)
    }
}

/// Never laid out: its size is a guess from its length.
const NEVER: u64 = 0;
/// Laid out, but not as it is now (restyled): its size is a good guess.
const STALE: u64 = 1;

/// How long a frame spends checking the spelling of paragraphs new to it.
const SPELL_BUDGET: Duration = Duration::from_micros(2000);

/// How long a frame spends measuring paragraphs about to come into view.
const MEASURE_BUDGET: Duration = Duration::from_micros(1000);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    None,
    /// The primary button went down on the text: dragging selects.
    Selecting,
    /// A press on the selection, not yet a drag of its text or a click.
    Armed { press: Pos2, at: usize },
}

/// How layouts are made: the wrap width (None: no wrapping), pixels per
/// point, alignment and justification.
type Params = (Option<f32>, f32, Align, bool);

pub struct Editor {
    id: Id,
    paras: Vec<Para>,
    /// Where each paragraph starts down the text (and the total, last).
    tops: Vec<f32>,
    tops_dirty: bool,
    /// The widest paragraph, unwrapped.
    widest: f32,
    /// The text's width in the view (a table's, when rows don't wrap).
    width: f32,
    params: Option<Params>,
    /// Bumped whenever every layout changes (`Para::key`).
    key: u64,
    /// Estimates are from a measured paragraph (else from guesses).
    calibrated: bool,
    row_h: f32,
    char_w: f32,
    anchor: usize,
    caret: usize,
    /// At the end of a wrapped row, the caret shows at the next's start.
    prefer_next: bool,
    /// Where up and down keep to, across rows.
    h_pos: Option<f32>,
    /// Scroll the caret into view this frame.
    reveal: bool,
    /// Scroll this far this frame (Page Up and Down, a selection dragged
    /// past an edge).
    scroll_by: Vec2,
    gesture: Gesture,
    /// When the caret last moved (it blinks from then).
    touched: f64,
    /// The paragraph at the top of the view (by its first byte) and how
    /// far down into it the view starts.
    top: (usize, f32),
    /// Where the view goes on the next frame ([`Editor::set_view`]).
    set_top: Option<(usize, f32)>,
    /// List markers of the paragraphs laid out this frame, by paragraph.
    markers: std::collections::HashMap<usize, Arc<Galley>>,
    /// Misspelled words (bytes) of paragraphs checked, by their text's
    /// hash: a paragraph is checked once until it changes.
    misspelled: std::collections::HashMap<u64, Arc<Vec<Range<usize>>>>,
    /// Tables' inner rules drawn last frame (where on screen, the row,
    /// which rule, its columns' widths), to take hold of.
    rules: Vec<(Rect, usize, usize, Vec<f32>)>,
    column_drag: Option<ColumnDrag>,
    /// Where the text's left edge was on screen last frame, and how wide
    /// it was ([`Editor::text_span`]).
    span: Option<(f32, f32)>,
}

impl Editor {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug) -> Editor {
        let mut e = Editor {
            id: Id::unique(id),
            paras: Vec::new(),
            tops: Vec::new(),
            tops_dirty: true,
            widest: 0.0,
            width: 400.0,
            params: None,
            key: 2,
            calibrated: false,
            row_h: 16.0,
            char_w: 7.0,
            anchor: 0,
            caret: 0,
            prefer_next: false,
            h_pos: None,
            reveal: false,
            scroll_by: Vec2::ZERO,
            gesture: Gesture::None,
            touched: 0.0,
            top: (0, 0.0),
            set_top: None,
            markers: std::collections::HashMap::new(),
            misspelled: std::collections::HashMap::new(),
            rules: Vec::new(),
            column_drag: None,
            span: None,
        };
        e.reset("");
        e
    }

    /// Check the spelling again (a word was learnt or ignored).
    pub fn respell(&mut self) {
        self.misspelled.clear();
    }

    /// The editor's widget id (it has the keyboard when this is focused).
    pub fn id(&self) -> Id {
        self.id
    }

    /// The text is all new (loaded): every paragraph's size is a guess
    /// again. The selection goes to the start.
    pub fn reset(&mut self, text: &str) {
        self.paras = split(text, 0, text.len()).map(|(start, len)| self.guess(start, len)).collect();
        self.tops_dirty = true;
        self.anchor = 0;
        self.caret = 0;
        self.top = (0, 0.0);
        self.set_top = None;
        self.gesture = Gesture::None;
    }

    /// The app changed the text from `old` to `new` (undo, Replace All):
    /// the paragraphs between what stayed the same are laid out again.
    pub fn changed(&mut self, old: &str, new: &str) {
        let (removed, inserted) = difference(old, new);
        self.replaced(new, removed, inserted);
    }

    /// Bytes `removed` of the text became `inserted` bytes; `text` is the
    /// text now.
    pub fn replaced(&mut self, text: &str, removed: Range<usize>, inserted: usize) {
        let delta = inserted as isize - removed.len() as isize;
        let shift = |at: usize| if at >= removed.end { (at as isize + delta) as usize } else if at > removed.start { removed.start } else { at };
        let p0 = self.para_of(removed.start);
        let p1 = self.para_of(removed.end);
        let start = self.paras[p0].start;
        let end = ((self.paras[p1].start + self.paras[p1].len) as isize + delta) as usize;
        let was = self.paras[p0];
        let mut new: Vec<Para> = split(text, start, end).map(|(s, len)| self.guess(s, len)).collect();
        // The first keeps its size until measured (it is most likely the
        // one being typed in, on screen).
        new[0] = Para { start, len: new[0].len, key: STALE, ..was };
        let n = new.len();
        self.paras.splice(p0..=p1, new);
        for p in &mut self.paras[p0 + n..] {
            p.start = (p.start as isize + delta) as usize;
        }
        self.tops_dirty = true;
        self.anchor = shift(self.anchor).min(text.len());
        self.caret = shift(self.caret).min(text.len());
        self.top.0 = shift(self.top.0).min(text.len());
    }

    /// Bytes `range` have new styles: their paragraphs are laid out again
    /// (those out of sight when they are next measured).
    pub fn restyled(&mut self, range: Range<usize>) {
        let (a, b) = (self.para_of(range.start), self.para_of(range.end));
        for p in &mut self.paras[a..=b] {
            p.key = p.key.min(STALE);
        }
    }

    /// Everything is laid out again (fonts loaded, the alignment changed).
    pub fn relayout(&mut self) {
        self.key += 1;
    }

    /// The selection (bytes, in order).
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// The caret (where the selection was extended to).
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Select bytes `r` of `text` (the caret at its end) and scroll it into
    /// view.
    pub fn select(&mut self, text: &str, r: Range<usize>) {
        self.anchor = floor(text, r.start);
        self.caret = floor(text, r.end);
        self.h_pos = None;
        self.prefer_next = false;
        self.reveal = true;
    }

    /// Where the view is: the first byte of the paragraph at its top and
    /// how far down into that it starts, to put it back with
    /// [`Editor::set_view`] (after a reload, say).
    pub fn view(&self) -> (usize, f32) {
        self.top
    }

    /// Put the view where [`Editor::view`] said, in `text`, on the next
    /// frame (instead of showing the caret, after [`Editor::select`]).
    pub fn set_view(&mut self, text: &str, (at, down): (usize, f32)) {
        self.set_top = Some((floor(text, at), down.max(0.0)));
        self.reveal = false;
    }

    /// Line (paragraph) and column (characters) of byte `at`, from 1.
    pub fn line_col(&self, text: &str, at: usize) -> (usize, usize) {
        let i = self.para_of(at);
        let p = self.paras[i];
        (i + 1, text[p.start..at.clamp(p.start, p.start + p.len)].chars().count() + 1)
    }

    /// The paragraph holding byte `at` (a newline belongs to the one it ends).
    fn para_of(&self, at: usize) -> usize {
        self.paras.partition_point(|p| p.start <= at).saturating_sub(1)
    }

    fn guess(&self, start: usize, len: usize) -> Para {
        let mut p = Para { start, len, height: 0.0, natural: len as f32 * self.char_w, row_h: self.row_h, key: NEVER, set: Set::default() };
        p.height = self.estimate(&p);
        p
    }

    fn estimate(&self, p: &Para) -> f32 {
        let rows = match self.params.and_then(|p| p.0) {
            Some(w) if w > 0.0 => (p.natural / w).ceil().max(1.0),
            _ => 1.0,
        };
        rows * p.row_h + p.set.above + p.set.below
    }

    fn ensure_tops(&mut self) {
        if !self.tops_dirty && self.tops.len() == self.paras.len() + 1 {
            return;
        }
        self.tops.clear();
        self.tops.reserve(self.paras.len() + 1);
        let (mut y, mut widest) = (0.0, 0.0f32);
        for p in &self.paras {
            self.tops.push(y);
            y += p.height;
            widest = widest.max(p.natural);
        }
        self.tops.push(y);
        self.widest = widest;
        self.tops_dirty = false;
    }

    /// The paragraph at `y` down the text (clamped to the first and last).
    fn para_at_y(&self, y: f32) -> usize {
        self.tops[1..].partition_point(|&t| t <= y).min(self.paras.len() - 1)
    }

    /// New layout parameters: every paragraph's size is a guess until it
    /// is measured again (from what it measured before).
    fn set_params(&mut self, params: Params) {
        if self.params == Some(params) {
            return;
        }
        self.params = Some(params);
        self.key += 1;
        for i in 0..self.paras.len() {
            let h = self.estimate(&self.paras[i]);
            self.paras[i].height = h;
        }
        self.tops_dirty = true;
    }

    /// Paragraph `i` laid out (egui keeps galleys while they're used), and
    /// measured: one galley, or one a cell of a table's row.
    fn laid(&mut self, ui: &Ui, doc: &mut dyn Document, i: usize) -> Laid {
        let (wrap, _, align, justify) = self.params.unwrap_or((None, 1.0, Align::Min, false));
        let p = self.paras[i];
        let range = p.start..p.start + p.len;
        let f = doc.paragraph(range.clone());
        let row_key = f.row.as_ref().map(|r| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            r.columns.iter().for_each(|c| c.to_bits().hash(&mut h));
            r.header.hash(&mut h);
            h.finish()
        });
        // (A table's rows meet: no space between them.)
        let (above, below) = if f.row.is_some() { (0.0, 0.0) } else { (f.above.max(0.0), f.below.max(0.0)) };
        let set = Set { left: f.left.max(0.0), right: f.right.max(0.0), above, below, align: f.align, row: row_key };
        let laid = match &f.row {
            None => {
                let mut job = LayoutJob {
                    text: doc.text()[range.clone()].to_owned(),
                    halign: f.align.unwrap_or(align),
                    justify: f.justify.unwrap_or(justify),
                    keep_trailing_whitespace: true,
                    ..Default::default()
                };
                job.wrap.max_width = wrap.map_or(f32::INFINITY, |w| (w - set.left - set.right).max(1.0));
                doc.sections(range, &mut job);
                if job.sections.is_empty() {
                    job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(p.len), format: TextFormat::default() });
                }
                job.sections[0].leading_space += f.first.max(0.0);
                let space = ui.fonts_mut(|fonts| fonts.glyph_width(&job.sections[0].format.font_id, ' '));
                job.tabs = Some(f.galley_tabs(space));
                // A list's marker, in the paragraph's first style (for this
                // frame's drawing).
                match f.marker {
                    Some(m) if !m.is_empty() => {
                        let format = job.sections[0].format.clone();
                        let mark = ui.fonts_mut(|f| f.layout_job(LayoutJob::single_section(m, format)));
                        self.markers.insert(i, mark);
                    }
                    _ => {
                        self.markers.remove(&i);
                    }
                }
                let g = ui.fonts_mut(|f| f.layout_job(job));
                Laid { parts: vec![Part { g, at: Vec2::ZERO, start: 0, len: p.len, frame: (0.0, 0.0) }], row: None }
            }
            Some(row) => {
                self.markers.remove(&i);
                let width = (wrap.unwrap_or(self.width) - set.left - set.right).max(1.0);
                let text = doc.text()[range].to_owned();
                let cells = cells(&text);
                let columns = cell_columns(row, cells.len(), width);
                let halign = f.align.unwrap_or(Align::Min);
                let mut parts = Vec::with_capacity(cells.len());
                for ((start, len), (x, w)) in cells.into_iter().zip(columns) {
                    let inner = (w - 2.0 * CELL_INSET.x).max(1.0);
                    let mut job = LayoutJob { text: text[start..start + len].to_owned(), halign, justify: f.justify.unwrap_or(false), keep_trailing_whitespace: true, ..Default::default() };
                    job.wrap.max_width = inner;
                    doc.sections(p.start + start..p.start + start + len, &mut job);
                    if job.sections.is_empty() {
                        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(len), format: TextFormat::default() });
                    }
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    let ax = match halign {
                        Align::Min => 0.0,
                        Align::Center => inner / 2.0,
                        Align::Max => inner,
                    };
                    parts.push(Part { g, at: vec2(x + CELL_INSET.x + ax, CELL_INSET.y), start, len, frame: (x, w) });
                }
                // A whole number of points high, the text in the middle of
                // what rounding leaves.
                let tallest = parts.iter().map(|q| q.g.size().y).fold(0.0, f32::max);
                let h = (tallest + 2.0 * CELL_INSET.y).round();
                for q in &mut parts {
                    q.at.y = (h - tallest) / 2.0;
                }
                Laid { parts, row: Some((h, row.header)) }
            }
        };
        if set != p.set {
            self.paras[i].set = set;
            self.tops_dirty = true;
        }
        if p.key != self.key || set != p.set {
            self.measured(i, &laid);
        }
        laid
    }

    fn measured(&mut self, i: usize, laid: &Laid) {
        let wrapped = self.params.is_some_and(|p| p.0.is_some());
        let g = &laid.parts[0].g;
        let rows = g.rows.len().max(1) as f32;
        let p = &mut self.paras[i];
        let first = p.key == NEVER;
        let body = laid.row.map_or(g.size().y, |(h, _)| h);
        let h = body + p.set.above + p.set.below;
        if p.height != h {
            self.tops_dirty = true;
        }
        p.height = h;
        p.natural = match laid.row {
            // A row is as wide as its columns (one row high, as guessed).
            Some(_) => laid.parts.iter().map(|q| q.frame.1).sum::<f32>() + p.set.left + p.set.right,
            None if wrapped => g.rows.iter().map(|r| r.size.x).sum(),
            None => g.size().x + p.set.left + p.set.right,
        };
        p.row_h = if laid.row.is_some() { body } else { g.size().y / rows };
        p.key = self.key;
        let p = *p;
        if first && p.len > 0 && laid.row.is_none() {
            let w = p.natural / p.len as f32;
            if !self.calibrated {
                // Guesses so far were from nothing: from this one now.
                self.calibrated = true;
                self.char_w = w;
                self.row_h = p.row_h;
                for q in self.paras.iter_mut().filter(|q| q.key == NEVER) {
                    q.natural = q.len as f32 * w;
                    q.row_h = p.row_h;
                }
                for k in 0..self.paras.len() {
                    if self.paras[k].key == NEVER {
                        self.paras[k].height = self.estimate(&self.paras[k]);
                    }
                }
                self.tops_dirty = true;
            } else {
                self.char_w += (w - self.char_w) * 0.05;
                self.row_h += (p.row_h - self.row_h) * 0.05;
            }
        }
    }

    /// The x of paragraph `i`'s galley origin from the text's left (its
    /// indent and alignment).
    fn shift(&self, i: usize, inner: f32) -> f32 {
        let (wrap, _, align, _) = self.params.unwrap_or((None, 1.0, Align::Min, false));
        let set = self.paras[i].set;
        // (A row's cells align their own text.)
        if set.row.is_some() {
            return set.left;
        }
        let w = (wrap.unwrap_or(inner.max(self.widest)) - set.left - set.right).max(1.0);
        set.left
            + match set.align.unwrap_or(align) {
                Align::Min => 0.0,
                Align::Center => w / 2.0,
                Align::Max => w,
            }
    }

    /// Paragraph `i`'s galley origin in content coordinates.
    fn origin(&self, i: usize, o: &Options, inner: f32) -> Vec2 {
        vec2(o.margin.x + self.shift(i, inner), o.margin.y + self.tops[i] + self.paras[i].set.above)
    }

    /// The byte at content point `pos` (and whether it is at the start of
    /// a wrapped row rather than the end of the one before).
    fn byte_at(&mut self, ui: &Ui, doc: &mut dyn Document, o: &Options, inner: f32, pos: Pos2) -> (usize, bool) {
        self.ensure_tops();
        let i = self.para_at_y(pos.y - o.margin.y);
        let laid = self.laid(ui, doc, i);
        self.ensure_tops();
        let local = pos.to_vec2() - self.origin(i, o, inner);
        let q = &laid.parts[laid.part_at_x(local.x)];
        let c = q.g.cursor_from_pos(local - q.at);
        let base = self.paras[i].start + q.start;
        (base + byte_of_col(&doc.text()[base..base + q.len], c.index.0), c.prefer_next_row)
    }

    /// Byte `at`'s paragraph, its part (cell) and the cursor in that.
    fn cursor(&mut self, ui: &Ui, doc: &mut dyn Document, at: usize, prefer_next: bool) -> (usize, Part, CCursor) {
        let i = self.para_of(at);
        let laid = self.laid(ui, doc, i);
        let p = self.paras[i];
        let rel = at.clamp(p.start, p.start + p.len) - p.start;
        let q = laid.parts[laid.part_of(rel)].clone();
        let col = doc.text()[p.start + q.start..p.start + rel.max(q.start)].chars().count();
        (i, q, CCursor { index: CharIndex(col), prefer_next_row: prefer_next })
    }

    /// The caret's rectangle (zero wide) in content coordinates.
    fn caret_rect(&mut self, ui: &Ui, doc: &mut dyn Document, o: &Options, inner: f32, at: usize, prefer_next: bool) -> Rect {
        let (i, q, c) = self.cursor(ui, doc, at, prefer_next);
        self.ensure_tops();
        q.g.pos_from_cursor(c).translate(self.origin(i, o, inner) + q.at)
    }

    /// The byte a row up or down from the caret, keeping to `h_pos` (an
    /// x from the text's left, so paragraphs indented differently line up).
    fn vertical(&mut self, ui: &Ui, doc: &mut dyn Document, inner: f32, down: bool) -> (usize, bool) {
        let (i, q, c) = self.cursor(ui, doc, self.caret, self.prefer_next);
        let lc = q.g.layout_from_cursor(c);
        let here = q.g.pos_from_cursor(c).center().x + self.shift(i, inner) + q.at.x;
        let h = *self.h_pos.get_or_insert(here);
        // The paragraph and part (a row's cell under `h`) the row is in.
        let mut beside = |e: &mut Editor, pi: usize| {
            let laid = e.laid(ui, doc, pi);
            laid.parts[laid.part_at_x(h - e.shift(pi, inner))].clone()
        };
        let (pi, part, row) = if down && lc.row + 1 < q.g.rows.len() {
            (i, q, lc.row + 1)
        } else if !down && lc.row > 0 {
            (i, q, lc.row - 1)
        } else if down && i + 1 < self.paras.len() {
            (i + 1, beside(self, i + 1), 0)
        } else if !down && i > 0 {
            let part = beside(self, i - 1);
            let last = part.g.rows.len().saturating_sub(1);
            (i - 1, part, last)
        } else {
            // Past the first or last row: to the very start or end.
            let to = if down { doc.text().len() } else { 0 };
            return (to, false);
        };
        let base = self.paras[pi].start + part.start;
        let Some(r) = part.g.rows.get(row) else { return (base, false) };
        let column = r.char_at(h - self.shift(pi, inner) - part.at.x - r.pos.x).0.min(r.char_count_excluding_newline().0);
        let col: usize = part.g.rows[..row].iter().map(|r| r.char_count_including_newline().0).sum::<usize>() + column;
        (base + byte_of_col(&doc.text()[base..base + part.len], col), column < r.char_count_excluding_newline().0)
    }

    /// The start or end of the caret's row.
    fn row_end(&mut self, ui: &Ui, doc: &mut dyn Document, end: bool) -> (usize, bool) {
        let (i, q, c) = self.cursor(ui, doc, self.caret, self.prefer_next);
        let c = if end { q.g.cursor_end_of_row(&c) } else { q.g.cursor_begin_of_row(&c) };
        let base = self.paras[i].start + q.start;
        (base + byte_of_col(&doc.text()[base..base + q.len], c.index.0), c.prefer_next_row)
    }

    /// Replace bytes `range` with `text` (typed, pasted, deleted), the
    /// caret after it. Where a table's row is touched, its structure
    /// stays: text put in a cell is one line (tabs and newlines become
    /// spaces), and the tabs and newlines going are put back after it, so
    /// cells and rows empty rather than join.
    fn edit(&mut self, doc: &mut dyn Document, range: Range<usize>, text: &str) {
        let (a, b) = (self.para_of(range.start), self.para_of(range.end));
        if !(a..=b).any(|i| self.paras[i].set.row.is_some()) {
            return self.edit_raw(doc, range, text, text.len());
        }
        let clean: String = text.chars().map(|c| if c == '\t' || c == '\n' { ' ' } else { c }).collect();
        let kept: String = doc.text()[range.clone()].chars().filter(|c| *c == '\t' || *c == '\n').collect();
        if clean.is_empty() && kept == doc.text()[range.clone()] {
            // Only structure (a cell's start, backspaced): the caret goes
            // back over it instead.
            self.anchor = range.start;
            self.caret = range.start;
            self.prefer_next = false;
            self.h_pos = None;
            self.reveal = true;
            return;
        }
        let caret = clean.len();
        self.edit_raw(doc, range, &(clean + &kept), caret);
    }

    /// Replace bytes `range` with `text` as it is, the caret `caret` bytes
    /// into it.
    fn edit_raw(&mut self, doc: &mut dyn Document, range: Range<usize>, text: &str, caret: usize) {
        let sel = self.selection();
        doc.replace(range.clone(), text, sel);
        self.replaced(doc.text(), range.clone(), text.len());
        self.anchor = range.start + caret;
        self.caret = self.anchor;
        self.prefer_next = false;
        self.h_pos = None;
        self.reveal = true;
    }

    /// The caret's cell, if it is in a table's row: the row's paragraph,
    /// its cells (bytes of the text) and which one.
    fn cell(&self, text: &str) -> Option<(usize, Vec<Range<usize>>, usize)> {
        let i = self.para_of(self.caret);
        let p = self.paras[i];
        p.set.row?;
        let cells: Vec<Range<usize>> = cells(&text[p.start..p.start + p.len]).into_iter().map(|(s, l)| p.start + s..p.start + s + l).collect();
        let k = cells.iter().position(|c| self.caret <= c.end).unwrap_or(cells.len() - 1);
        Some((i, cells, k))
    }

    /// Tab (or ⇧Tab) in a table's row: the next (previous) cell, all of
    /// it selected; Tab in the last cell of a table's last row adds a row.
    fn tab(&mut self, doc: &mut dyn Document, back: bool) -> Option<bool> {
        let (i, cells, k) = self.cell(doc.text())?;
        let select = |e: &mut Editor, r: Range<usize>| {
            e.anchor = r.start;
            e.caret = r.end;
            e.h_pos = None;
            e.reveal = true;
        };
        if back {
            if k > 0 {
                select(self, cells[k - 1].clone());
            } else if i > 0 && self.paras[i - 1].set.row.is_some() {
                let q = self.paras[i - 1];
                let last = cells_of(doc.text(), q.start, q.len).pop().unwrap_or(q.start..q.start);
                select(self, last);
            }
            return Some(false);
        }
        if k + 1 < cells.len() {
            select(self, cells[k + 1].clone());
            return Some(false);
        }
        if i + 1 < self.paras.len() && self.paras[i + 1].set.row.is_some() {
            let q = self.paras[i + 1];
            let first = cells_of(doc.text(), q.start, q.len).swap_remove(0);
            select(self, first);
            return Some(false);
        }
        Some(self.add_row(doc, i, cells.len()))
    }

    /// A row of `n` empty cells after row `i`, the caret in its first.
    fn add_row(&mut self, doc: &mut dyn Document, i: usize, n: usize) -> bool {
        let p = self.paras[i];
        let end = p.start + p.len;
        let row = format!("\n{}", "\t".repeat(n.saturating_sub(1)));
        self.anchor = end;
        self.caret = end;
        self.edit_raw(doc, end..end, &row, 1);
        true
    }

    /// Where the text's left edge was on screen when last shown (scrolled
    /// sideways too), and its width (the wrap width, or the view's): for
    /// a ruler over it.
    pub fn text_span(&self) -> Option<(f32, f32)> {
        self.span
    }

    /// Show the editor in `rect`.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, doc: &mut dyn Document, o: &Options) -> Output {
        let focused = ui.memory(|mem| mem.has_focus(self.id));
        let mut view = ScrollView::new(self.id.with("scroll")).horizontal(!o.wrap).focused(focused);
        if let Some(bg) = o.background {
            view = view.background(bg);
        }
        view.show_in(ui, rect, |ui, vp| self.body(ui, vp, doc, o))
    }

    fn body(&mut self, ui: &mut Ui, vp: Rect, doc: &mut dyn Document, o: &Options) -> Output {
        let ctx = ui.ctx().clone();
        let id = self.id;
        let ppp = ctx.pixels_per_point();
        let screen = Rect::from_min_size(ui.max_rect().min + vp.min.to_vec2(), vp.size());
        let inner = (vp.width() - 2.0 * o.margin.x).max(1.0);
        self.span = Some((screen.min.x - vp.min.x + o.margin.x, inner.round()));
        if !o.wrap && (self.width - inner.round()).abs() > 0.5 {
            // (Unwrapped, a table is the view's width.)
            self.width = inner.round();
            self.key += 1;
        }
        self.set_params((o.wrap.then_some(inner.round()), ppp, o.align, o.justify));
        // The paragraph at the top of the view, where it stays.
        self.ensure_tops();
        let t = self.para_at_y(vp.min.y - o.margin.y);
        self.top = match self.set_top.take() {
            Some((at, down)) => (self.paras[self.para_of(at)].start, down),
            None => (self.paras[t].start, vp.min.y - (o.margin.y + self.tops[t])),
        };

        // Field-like to the kit and egui: ⌘C/⌘X/⌘V and ⌘A come here, not
        // to the menus.
        if egui::text_edit::TextEditState::load(&ctx, id).is_none() {
            egui::text_edit::TextEditState::default().store(&ctx, id);
        }
        let response = ui.interact(screen, id, Sense::click_and_drag());
        let to_content = |p: Pos2| Pos2::ZERO + (p - screen.min) + vp.min.to_vec2();
        let changed = self.input(ui, &response, doc, o, inner, vp, &to_content);

        // Where the view goes: kept on its top paragraph, or to the caret.
        let content_h = |e: &Editor| e.tops[e.paras.len()] + 2.0 * o.margin.y;
        let mut offset = vp.min.to_vec2();
        if self.scroll_by != Vec2::ZERO {
            // The caret went as far, so it stays where it shows.
            let by = std::mem::take(&mut self.scroll_by);
            offset.y = (offset.y + by.y).clamp(0.0, (content_h(self) - vp.height()).max(0.0));
            if !o.wrap {
                offset.x = (offset.x + by.x).clamp(0.0, (self.widest + 2.0 * o.margin.x - vp.width()).max(0.0));
            }
            self.reveal = false;
            let t = self.para_at_y(offset.y - o.margin.y);
            self.top = (self.paras[t].start, offset.y - (o.margin.y + self.tops[t]));
        }
        // Lay out what shows, measuring it (which may move what follows,
        // or the caret being revealed): again until it settles.
        let revealing = std::mem::take(&mut self.reveal);
        let mut shown: Vec<(usize, Laid)> = Vec::new();
        self.markers.clear();
        for pass in 0..6 {
            if revealing {
                let r = self.caret_rect(ui, doc, o, inner, self.caret, self.prefer_next);
                let pad = 2.0 * self.row_h.min(vp.height() / 4.0);
                if r.top() < offset.y + pad * 0.5 {
                    offset.y = r.top() - pad;
                } else if r.bottom() > offset.y + vp.height() - pad * 0.5 {
                    offset.y = r.bottom() + pad - vp.height();
                }
                if pass == 0 && !o.wrap {
                    if r.left() < vp.min.x + o.margin.x {
                        offset.x = r.left() - o.margin.x - inner / 4.0;
                    } else if r.right() > vp.max.x - o.margin.x {
                        offset.x = r.right() + o.margin.x + inner / 4.0 - vp.width();
                    }
                    offset.x = offset.x.max(0.0);
                }
                self.ensure_tops();
                offset.y = offset.y.clamp(0.0, (content_h(self) - vp.height()).max(0.0));
                let t = self.para_at_y(offset.y - o.margin.y);
                self.top = (self.paras[t].start, offset.y - (o.margin.y + self.tops[t]));
            }
            if pass == 0 {
                // Paragraphs about to show, measured while this frame has
                // time.
                self.ensure_tops();
                let a = self.para_of(self.top.0);
                let top = self.tops[a] + self.top.1;
                self.measure_ahead(ui, doc, top, top + vp.height());
            }
            self.ensure_tops();
            let a = self.para_of(self.top.0);
            offset.y = (o.margin.y + self.tops[a] + self.top.1.min(self.paras[a].height)).clamp(0.0, (content_h(self) - vp.height()).max(0.0));
            let first = self.para_at_y(offset.y - o.margin.y);
            shown.clear();
            let mut i = first;
            while i < self.paras.len() && o.margin.y + self.tops[i] < offset.y + vp.height() {
                let laid = self.laid(ui, doc, i);
                shown.push((i, laid));
                i += 1;
            }
            if !self.tops_dirty {
                break;
            }
        }
        self.ensure_tops();
        if (offset - vp.min.to_vec2()).length() > 0.01 {
            ScrollView::scroll_to(&ctx, self.id.with("scroll"), offset);
        }
        let width = if o.wrap { vp.width() } else { vp.width().max(self.widest + 2.0 * o.margin.x) };
        ui.set_min_size(vec2(width, content_h(self).max(vp.height())));

        // Where content point `p` is drawn this frame.
        let origin = screen.min - offset;
        let snap = |p: Pos2| Pos2::new((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);
        let painter = ui.painter().clone();
        let active = ui.memory(|mem| mem.has_focus(id)) && ui.input(|i| i.focused);
        let sel = self.selection();
        let visuals = ui.visuals().clone();
        let fill = if active { visuals.selection.bg_fill } else { visuals.selection.bg_fill.gamma_multiply(0.6) };
        let text = doc.text();
        let checker = (o.spelling && o.editable).then(|| crate::spell::checker(&ctx)).flatten();
        let squiggle_color = style::c(style::palette(&ctx).failure);
        let checking = Instant::now();
        // Rules in the text's colour, faint (a border's colour is lost on a
        // dark page); a header shaded with the controls'.
        let rule = visuals.text_color().gamma_multiply(0.35);
        let shade = style::c(style::palette(&ctx).control_background);
        let dragging = style::c(style::palette(&ctx).keyboard_navigation);
        self.rules.clear();
        // The table a rule is being dragged in: its rows' paragraphs.
        let drag_table = self.column_drag.as_ref().map(|d| {
            let (mut a, mut b) = (d.para, d.para);
            while a > 0 && self.paras[a - 1].set.row.is_some() {
                a -= 1;
            }
            while b + 1 < self.paras.len() && self.paras[b + 1].set.row.is_some() {
                b += 1;
            }
            a..=b
        });
        for (i, laid) in &shown {
            let p = self.paras[*i];
            let at = snap(origin + self.origin(*i, o, inner));
            let end = p.start + p.len;
            // A row: its cells' rules, a device pixel wide on device
            // pixels (rows meet on one), and a header's shade.
            if let Some((h, header)) = laid.row {
                let px = 1.0 / ppp;
                let line = |v: f32| ((v * ppp).floor() + 0.5) / ppp;
                // From the rows' tops before snapping, so a row's bottom
                // rule and the next's top fall on the same pixel.
                let y0 = origin.y + o.margin.y + self.tops[*i];
                let (top, bottom) = (line(y0), line(y0 + h));
                for q in &laid.parts {
                    let (l, r) = (line(at.x + q.frame.0), line(at.x + q.frame.0 + q.frame.1));
                    if header {
                        // Whole pixels, the rules' own included.
                        let half = px / 2.0;
                        painter.rect_filled(Rect::from_min_max(Pos2::new(l - half, top - half), Pos2::new(r + half, bottom + half)), 0.0, shade);
                    }
                    painter.line_segment([Pos2::new(l, top), Pos2::new(l, bottom)], egui::Stroke::new(px, rule));
                    painter.line_segment([Pos2::new(r, top), Pos2::new(r, bottom)], egui::Stroke::new(px, rule));
                    painter.line_segment([Pos2::new(l, top), Pos2::new(r, top)], egui::Stroke::new(px, rule));
                    painter.line_segment([Pos2::new(l, bottom), Pos2::new(r, bottom)], egui::Stroke::new(px, rule));
                }
                // The inner rules, to take hold of (a few points either side).
                let widths: Vec<f32> = laid.parts.iter().map(|q| q.frame.1).collect();
                for (k, q) in laid.parts.iter().enumerate().skip(1) {
                    let x = at.x + q.frame.0;
                    self.rules.push((Rect::from_x_y_ranges(x - 3.0..=x + 3.0, top..=bottom), *i, k, widths.clone()));
                }
                // One being dragged: where it would go, across the table.
                if let (Some(d), Some(t)) = (&self.column_drag, &drag_table)
                    && t.contains(i)
                    && let Some(q) = laid.parts.get(d.k)
                {
                    let x = line(at.x + q.frame.0 + d.moved());
                    painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom)], egui::Stroke::new(2.0 * px, dragging));
                }
            }
            for q in &laid.parts {
                let (qs, qe) = (p.start + q.start, p.start + q.start + q.len);
                let mut qat = snap(at + q.at);
                if laid.row.is_some() {
                    // Up a part pixel rather than down: capitals sit a
                    // little below their line's middle, so a cell's text
                    // rounded down would look low in its row.
                    qat.y = ((at.y + q.at.y) * ppp).floor() / ppp;
                }
                // The selection behind the text (the paragraph's newline,
                // or a cell's tab, too, as a sliver).
                if !sel.is_empty() && sel.start <= qe && sel.end > qs {
                    let part = &text[qs..qe];
                    let a = part[..sel.start.clamp(qs, qe) - qs].chars().count();
                    let b = part[..sel.end.clamp(qs, qe) - qs].chars().count();
                    let past = sel.end > qe;
                    let mut c0 = 0;
                    for (ri, row) in q.g.rows.iter().enumerate() {
                        let n = row.char_count_excluding_newline().0;
                        let last = ri + 1 == q.g.rows.len();
                        // On past this row's end: to its edge (and the
                        // paragraph's newline, a sliver after the last).
                        let on = if last { past } else { b > c0 + n };
                        if a <= c0 + n && b >= c0 {
                            let left = row.x_offset(CharIndex(a.max(c0) - c0));
                            let right = if on { row.size.x + if last { row.size.y * 0.5 } else { 0.0 } } else { row.x_offset(CharIndex(b.min(c0 + n) - c0)) };
                            if right > left {
                                let r = Rect::from_min_max(snap(qat + row.pos.to_vec2() + vec2(left, 0.0)), snap(qat + row.pos.to_vec2() + vec2(right, row.size.y)));
                                painter.rect_filled(r, 0.0, fill);
                            }
                        }
                        c0 += n;
                    }
                }
                painter.galley(qat, q.g.clone(), visuals.text_color());
            }
            if let Some(c) = &checker {
                let para = &text[p.start..end];
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    para.hash(&mut h);
                    h.finish()
                };
                if self.misspelled.len() > 4096 {
                    self.misspelled.clear();
                }
                // Checked once, within a budget a frame (the rest next frame).
                let wrong = match self.misspelled.get(&key) {
                    Some(w) => w.clone(),
                    None if checking.elapsed() < SPELL_BUDGET => {
                        let w = Arc::new(c.misspelled(para));
                        self.misspelled.insert(key, w.clone());
                        w
                    }
                    None => {
                        ctx.request_repaint();
                        Arc::new(Vec::new())
                    }
                };
                for r in wrong.iter() {
                    // Not the word being typed.
                    if active && sel.is_empty() && (p.start + r.start..=p.start + r.end).contains(&self.caret) {
                        continue;
                    }
                    // In its cell (a word never crosses a tab).
                    let q = &laid.parts[laid.part_of(r.start)];
                    let part = &para[q.start..q.start + q.len];
                    let local = r.start.saturating_sub(q.start)..(r.end - q.start).min(q.len);
                    squiggle(&painter, &q.g, snap(at + q.at), part, local, ppp, squiggle_color);
                }
            }
            if let Some(m) = self.markers.get(i) {
                let g = &laid.parts[0].g;
                let first = g.rows.first().map_or(0.0, |r| r.pos.y);
                let w = m.size().x;
                let gap = (m.size().y * 0.35).round();
                let x = origin.x + o.margin.x + self.paras[*i].set.left - gap - w;
                painter.galley(snap(Pos2::new(x, at.y + first)), m.clone(), visuals.text_color());
            }
        }
        if active && sel.is_empty() {
            let r = self.caret_rect(ui, doc, o, inner, self.caret, self.prefer_next).translate(origin.to_vec2());
            let x = snap(r.center_top()).x;
            let r = Rect::from_x_y_ranges(x..=x, r.y_range());
            egui::text_selection::visuals::paint_text_cursor(ui, &painter, r, ui.input(|i| i.time) - self.touched);
        }

        // Text dragged over: where it would go.
        if o.editable
            && let Some(h) = dnd::hover(&ctx)
            && !h.has_files()
            && screen.contains(h.pos)
            && let Some(mime) = h.text_type()
        {
            dnd::accept(&ctx, text_drag::target(id), mime, dnd::Action::Copy);
            let (at, next) = self.byte_at(ui, doc, o, inner, to_content(h.pos));
            let r = self.caret_rect(ui, doc, o, inner, at, next).translate(origin.to_vec2());
            let p = style::palette(&ctx);
            let x = snap(r.center_top()).x;
            painter.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], egui::Stroke::new(2.0, style::c(p.keyboard_navigation)));
        }

        // For agents (and screen readers): the whole text, where it shows.
        let text = doc.text();
        ctx.accesskit_node_builder(id, |n| {
            n.set_role(accesskit::Role::MultilineTextInput);
            n.set_value(text);
            n.set_bounds(accesskit::Rect { x0: screen.min.x as f64, y0: screen.min.y as f64, x1: screen.max.x as f64, y1: screen.max.y as f64 });
            if !o.editable {
                n.set_read_only();
            }
            n.add_action(accesskit::Action::Focus);
        });
        Output { response, changed }
    }

    /// Measure the paragraphs within a screen above and below the view
    /// (`top` to `bottom` down the text), nearest first, within the
    /// frame's budget; asks for another frame while some are left.
    fn measure_ahead(&mut self, ui: &Ui, doc: &mut dyn Document, top: f32, bottom: f32) {
        self.ensure_tops();
        let key = self.key;
        let view = bottom - top;
        let (a, b) = (self.para_at_y(top), self.para_at_y(bottom));
        let (above, below) = (self.para_at_y(top - view), self.para_at_y(bottom + view));
        let start = Instant::now();
        for i in (b + 1..=below).chain((above..a).rev()) {
            if self.paras[i].key == key {
                continue;
            }
            if start.elapsed() >= MEASURE_BUDGET {
                ui.ctx().request_repaint();
                return;
            }
            self.laid(ui, doc, i);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn input(&mut self, ui: &Ui, response: &Response, doc: &mut dyn Document, o: &Options, inner: f32, vp: Rect, content: &dyn Fn(Pos2) -> Pos2) -> bool {
        let ctx = ui.ctx().clone();
        let id = self.id;
        let now = ui.input(|i| i.time);
        let mut changed = false;

        // A drop: in where it lands (moved, if it came from here).
        if let Some(drop) = dnd::take_widget_drop(&ctx, &text_drag::target(id))
            && o.editable
        {
            let text = drop.text().replace("\r\n", "\n");
            let (at, _) = self.byte_at(ui, doc, o, inner, content(drop.pos));
            let source = ctx.data_mut(|d| d.remove_temp::<Option<text_drag::Source>>(text_drag::source_id())).flatten();
            let sel = self.selection();
            match source {
                Some(s) if drop.from_here && s.id == id => {
                    let (from, to) = (s.range, at);
                    if !(from.start..=from.end).contains(&to) {
                        let region = from.start.min(to)..from.end.max(to);
                        let r = doc.move_text(from, to, sel);
                        self.replaced(doc.text(), region.clone(), region.len());
                        self.select(doc.text(), r);
                        changed = true;
                    }
                }
                _ => {
                    self.edit(doc, at..at, &text);
                    self.anchor = at;
                    changed = true;
                }
            }
            ctx.memory_mut(|mem| mem.request_focus(id));
            self.touched = now;
        }

        // The pointer.
        if response.hovered() {
            ctx.set_cursor_icon(CursorIcon::Text);
        }
        let (pressed, down, latest, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
        // A table's inner rule: dragged, it sizes the columns either side
        // (not selecting text); let go, the app is told.
        let on_rule = latest.filter(|_| o.editable && response.hovered()).and_then(|p| self.rules.iter().find(|(r, ..)| r.contains(p)).cloned());
        if on_rule.is_some() || self.column_drag.is_some() {
            ctx.set_cursor_icon(CursorIcon::ResizeColumn);
        }
        if pressed
            && let (Some((_, para, k, widths)), Some(p)) = (on_rule, latest)
        {
            self.column_drag = Some(ColumnDrag { para, k, widths, press: p.x, by: 0.0 });
            ctx.memory_mut(|mem| mem.request_focus(id));
            return changed;
        }
        if let Some(d) = &mut self.column_drag {
            if down {
                if let Some(p) = latest {
                    d.by = p.x - d.press;
                }
                return changed;
            }
            let d = self.column_drag.take().expect("a drag");
            let mut widths = d.widths.clone();
            let by = d.moved();
            widths[d.k - 1] += by;
            widths[d.k] -= by;
            if by != 0.0 && d.para < self.paras.len() {
                let p = self.paras[d.para];
                doc.set_columns(p.start..p.start + p.len, widths);
                self.relayout();
            }
            return changed;
        }
        // A right-click outside the selection puts the caret there first,
        // so the menu is about what was clicked (a misspelled word's
        // suggestions).
        if ui.input(|i| i.pointer.secondary_pressed())
            && response.hovered()
            && let Some(pos) = latest
        {
            let (at, next) = self.byte_at(ui, doc, o, inner, content(pos));
            if !(self.selection().start..=self.selection().end).contains(&at) || self.selection().is_empty() {
                self.anchor = at;
                self.caret = at;
                self.prefer_next = next;
                self.h_pos = None;
            }
            ctx.memory_mut(|mem| mem.request_focus(id));
        }
        if pressed
            && response.hovered()
            && let Some(pos) = latest
        {
            let had = ctx.memory(|mem| mem.has_focus(id));
            ctx.memory_mut(|mem| mem.request_focus(id));
            let (at, next) = self.byte_at(ui, doc, o, inner, content(pos));
            let sel = self.selection();
            if had && !mods.shift && !mods.alt && !sel.is_empty() && sel.contains(&at) {
                self.gesture = Gesture::Armed { press: pos, at };
            } else {
                if !mods.shift {
                    self.anchor = at;
                }
                self.caret = at;
                self.prefer_next = next;
                self.h_pos = None;
                self.gesture = Gesture::Selecting;
            }
            self.touched = now;
        }
        match self.gesture {
            Gesture::Selecting if down => {
                if let Some(pos) = latest
                    && response.is_pointer_button_down_on()
                {
                    // Held beyond the view: it scrolls that way, the
                    // caret following along its edge.
                    let c = content(pos);
                    let past = c - c.clamp(vp.min, vp.max);
                    let past = if o.wrap { vec2(0.0, past.y) } else { past };
                    let by = ScrollView::edge_scroll(&ctx, past);
                    self.scroll_by += by;
                    let edge = vp.translate(by).shrink(0.5);
                    let (at, next) = self.byte_at(ui, doc, o, inner, c.clamp(edge.min, edge.max));
                    if at != self.caret {
                        self.caret = at;
                        self.prefer_next = next;
                        self.touched = now;
                    }
                }
            }
            Gesture::Armed { press, at } => {
                if !down {
                    // A click on the selection: the caret goes there.
                    self.anchor = at;
                    self.caret = at;
                    self.gesture = Gesture::None;
                } else if latest.is_some_and(|p| p.distance(press) > text_drag::SLOP) {
                    let range = self.selection();
                    ctx.data_mut(|d| d.insert_temp(text_drag::source_id(), Some(text_drag::Source { id, range: range.clone() })));
                    dnd::start(&ctx, text_drag::drag_data(&ctx, &doc.text()[range]));
                    self.gesture = Gesture::None;
                }
                ctx.request_repaint();
            }
            _ if !down => self.gesture = Gesture::None,
            _ => {}
        }
        if response.double_clicked() || response.triple_clicked() {
            let text = doc.text();
            let at = self.caret.min(text.len());
            let r = if response.triple_clicked() {
                let p = self.paras[self.para_of(at)];
                p.start..(p.start + p.len + 1).min(text.len())
            } else {
                word_at(text, at)
            };
            self.anchor = r.start;
            self.caret = r.end;
            self.touched = now;
        }

        // The keyboard.
        let focused = ctx.memory(|mem| mem.has_focus(id));
        if !focused {
            return changed;
        }
        ctx.memory_mut(|mem| {
            mem.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false })
        });
        let events = ui.input(|i| i.events.clone());
        for e in &events {
            let before = (self.anchor, self.caret);
            match e {
                egui::Event::Text(t) if o.editable && !t.is_empty() && t != "\n" && t != "\r" => {
                    let sel = self.selection();
                    self.edit(doc, sel, t);
                    changed = true;
                }
                egui::Event::Paste(t) if o.editable && !t.is_empty() => {
                    let sel = self.selection();
                    self.edit(doc, sel, &t.replace("\r\n", "\n"));
                    changed = true;
                }
                egui::Event::Copy | egui::Event::Cut => {
                    let sel = self.selection();
                    if !sel.is_empty() {
                        doc.copied(sel.clone());
                        ctx.copy_text(doc.text()[sel.clone()].to_owned());
                        if matches!(e, egui::Event::Cut) && o.editable {
                            self.edit(doc, sel, "");
                            changed = true;
                        }
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    changed |= self.key(ui, doc, o, inner, vp, *key, *modifiers);
                }
                _ => {}
            }
            if (self.anchor, self.caret) != before {
                self.touched = now;
            }
        }
        changed
    }

    #[allow(clippy::too_many_arguments)]
    fn key(&mut self, ui: &Ui, doc: &mut dyn Document, o: &Options, inner: f32, vp: Rect, key: egui::Key, mods: egui::Modifiers) -> bool {
        use egui::Key as K;
        let text_len = doc.text().len();
        let sel = self.selection();
        let word = mods.alt || mods.ctrl;
        let to: Option<(usize, bool)>;
        let mut keep_h = false;
        match key {
            K::Enter if o.editable => {
                // In a table, a row below.
                if let Some((i, cells, _)) = self.cell(doc.text()) {
                    return self.add_row(doc, i, cells.len());
                }
                self.edit(doc, sel, "\n");
                return true;
            }
            K::Tab if !mods.command && self.cell(doc.text()).is_some() => {
                return if o.editable { self.tab(doc, mods.shift).unwrap_or(false) } else { false };
            }
            K::Tab if o.editable && !mods.shift && !mods.command => {
                self.edit(doc, sel, "\t");
                return true;
            }
            K::Backspace | K::Delete if o.editable => {
                let r = if !sel.is_empty() {
                    sel
                } else {
                    let t = doc.text();
                    match (key == K::Backspace, word) {
                        (true, true) => prev_word(t, self.caret)..self.caret,
                        (true, false) => prev_char(t, self.caret)..self.caret,
                        (false, true) => self.caret..next_word(t, self.caret),
                        (false, false) => self.caret..next_char(t, self.caret),
                    }
                };
                if r.is_empty() {
                    return false;
                }
                self.edit(doc, r, "");
                return true;
            }
            K::A if mods.command => {
                self.anchor = 0;
                self.caret = text_len;
                return false;
            }
            K::ArrowLeft | K::ArrowRight if !sel.is_empty() && !mods.shift && !mods.alt && !mods.ctrl => {
                let at = if key == K::ArrowLeft { sel.start } else { sel.end };
                to = Some((at, false));
            }
            K::ArrowLeft => {
                let t = doc.text();
                to = Some((if word { prev_word(t, self.caret) } else { prev_char(t, self.caret) }, true));
            }
            K::ArrowRight => {
                let t = doc.text();
                to = Some((if word { next_word(t, self.caret) } else { next_char(t, self.caret) }, false));
            }
            K::ArrowUp | K::ArrowDown if mods.command => to = Some((if key == K::ArrowUp { 0 } else { text_len }, false)),
            K::ArrowUp | K::ArrowDown => {
                to = Some(self.vertical(ui, doc, inner, key == K::ArrowDown));
                keep_h = true;
            }
            K::Home | K::End if mods.ctrl => to = Some((if key == K::Home { 0 } else { text_len }, false)),
            K::Home | K::End => to = Some(self.row_end(ui, doc, key == K::End)),
            K::PageUp | K::PageDown => {
                // A view's height up or down, the view going along.
                let r = self.caret_rect(ui, doc, o, inner, self.caret, self.prefer_next);
                let h = *self.h_pos.get_or_insert(r.center().x - o.margin.x);
                let dy = if key == K::PageUp { -vp.height() } else { vp.height() };
                let target = Pos2::new(o.margin.x + h, r.center().y + dy);
                to = Some(self.byte_at(ui, doc, o, inner, target));
                keep_h = true;
                self.scroll_by.y += dy;
            }
            _ => return false,
        }
        if let Some((at, next)) = to {
            if !mods.shift {
                self.anchor = at;
            }
            self.caret = at;
            self.prefer_next = next;
            if !keep_h {
                self.h_pos = None;
            }
            self.reveal = self.scroll_by == Vec2::ZERO;
        }
        false
    }
}

/// The cells of the row at bytes `start..start + len` of `text`, as bytes
/// of the text.
fn cells_of(text: &str, start: usize, len: usize) -> Vec<Range<usize>> {
    cells(&text[start..start + len]).into_iter().map(|(s, l)| start + s..start + s + l).collect()
}

/// Paragraphs of `text[start..end]` (which starts one): their starts and
/// lengths, without newlines.
fn split(text: &str, start: usize, end: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut from = start;
    let mut done = false;
    let mut breaks = text[start..end].match_indices('\n').map(move |(k, _)| start + k);
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        match breaks.next() {
            Some(nl) => {
                let p = (from, nl - from);
                from = nl + 1;
                Some(p)
            }
            None => {
                done = true;
                Some((from, end - from))
            }
        }
    })
}

/// What changed from `old` to `new`: the bytes of `old` that went, and
/// how many bytes came in their place (what is the same at either end
/// stays).
/// A wavy line under bytes `r` of paragraph text `para` (laid out as
/// `g`, drawn at `at`), a device pixel thick, on device pixels.
fn squiggle(painter: &egui::Painter, g: &Galley, at: Pos2, para: &str, r: Range<usize>, ppp: f32, color: egui::Color32) {
    let (a, b) = (para[..r.start].chars().count(), para[..r.end].chars().count());
    let px = 1.0 / ppp;
    let mut c0 = 0;
    for row in &g.rows {
        let n = row.char_count_excluding_newline().0;
        if a < c0 + n && b > c0 {
            let left = row.x_offset(CharIndex(a.max(c0) - c0));
            let right = row.x_offset(CharIndex(b.min(c0 + n) - c0));
            let snap = |v: f32| (v * ppp).round() / ppp;
            let base = snap(at.y + row.pos.y + row.size.y) - 2.0 * px;
            let x0 = snap(at.x + row.pos.x + left);
            let x1 = snap(at.x + row.pos.x + right);
            // Up and down every two device pixels.
            let step = 2.0 * px;
            let mut points = Vec::new();
            let mut x = x0;
            let mut k = 0;
            while x <= x1 {
                points.push(Pos2::new(x, base + if k % 2 == 0 { 0.5 * px } else { -1.5 * px } + px));
                x += step;
                k += 1;
            }
            if points.len() >= 2 {
                painter.add(egui::Shape::line(points, egui::Stroke::new(px, color)));
            }
        }
        c0 += n;
    }
}

pub fn difference(old: &str, new: &str) -> (Range<usize>, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut p = common(a.chunks(256).zip(b.chunks(256)), |(x, y)| x == y) * 256;
    p += a[p.min(a.len())..].iter().zip(&b[p.min(b.len())..]).take_while(|(x, y)| x == y).count();
    p = floor(old, p.min(old.len())).min(floor(new, p.min(new.len())));
    let max = (a.len() - p).min(b.len() - p);
    let mut s = 0;
    while s < max && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    while s > 0 && (!old.is_char_boundary(a.len() - s) || !new.is_char_boundary(b.len() - s)) {
        s -= 1;
    }
    (p..a.len() - s, b.len() - s - p)
}

fn common<T>(it: impl Iterator<Item = T>, same: impl Fn(&T) -> bool) -> usize {
    it.take_while(same).count()
}

/// Back `i` off to a char boundary of `s`.
fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn byte_of_col(para: &str, col: usize) -> usize {
    para.char_indices().nth(col).map_or(para.len(), |(b, _)| b)
}

fn prev_char(t: &str, at: usize) -> usize {
    t[..at].chars().next_back().map_or(0, |c| at - c.len_utf8())
}

fn next_char(t: &str, at: usize) -> usize {
    t[at..].chars().next().map_or(t.len(), |c| at + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the word before `at` (over any space or punctuation first).
fn prev_word(t: &str, at: usize) -> usize {
    let mut it = t[..at].char_indices().rev().peekable();
    while it.next_if(|(_, c)| !is_word(*c)).is_some() {}
    let mut start = it.peek().map_or(0, |(i, c)| i + c.len_utf8());
    while let Some((i, _)) = it.next_if(|(_, c)| is_word(*c)) {
        start = i;
    }
    start
}

/// The end of the word after `at` (over any space or punctuation first).
fn next_word(t: &str, at: usize) -> usize {
    let mut it = t[at..].char_indices().peekable();
    while it.next_if(|(_, c)| !is_word(*c)).is_some() {}
    while it.next_if(|(_, c)| is_word(*c)).is_some() {}
    it.peek().map_or(t.len(), |(i, _)| at + i)
}

/// The word (or run of spaces, or the one other character) at `at`.
fn word_at(t: &str, at: usize) -> Range<usize> {
    let c = t[at..].chars().next().or_else(|| t[..at].chars().next_back());
    let Some(c) = c else { return at..at };
    let class = |x: char| if is_word(x) { 0 } else if x.is_whitespace() && x != '\n' { 1 } else { 2 };
    let k = class(c);
    if k == 2 {
        let start = if t[at..].starts_with(c) { at } else { at - c.len_utf8() };
        return start..start + c.len_utf8();
    }
    let start = t[..at].char_indices().rev().take_while(|(_, x)| class(*x) == k).last().map_or(at, |(i, _)| i);
    let end = t[at..].char_indices().find(|(_, x)| class(*x) != k).map_or(t.len(), |(i, _)| at + i);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_split_and_follow_edits() {
        let mut e = Editor::new("t");
        let text = "one\ntwo\n\nfour";
        e.reset(text);
        let starts = |e: &Editor| e.paras.iter().map(|p| (p.start, p.len)).collect::<Vec<_>>();
        assert_eq!(starts(&e), [(0, 3), (4, 3), (8, 0), (9, 4)]);
        // "two" → "t\nwo": a paragraph more, the rest moved on by two.
        let new = "one\nt\nwo\n\nfour";
        e.changed(text, new);
        assert_eq!(starts(&e), [(0, 3), (4, 1), (6, 2), (9, 0), (10, 4)]);
        // Joined back up across the break.
        e.changed(new, "onefour");
        assert_eq!(starts(&e), [(0, 7)]);
        e.reset("");
        assert_eq!(starts(&e), [(0, 0)]);
    }

    #[test]
    fn rows_cells_and_columns() {
        assert_eq!(cells("a\tbc\t"), [(0, 1), (2, 2), (5, 0)]);
        assert_eq!(cells(""), [(0, 0)]);
        // Equal shares unless there is one a cell; whole points that add up.
        let row = Row { columns: vec![1.0, 3.0], header: false };
        assert_eq!(cell_columns(&row, 2, 400.0), [(0.0, 100.0), (100.0, 300.0)]);
        assert_eq!(cell_columns(&row, 3, 100.0), [(0.0, 33.0), (33.0, 34.0), (67.0, 33.0)]);
        assert_eq!(cells_of("x\na\tb", 2, 3), [2..3, 4..5]);
    }

    #[test]
    fn differences() {
        assert_eq!(difference("hello world", "hello brave world"), (6..6, 6));
        assert_eq!(difference("abc", "abc"), (3..3, 0));
        assert_eq!(difference("héllo", "hèllo"), (1..3, 2));
        let long: String = "x".repeat(1000);
        assert_eq!(difference(&long, &format!("{long}y")), (1000..1000, 1));
    }

    #[test]
    fn words() {
        let t = "foo.bar  baz";
        assert_eq!(next_word(t, 0), 3);
        assert_eq!(next_word(t, 3), 7);
        assert_eq!(prev_word(t, 12), 9);
        assert_eq!(prev_word(t, 9), 4);
        assert_eq!(prev_word(t, 4), 0);
        assert_eq!(word_at(t, 5), 4..7);
        assert_eq!(word_at(t, 7), 7..9);
        assert_eq!(word_at(t, 3), 3..4);
        assert_eq!(word_at(t, 12), 9..12);
        assert_eq!(prev_char("hé", 3), 1);
        assert_eq!(next_char("hé", 1), 3);
    }
}
