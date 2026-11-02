//! A text editor for documents of any length, on Olo's GPUI: BeOS's
//! BTextView, as StyledEdit's window has it (the egui kit's `editor`,
//! moved). Each paragraph is laid out on its own ([`layout`]) and only
//! while it shows; a layout is kept while its paragraph, styles and width
//! stay the same. Paragraphs out of sight keep an estimated height; those
//! within a screen of the view are measured before they come into it (a
//! millisecond's worth a frame), and the view stays on the paragraph at
//! its top while heights above it settle, so nothing that shows ever
//! jumps. The rest are never laid out at all.
//!
//! The text and its styles are the app's [`Document`]: the editor reads
//! them a paragraph at a time and makes every edit through
//! [`Document::replace`], so the app's undo and styles follow. Changes the
//! app makes itself (undo, Replace All, loading) it reports with
//! [`Editor::changed`], [`Editor::replaced`] or [`Editor::reset`]; new
//! styles need nothing (a paragraph laid out differently is measured
//! again). Positions are bytes of the text.
//!
//! The app's view owns the [`Editor`] and its document and lets the
//! editor at them ([`EditorHost`]); [`view`] is the element that shows
//! it, in a BeOS scroll view whose bars it drives itself. Keys: arrows,
//! ⌥ or ⌘ with ←/→ by word, ⌘↑/↓ and ⌘Home/End to either end, Home/End
//! along the row, Page Up/Down; Shift extends. Typing comes through the
//! input method (composing shows underlined).
//!
//! A paragraph can be a table's row ([`Paragraph::row`]): its text is its
//! cells, a tab between each, each laid out in its column and ruled; the
//! rows of a table are paragraphs one after another. Tab and ⇧Tab go from
//! cell to cell (Tab in the last cell adds a row), Return adds a row
//! below, and edits keep a table's tabs and newlines: what is typed or
//! pasted into a cell is one line, and deleting across cells or rows
//! empties them rather than joining them. The rule between two columns
//! drags to size them.
//!
//! A picture is a character in the text (U+FFFC, [`PICTURE`]) that the
//! document gives a picture's size ([`Span::picture`]) and names in
//! [`Paragraph::pictures`]; the editor draws it there (a frame while it
//! isn't loaded), tinted when selected. Typed around, deleted, copied and
//! dragged, it is a character like any other.
//!
//! A press on the selection drags its text: let go in the editor, it
//! moves there; taken out of the window, it goes as text to wherever it's
//! dropped (and is cut, if that moved it).

mod layout;

use std::{
    any::TypeId,
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Element, ElementId, Entity, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, InteractiveElement as _, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, Style, Styled as _, Task, UTF16Selection, Window, actions, div, point, px, relative, size,
};
pub use layout::{Align, Kind, Laid, Metrics, Params, Shaper, Span, TabAlign, TabStop, TabStops, Unit, lay_out, line as lay_out_line};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme, widgets::ScrollView};

/// The text an [`Editor`] shows and edits, with its styles.
pub trait Document {
    fn text(&self) -> &str;
    /// The styles of bytes `range` (one paragraph, without its newline),
    /// in order, covering it. An empty paragraph needs one, empty, for
    /// its height.
    fn spans(&mut self, range: Range<usize>, out: &mut Vec<Span>);
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
    /// What the clipboard holds was pasted at `selection`: the text to put
    /// in (None: the document took it itself, a picture).
    fn paste(&mut self, item: &ClipboardItem, _selection: Range<usize>) -> Option<String> {
        item.text()
    }
    /// A key the document takes before the editor (a list's Tab), with
    /// the selection; whether it did.
    fn key(&mut self, _key: EditKey, _selection: Range<usize>) -> bool {
        false
    }
}

/// Keys a [`Document`] may take first ([`Document::key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Enter,
    Tab,
    BackTab,
    Backspace,
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
    /// Its pictures: each one's character ([`PICTURE`]), in bytes from the
    /// paragraph's start, and how it is drawn there.
    pub pictures: Vec<(usize, Picture)>,
}

/// The character a picture is in the text.
pub const PICTURE: char = '\u{FFFC}';

/// A picture as the editor draws it ([`Paragraph::pictures`]).
#[derive(Clone, Debug)]
pub struct Picture {
    /// Its pixels; None while it's loading (or couldn't be): a frame.
    pub image: Option<Arc<RenderImage>>,
    /// Points across and down.
    pub size: (f32, f32),
}

impl PartialEq for Picture {
    fn eq(&self, other: &Picture) -> bool {
        self.size == other.size
            && match (&self.image, &other.image) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Paragraph {
    /// Its tab stops for its layout, which starts at its indent; `space`
    /// is a space's width in its first font.
    pub fn layout_tabs(&self, space: f32) -> TabStops {
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
pub const CELL_INSET: (f32, f32) = (6.0, 3.0);
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Typing changes it (else it can be read, selected and copied).
    pub editable: bool,
    /// Rows wrap at the view's width (else it scrolls sideways).
    pub wrap: bool,
    pub align: Align,
    /// Wrapped rows fill the width (all but each paragraph's last).
    pub justify: bool,
    /// Between the view's edges and the text.
    pub margin: (f32, f32),
    pub background: Option<olo_theme::Rgba>,
    /// Misspelled words are underlined ([`crate::spell`]), all but the
    /// one being typed.
    pub spelling: bool,
    /// The text set on pages, as it prints (else in one column the view's
    /// width).
    pub pages: Option<Pages>,
}

impl Default for Options {
    fn default() -> Self {
        Options { editable: true, wrap: true, align: Align::Left, justify: false, margin: (m::TEXT_INSET, 4.0), background: None, spelling: false, pages: None }
    }
}

/// Text set on pages ([`Options::pages`]): sheets of paper down a desk,
/// the text within each one's margins, a page's text going on to the
/// next's. In points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pages {
    pub paper: (f32, f32),
    /// On every side.
    pub margin: f32,
    /// Between sheets, and around them.
    pub gap: f32,
}

impl Pages {
    fn text_w(&self) -> f32 {
        (self.paper.0 - 2.0 * self.margin).max(1.0)
    }
    fn text_h(&self) -> f32 {
        (self.paper.1 - 2.0 * self.margin).max(1.0)
    }
    /// From a page's text to the next's (down the text, where page 0's
    /// starts at 0).
    fn pitch(&self) -> f32 {
        self.paper.1 + self.gap
    }
    /// The page that `y` (down the text) is on (or in the gap after).
    fn page_of(&self, y: f32) -> usize {
        (y / self.pitch()).floor().max(0.0) as usize
    }
    fn top(&self, k: usize) -> f32 {
        k as f32 * self.pitch()
    }
    fn bottom(&self, k: usize) -> f32 {
        self.top(k) + self.text_h()
    }
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
    /// The layout it was measured from (its key), `NEVER` or `STALE`.
    key: u64,
    /// How it was set when last laid out.
    set: Set,
}

/// What of a [`Paragraph`] places its layout: indents, the space above
/// (and below, in its height).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Set {
    left: f32,
    right: f32,
    above: f32,
    below: f32,
    /// A table's row: its columns and header, hashed.
    row: Option<u64>,
    /// Set on pages: where on its page it starts (half points), which its
    /// rows' breaks depend on.
    page_at: i32,
}

/// A paragraph laid out: one layout, or one a cell of a table's row.
#[derive(Clone)]
struct Placed {
    parts: Vec<Part>,
    /// A row's height (its cells' rules) and whether it is a header.
    row: Option<(f32, bool)>,
    /// Its pictures ([`Paragraph::pictures`]).
    pictures: Vec<(usize, Picture)>,
    /// A list's marker, and the gap before the text.
    marker: Option<Rc<Laid>>,
    /// What its layouts were made from, together.
    key: u64,
}

/// A layout of a [`Placed`] paragraph: where it is from the paragraph's
/// origin, the bytes it holds (from the paragraph's start), and its
/// cell's left edge and width.
#[derive(Clone)]
struct Part {
    laid: Rc<Laid>,
    at: (f32, f32),
    start: usize,
    len: usize,
    frame: (f32, f32),
}

impl Placed {
    /// The part (cell) at `x` across the paragraph.
    fn part_at_x(&self, x: f32) -> usize {
        self.parts.iter().rposition(|q| q.frame.0 <= x).unwrap_or(0)
    }

    /// The part holding byte `rel` of the paragraph (a cell's end before
    /// the next's start).
    fn part_of(&self, rel: usize) -> usize {
        self.parts.iter().position(|q| rel <= q.start + q.len).unwrap_or(self.parts.len() - 1)
    }

    fn height(&self) -> f32 {
        self.row.map_or_else(|| self.parts[0].laid.height(), |(h, _)| h)
    }
}

/// Never laid out: its size is a guess from its length.
const NEVER: u64 = 0;
/// Laid out, but not as it is now (restyled): its size is a good guess.
const STALE: u64 = 1;

/// How long a frame spends measuring paragraphs about to come into view.
const MEASURE_BUDGET: Duration = Duration::from_micros(1000);
/// How long the caret shows and hides, and how long it blinks after the
/// last keystroke or click (then it stays: an idle window draws nothing).
const BLINK: Duration = Duration::from_millis(530);
const BLINK_FOR: Duration = Duration::from_secs(10);
/// How far a press moves before it drags the selection's text.
const SLOP: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    None,
    /// The primary button went down on the text: dragging selects.
    Selecting,
    /// A press on the selection, not yet a drag of its text or a click.
    Armed { press: Point<Pixels>, at: usize },
    /// The selection's text being dragged within the view: where it would
    /// go (a caret there).
    Moving { to: Option<usize> },
}

/// How layouts are made: the wrap width (None: no wrapping), alignment
/// and justification.
type Look = (Option<f32>, Align, bool);

/// Layouts kept while used: this frame's and the last's.
#[derive(Default)]
struct Kept {
    now: HashMap<u64, Rc<Laid>>,
    before: HashMap<u64, Rc<Laid>>,
}

impl Kept {
    fn get(&mut self, key: u64, make: impl FnOnce() -> Laid) -> Rc<Laid> {
        if let Some(l) = self.now.get(&key) {
            return l.clone();
        }
        let l = self.before.remove(&key).unwrap_or_else(|| Rc::new(make()));
        self.now.insert(key, l.clone());
        l
    }

    /// Kept another frame (used, though not looked up).
    fn touch(&mut self, key: u64) {
        if !self.now.contains_key(&key)
            && let Some(l) = self.before.remove(&key)
        {
            self.now.insert(key, l);
        }
    }

    /// A frame is done: what it didn't use goes.
    fn roll(&mut self) {
        self.before = std::mem::take(&mut self.now);
    }
}

/// What the last frame showed, for the pointer and painting.
#[derive(Default)]
struct Shown {
    paras: Vec<(usize, Placed)>,
    /// The view on the window, and where content point (0, 0) was on it.
    view: Bounds<Pixels>,
    origin: Point<Pixels>,
    sheets: Vec<(usize, Bounds<Pixels>)>,
    o: Options,
}

pub struct Editor {
    focus: FocusHandle,
    scroll: ScrollHandle,
    paras: Vec<Para>,
    /// Where each paragraph starts down the text (and the total, last).
    tops: Vec<f32>,
    tops_dirty: bool,
    /// The widest paragraph, unwrapped.
    widest: f32,
    /// The text's width in the view (a table's, when rows don't wrap).
    width: f32,
    look: Option<Look>,
    /// Bumped whenever every layout changes.
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
    /// Scroll this far this frame (Page Up and Down).
    scroll_by: (f32, f32),
    gesture: Gesture,
    /// Where the pointer is while a press selects (to scroll on past an
    /// edge).
    pointer: Option<Point<Pixels>>,
    /// The paragraph at the top of the view (by its first byte) and how
    /// far down into it the view starts.
    top: (usize, f32),
    /// Where the view goes on the next frame ([`Editor::set_view`]).
    set_top: Option<(usize, f32)>,
    /// How far the view is scrolled (content points), and what the scroll
    /// bars were told it was (a bar's drag changes theirs).
    offset: (f32, f32),
    told: Point<Pixels>,
    /// Misspelled words (bytes) of paragraphs checked, by their text's
    /// hash: a paragraph is checked once until it changes.
    misspelled: HashMap<u64, Arc<Vec<Range<usize>>>>,
    /// Paragraphs being checked on a worker (by their text's hash), and
    /// what it found, to take in (checked as of `spell_gen`).
    spell_pending: std::collections::HashSet<u64>,
    spell_done: Arc<std::sync::Mutex<Vec<(u64, u64, Arc<Vec<Range<usize>>>)>>>,
    /// Bumped when the words known change: what was checked before is
    /// stale.
    spell_gen: u64,
    /// Tables' inner rules drawn last frame (where in the window, the row,
    /// which rule, its columns' widths), to take hold of.
    rules: Vec<(Bounds<Pixels>, usize, usize, Vec<f32>)>,
    column_drag: Option<ColumnDrag>,
    /// Where the text's left edge was in the window when last shown
    /// (scrolled sideways too), and how wide it was ([`Editor::text_span`]).
    span: Option<(f32, f32)>,
    pages: Option<Pages>,
    /// A dark document's ground: its text's own colours lightened on it
    /// to read (None: on a light one, as they print).
    paper: Option<olo_theme::Rgba>,
    /// Colours as they were made to read on `paper`.
    legible: Vec<(Hsla, Hsla)>,
    kept: Kept,
    /// Paragraphs shaped (not placed), by their text and styles: a new
    /// width (a window resized) places them again without shaping them.
    shapes: Kept,
    spans: Vec<Span>,
    shown: Shown,
    /// What an input method is composing.
    marked: Option<Range<usize>>,
    /// The text as agents read it, kept until it changes (a long
    /// document's copy each frame cost a share of the frame).
    agent_text: Option<SharedString>,
    /// Paragraphs about to show were left to measure (the frame's budget
    /// ran out): another frame does more.
    more: bool,
    /// Its ends' stretch (fingers past one, a glide bouncing off it), and
    /// whether it's springing back (drawn again next frame).
    elastic: gpui::Elastic,
    springing: bool,
    caret_on: bool,
    blink: Option<Task<()>>,
}

/// An app view with editors in it: lends the editor `key` and its
/// document for the editor to work on.
pub trait EditorHost: Sized + 'static {
    /// Calls `f` with editor `key` and its document.
    fn editor(&mut self, key: usize, cx: &mut Context<Self>, f: &mut dyn FnMut(&mut Editor, &mut dyn Document, &mut Context<Self>));
    /// Its text changed by typing, pasting, cutting or a drop.
    fn edited(&mut self, _key: usize, _cx: &mut Context<Self>) {}
    /// Where its text is across the window ([`Editor::text_span`]) moved
    /// as it was laid out for this frame, before anything is painted (for
    /// a ruler over it, which follows in the same frame).
    fn text_moved(&mut self, _key: usize, _span: (f32, f32), _cx: &mut Context<Self>) {}
    /// The right button was pressed on it (the caret is there first, if
    /// it was outside the selection): Cut, Copy, Paste and Select All,
    /// unless the app has a menu of its own.
    fn context_menu(&mut self, key: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        edit_menu(self, key, at, window, cx);
    }
}

/// The editor's own right-button menu (an app's may start with it).
pub fn edit_menu<V: EditorHost>(host: &mut V, key: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<V>) {
    let (mut has_sel, mut editable) = (false, false);
    host.editor(key, cx, &mut |ed, _, _| {
        has_sel = !ed.selection().is_empty();
        editable = ed.shown.o.editable;
    });
    let menu = crate::Menu::new("Edit")
        .item("Cut", "cut")
        .enabled(editable && has_sel)
        .keys("⌘X")
        .item("Copy", "copy")
        .enabled(has_sel)
        .keys("⌘C")
        .item("Paste", "paste")
        .enabled(editable)
        .keys("⌘V")
        .separator()
        .item("Select All", "select-all")
        .keys("⌘A");
    let me = cx.entity();
    crate::menu::context_menu(window, cx, menu, at, move |cmd, window, cx| {
        let k = match cmd {
            "cut" => Cmd::Cut,
            "copy" => Cmd::Copy,
            "paste" => Cmd::Paste,
            "select-all" => Cmd::SelectAll,
            _ => return,
        };
        run(&me, key, k, window, cx);
    });
}

impl Editor {
    pub fn new(cx: &mut App) -> Editor {
        let mut e = Editor {
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            paras: Vec::new(),
            tops: Vec::new(),
            tops_dirty: true,
            widest: 0.0,
            width: 400.0,
            look: None,
            key: 2,
            calibrated: false,
            row_h: 16.0,
            char_w: 7.0,
            anchor: 0,
            caret: 0,
            prefer_next: false,
            h_pos: None,
            reveal: false,
            scroll_by: (0.0, 0.0),
            gesture: Gesture::None,
            pointer: None,
            top: (0, 0.0),
            set_top: None,
            offset: (0.0, 0.0),
            told: Point::default(),
            misspelled: HashMap::new(),
            spell_pending: Default::default(),
            spell_done: Default::default(),
            spell_gen: 0,
            rules: Vec::new(),
            column_drag: None,
            span: None,
            pages: None,
            paper: None,
            legible: Vec::new(),
            kept: Kept::default(),
            shapes: Kept::default(),
            spans: Vec::new(),
            shown: Shown::default(),
            marked: None,
            agent_text: None,
            more: false,
            elastic: gpui::Elastic::default(),
            springing: false,
            caret_on: true,
            blink: None,
        };
        e.reset("");
        e
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Gives it the keyboard.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Check the spelling again (a word was learnt or ignored).
    pub fn respell(&mut self) {
        self.misspelled.clear();
        self.spell_pending.clear();
        self.spell_gen += 1;
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
        self.offset = (0.0, 0.0);
        self.gesture = Gesture::None;
        self.marked = None;
        self.agent_text = None;
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
        self.marked = None;
        self.agent_text = None;
    }

    /// Bytes `range` have new styles. (Nothing needs doing: a paragraph
    /// laid out differently is measured again. Kept for the app's sake.)
    pub fn restyled(&mut self, range: Range<usize>) {
        let (a, b) = (self.para_of(range.start), self.para_of(range.end));
        for p in &mut self.paras[a..=b] {
            p.key = p.key.min(STALE);
        }
    }

    /// Everything is laid out again (fonts loaded, the alignment changed).
    pub fn relayout(&mut self) {
        self.key += 1;
        for p in &mut self.paras {
            p.key = p.key.min(STALE);
        }
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

    /// Where the text's left edge was in the window when last shown
    /// (scrolled sideways too), and its width (the wrap width, or the
    /// view's): for a ruler over it.
    pub fn text_span(&self) -> Option<(f32, f32)> {
        self.span
    }

    /// Set on pages, the sheets showing when last drawn (each page, from
    /// 0, and where it is in the window) and how many pages there are:
    /// for the app's header and footer on them.
    pub fn sheets(&self) -> (&[(usize, Bounds<Pixels>)], usize) {
        (&self.shown.sheets, self.page_count())
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
        let rows = match self.look.and_then(|l| l.0) {
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
            // On pages: one whose first row (or a table's row, or one not
            // yet laid out, whole) doesn't fit at a page's foot starts the
            // next page (a longer one's other rows go on as they come).
            if let Some(pg) = self.pages {
                let k = pg.page_of(y);
                let body = p.height - p.set.above - p.set.below;
                let first = p.set.above + if p.set.row.is_some() || p.key == NEVER { body } else { p.row_h.min(body) };
                if y > pg.top(k) + 0.5 && y + first > pg.bottom(k) + 0.5 {
                    y = pg.top(k + 1);
                }
            }
            self.tops.push(y);
            y += p.height;
            widest = widest.max(p.natural);
        }
        self.tops.push(y);
        self.widest = widest;
        self.tops_dirty = false;
    }

    /// How many pages the text is on (set on pages; else 1).
    fn page_count(&self) -> usize {
        match (self.pages, self.tops.last()) {
            (Some(pg), Some(&end)) => pg.page_of((end - 0.5).max(0.0)) + 1,
            _ => 1,
        }
    }

    /// The paragraph at `y` down the text (clamped to the first and last).
    fn para_at_y(&self, y: f32) -> usize {
        self.tops[1..].partition_point(|&t| t <= y).min(self.paras.len() - 1)
    }

    /// New layout parameters: every paragraph's size is a guess until it
    /// is measured again (from what it measured before).
    fn set_look(&mut self, look: Look) {
        if self.look == Some(look) {
            return;
        }
        self.look = Some(look);
        self.key += 1;
        for i in 0..self.paras.len() {
            let h = self.estimate(&self.paras[i]);
            self.paras[i].height = h;
            self.paras[i].key = self.paras[i].key.min(STALE);
        }
        self.tops_dirty = true;
    }

    /// On a dark document, `spans`' own colours as they read on it: no
    /// darker than it takes for 4.5:1 (as the text's own colour is white
    /// there, not the black it prints in).
    fn make_legible(&mut self, spans: &mut [Span]) {
        let Some(bg) = self.paper else { return };
        for s in spans {
            let Some(c) = s.color else { continue };
            let made = match self.legible.iter().find(|(from, _)| *from == c) {
                Some((_, to)) => *to,
                None => {
                    let r = gpui::Rgba::from(c);
                    let b = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
                    let to = style::c(olo_theme::readable([b(r.r), b(r.g), b(r.b), 255], bg, 4.5)).opacity(c.a);
                    if self.legible.len() > 64 {
                        self.legible.clear();
                    }
                    self.legible.push((c, to));
                    to
                }
            };
            s.color = Some(made);
        }
    }

    /// Paragraph `i` laid out (kept while it stays the same), and
    /// measured if it's new: one layout, or one a cell of a table's row.
    fn placed(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, i: usize) -> Placed {
        let (wrap, align, justify) = self.look.unwrap_or((None, Align::Left, false));
        let p = self.paras[i];
        let range = p.start..p.start + p.len;
        let f = doc.paragraph(range.clone());
        let row_key = f.row.as_ref().map(|r| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            r.columns.iter().for_each(|c| c.to_bits().hash(&mut h));
            r.header.hash(&mut h);
            h.finish()
        });
        // (A table's rows meet: no space between them.)
        let (above, below) = if f.row.is_some() { (0.0, 0.0) } else { (f.above.max(0.0), f.below.max(0.0)) };
        // On pages, where on its page it starts.
        let top = self.tops.get(i).copied().unwrap_or(0.0);
        let page_at = self.pages.map_or(0, |pg| ((top - pg.top(pg.page_of(top))) * 2.0).round() as i32);
        let set = Set { left: f.left.max(0.0), right: f.right.max(0.0), above, below, row: row_key, page_at };
        let mut spans = std::mem::take(&mut self.spans);
        let pages = self.pages;
        let placed = match &f.row {
            None => {
                spans.clear();
                doc.spans(range.clone(), &mut spans);
                self.make_legible(&mut spans);
                cover(&mut spans, p.len);
                let text = &doc.text()[range.clone()];
                let space = space_width(sh, &spans[0]);
                let params = Params {
                    wrap: wrap.map(|w| (w - set.left - set.right).max(1.0)),
                    width: (wrap.unwrap_or(self.width.max(self.widest)) - set.left - set.right).max(1.0),
                    first: f.first.max(0.0),
                    align: f.align.unwrap_or(align),
                    justify: f.justify.unwrap_or(justify),
                    tabs: f.layout_tabs(space),
                };
                let mut h = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut h);
                hash_spans(&spans, &mut h);
                // (Shaped as it was, whatever it's set in.)
                let shape_key = h.clone().finish();
                self.shapes.touch(shape_key);
                hash_params(&params, &mut h);
                if let Some(pg) = pages {
                    (pg.paper.0.to_bits(), pg.paper.1.to_bits(), pg.margin.to_bits(), pg.gap.to_bits(), page_at, above.to_bits()).hash(&mut h);
                }
                let key = h.finish();
                let shapes = &mut self.shapes;
                let laid = self.kept.get(key, || {
                    let mut l = shapes.get(shape_key, || layout::shape(sh, text, &spans)).placed(text, &params);
                    // On pages: a row that doesn't fit at a page's foot goes
                    // to the next page's top, and the rows after it with it.
                    if let Some(pg) = pages {
                        let t = top + above;
                        l.break_rows(|y| {
                            let k = pg.page_of(t + y);
                            (pg.bottom(k) - t, pg.top(k + 1) - t)
                        });
                    }
                    l
                });
                let marker = f.marker.as_ref().filter(|m| !m.is_empty()).map(|m| {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    ("marker", m).hash(&mut h);
                    hash_spans(&spans[..1], &mut h);
                    self.kept.get(h.finish(), || lay_out_line(sh, m, &spans[0]))
                });
                Placed { parts: vec![Part { laid, at: (0.0, 0.0), start: 0, len: p.len, frame: (0.0, 0.0) }], row: None, pictures: f.pictures.clone(), marker, key }
            }
            Some(row) => {
                let width = (wrap.unwrap_or(self.width) - set.left - set.right).max(1.0);
                let text = doc.text()[range.clone()].to_owned();
                let cs = cells(&text);
                let columns = cell_columns(row, cs.len(), width);
                let halign = f.align.unwrap_or(Align::Left);
                let mut parts = Vec::with_capacity(cs.len());
                let mut all = std::collections::hash_map::DefaultHasher::new();
                for ((start, len), (x, w)) in cs.into_iter().zip(columns) {
                    let inner = (w - 2.0 * CELL_INSET.0).max(1.0);
                    spans.clear();
                    doc.spans(p.start + start..p.start + start + len, &mut spans);
                    self.make_legible(&mut spans);
                    cover(&mut spans, len);
                    let params = Params { wrap: Some(inner), width: inner, first: 0.0, align: halign, justify: f.justify.unwrap_or(false), tabs: TabStops::default() };
                    let cell = &text[start..start + len];
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    cell.hash(&mut h);
                    hash_spans(&spans, &mut h);
                    let shape_key = h.clone().finish();
                    self.shapes.touch(shape_key);
                    hash_params(&params, &mut h);
                    let key = h.finish();
                    key.hash(&mut all);
                    (x.to_bits(), w.to_bits()).hash(&mut all);
                    let shapes = &mut self.shapes;
                    let laid = self.kept.get(key, || shapes.get(shape_key, || layout::shape(sh, cell, &spans)).placed(cell, &params));
                    parts.push(Part { laid, at: (x + CELL_INSET.0, CELL_INSET.1), start, len, frame: (x, w) });
                }
                // A whole number of points high, the text in the middle of
                // what rounding leaves.
                let tallest = parts.iter().map(|q| q.laid.height()).fold(0.0, f32::max);
                let h = (tallest + 2.0 * CELL_INSET.1).round();
                for q in &mut parts {
                    q.at.1 = (h - tallest) / 2.0;
                }
                row.header.hash(&mut all);
                Placed { parts, row: Some((h, row.header)), pictures: f.pictures.clone(), marker: None, key: all.finish() | 2 }
            }
        };
        self.spans = spans;
        if set != p.set {
            self.paras[i].set = set;
            self.tops_dirty = true;
        }
        if p.key != placed.key.max(2) || set != p.set {
            self.measured(i, &placed);
        }
        placed
    }

    fn measured(&mut self, i: usize, placed: &Placed) {
        let wrapped = self.look.is_some_and(|l| l.0.is_some());
        let laid = &placed.parts[0].laid;
        let rows = laid.rows.len().max(1) as f32;
        let p = &mut self.paras[i];
        let first = p.key == NEVER;
        let body = placed.height();
        let h = body + p.set.above + p.set.below;
        if p.height != h {
            self.tops_dirty = true;
        }
        p.height = h;
        p.natural = match placed.row {
            // A row is as wide as its columns (one row high, as guessed).
            Some(_) => placed.parts.iter().map(|q| q.frame.1).sum::<f32>() + p.set.left + p.set.right,
            None if wrapped => laid.natural,
            None => laid.width() + p.set.left + p.set.right,
        };
        p.row_h = if placed.row.is_some() { body } else { laid.rows.iter().map(|r| r.height).sum::<f32>().max(1.0) / rows };
        p.key = placed.key.max(2);
        let p = *p;
        if first && p.len > 0 && placed.row.is_none() {
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

    /// Paragraph `i`'s origin in content coordinates.
    fn origin(&self, i: usize, o: &Options) -> (f32, f32) {
        (o.margin.0 + self.paras[i].set.left, o.margin.1 + self.tops[i] + self.paras[i].set.above)
    }

    /// The byte at content point `(x, y)` (and whether it is at the start
    /// of a wrapped row rather than the end of the one before).
    fn byte_at(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, o: &Options, (x, y): (f32, f32)) -> (usize, bool) {
        self.ensure_tops();
        let i = self.para_at_y(y - o.margin.1);
        let placed = self.placed(doc, sh, i);
        self.ensure_tops();
        let (ox, oy) = self.origin(i, o);
        let (lx, ly) = (x - ox, y - oy);
        let q = &placed.parts[placed.part_at_x(lx)];
        let (b, next) = q.laid.byte_at(lx - q.at.0, ly - q.at.1);
        let base = self.paras[i].start + q.start;
        (floor(doc.text(), base + b.min(q.len)), next)
    }

    /// Byte `at`'s paragraph, part (cell), and its row in that.
    fn cursor(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, at: usize, prefer_next: bool) -> (usize, Part, usize, usize) {
        let i = self.para_of(at);
        let placed = self.placed(doc, sh, i);
        let p = self.paras[i];
        let rel = at.clamp(p.start, p.start + p.len) - p.start;
        let q = placed.parts[placed.part_of(rel)].clone();
        let local = rel.max(q.start) - q.start;
        let row = q.laid.row_of(local, prefer_next);
        (i, q, local, row)
    }

    /// The caret's place (x, top, height) in content coordinates.
    fn caret_rect(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, o: &Options, at: usize, prefer_next: bool) -> (f32, f32, f32) {
        let (i, q, local, r) = self.cursor(doc, sh, at, prefer_next);
        self.ensure_tops();
        let (ox, oy) = self.origin(i, o);
        let row = &q.laid.rows[r];
        let x = q.laid.x_in_row(r, local);
        (ox + q.at.0 + x, oy + q.at.1 + row.top, row.height)
    }

    /// The byte a row up or down from the caret, keeping to `h_pos` (an
    /// x from the text's left, so paragraphs indented differently line up).
    fn vertical(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, down: bool) -> (usize, bool) {
        let (i, q, local, r) = self.cursor(doc, sh, self.caret, self.prefer_next);
        let here = q.laid.x_in_row(r, local) + self.paras[i].set.left + q.at.0;
        let h = *self.h_pos.get_or_insert(here);
        // The paragraph and part (a row's cell under `h`) the row is in.
        let beside = |e: &mut Editor, doc: &mut dyn Document, pi: usize| {
            let placed = e.placed(doc, sh, pi);
            placed.parts[placed.part_at_x(h - e.paras[pi].set.left)].clone()
        };
        let (pi, part, row) = if down && r + 1 < q.laid.rows.len() {
            (i, q, r + 1)
        } else if !down && r > 0 {
            (i, q, r - 1)
        } else if down && i + 1 < self.paras.len() {
            (i + 1, beside(self, doc, i + 1), 0)
        } else if !down && i > 0 {
            let part = beside(self, doc, i - 1);
            let last = part.laid.rows.len().saturating_sub(1);
            (i - 1, part, last)
        } else {
            // Past the first or last row: to the very start or end.
            let to = if down { doc.text().len() } else { 0 };
            return (to, false);
        };
        let base = self.paras[pi].start + part.start;
        let (b, next) = part.laid.byte_in_row(row, h - self.paras[pi].set.left - part.at.0);
        (base + b.min(part.len), next)
    }

    /// The start or end of the caret's row.
    fn row_end(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, end: bool) -> (usize, bool) {
        let (i, q, _, r) = self.cursor(doc, sh, self.caret, self.prefer_next);
        let base = self.paras[i].start + q.start;
        if end {
            // (Before a wrapped row's last space: the caret stays on it.)
            let mut b = q.laid.row_end(r);
            if r + 1 < q.laid.rows.len() {
                let t = &doc.text()[base..base + q.len];
                if t[..b].ends_with(' ') {
                    b -= 1;
                }
            }
            (base + b, false)
        } else {
            (base + q.laid.row_start(r), r > 0)
        }
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
        let cs: Vec<Range<usize>> = cells_of(text, p.start, p.len);
        let k = cs.iter().position(|c| self.caret <= c.end).unwrap_or(cs.len() - 1);
        Some((i, cs, k))
    }

    /// Tab (or ⇧Tab) in a table's row: the next (previous) cell, all of
    /// it selected; Tab in the last cell of a table's last row adds a row.
    fn tab(&mut self, doc: &mut dyn Document, back: bool) -> Option<bool> {
        let (i, cs, k) = self.cell(doc.text())?;
        let select = |e: &mut Editor, r: Range<usize>| {
            e.anchor = r.start;
            e.caret = r.end;
            e.h_pos = None;
            e.reveal = true;
        };
        if back {
            if k > 0 {
                select(self, cs[k - 1].clone());
            } else if i > 0 && self.paras[i - 1].set.row.is_some() {
                let q = self.paras[i - 1];
                let last = cells_of(doc.text(), q.start, q.len).pop().unwrap_or(q.start..q.start);
                select(self, last);
            }
            return Some(false);
        }
        if k + 1 < cs.len() {
            select(self, cs[k + 1].clone());
            return Some(false);
        }
        if i + 1 < self.paras.len() && self.paras[i + 1].set.row.is_some() {
            let q = self.paras[i + 1];
            let first = cells_of(doc.text(), q.start, q.len).swap_remove(0);
            select(self, first);
            return Some(false);
        }
        Some(self.add_row(doc, i, cs.len()))
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

    /// Works out what shows in `view` (the editor's box in the window):
    /// the view kept on its top paragraph (or moved to the caret), what's
    /// about to show measured, what shows laid out.
    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, o: &Options, view: Bounds<Pixels>, scale: f32) -> Option<f32> {
        let (vw, vh) = (f32::from(view.size.width), f32::from(view.size.height));
        // On pages: the text as wide as a page's, the sheets in the
        // middle (else at the left, the view scrolling sideways), the
        // first page's text a gap and a margin down.
        if self.pages != o.pages {
            self.pages = o.pages;
            self.key += 1;
            self.tops_dirty = true;
            // At the top of the text, still (on pages, the first sheet's
            // top), not its first paragraph kept where it was.
            if self.offset.1 <= 0.0 && self.set_top.is_none() {
                self.set_top = Some((0, 0.0));
            }
        }
        let o = match o.pages {
            Some(pg) => {
                let sheet_x = ((vw - pg.paper.0) / 2.0).max(pg.gap).round();
                Options { margin: (sheet_x + pg.margin, pg.gap + pg.margin), wrap: true, ..*o }
            }
            None => *o,
        };
        let inner = match o.pages {
            Some(pg) => pg.text_w(),
            None => (vw - 2.0 * o.margin.0).max(1.0),
        };
        if !o.wrap && (self.width - inner.round()).abs() > 0.5 {
            // (Unwrapped, a table is the view's width.)
            self.width = inner.round();
            self.key += 1;
        }
        self.set_look((o.wrap.then_some(inner.round()), o.align, o.justify));
        self.ensure_tops();

        let content_h = |e: &Editor| match o.pages {
            Some(pg) => e.page_count() as f32 * pg.pitch() + pg.gap,
            None => e.tops[e.paras.len()] + 2.0 * o.margin.1,
        };
        let content_w = |e: &Editor| match o.pages {
            Some(pg) => vw.max(pg.paper.0 + 2.0 * pg.gap),
            None if o.wrap => vw,
            None => vw.max(e.widest + 2.0 * o.margin.0),
        };
        // A scroll bar moved it: from there.
        let bar = self.scroll.offset();
        if (bar - self.told).x.abs() > px(0.25) || (bar - self.told).y.abs() > px(0.25) {
            self.offset = (-f32::from(bar.x), -f32::from(bar.y));
            self.set_top = None;
            let t = self.para_at_y(self.offset.1 - o.margin.1);
            self.top = (self.paras[t].start, self.offset.1 - (o.margin.1 + self.tops[t]));
        }
        let mut offset = self.offset;
        // The paragraph at the top of the view, where it stays.
        if let Some((at, down)) = self.set_top.take() {
            let i = self.para_of(at);
            // At (or above) the first paragraph's top: the very top, its
            // margin showing.
            let down = if i == 0 && down <= 0.0 { -o.margin.1 } else { down };
            self.top = (self.paras[i].start, down);
        }
        if self.scroll_by != (0.0, 0.0) {
            // The caret went as far, so it stays where it shows.
            let by = std::mem::take(&mut self.scroll_by);
            offset.1 = (offset.1 + by.1).clamp(0.0, (content_h(self) - vh).max(0.0));
            offset.0 = (offset.0 + by.0).clamp(0.0, (content_w(self) - vw).max(0.0));
            self.reveal = false;
            let t = self.para_at_y(offset.1 - o.margin.1);
            self.top = (self.paras[t].start, offset.1 - (o.margin.1 + self.tops[t]));
        }
        // Lay out what shows, measuring it (which may move what follows,
        // or the caret being revealed): again until it settles.
        let revealing = std::mem::take(&mut self.reveal);
        let mut shown: Vec<(usize, Placed)> = Vec::new();
        for pass in 0..6 {
            if revealing {
                let (x, top, h) = self.caret_rect(doc, sh, &o, self.caret, self.prefer_next);
                let pad = 2.0 * self.row_h.min(vh / 4.0);
                if top < offset.1 + pad * 0.5 {
                    offset.1 = top - pad;
                } else if top + h > offset.1 + vh - pad * 0.5 {
                    offset.1 = top + h + pad - vh;
                }
                if pass == 0 && !o.wrap {
                    if x < offset.0 + o.margin.0 {
                        offset.0 = x - o.margin.0 - inner / 4.0;
                    } else if x > offset.0 + vw - o.margin.0 {
                        offset.0 = x + o.margin.0 + inner / 4.0 - vw;
                    }
                    offset.0 = offset.0.max(0.0);
                }
                self.ensure_tops();
                offset.1 = offset.1.clamp(0.0, (content_h(self) - vh).max(0.0));
                let t = self.para_at_y(offset.1 - o.margin.1);
                self.top = (self.paras[t].start, offset.1 - (o.margin.1 + self.tops[t]));
            }
            if pass == 0 {
                // Paragraphs about to show, measured while this frame has
                // time.
                self.ensure_tops();
                let a = self.para_of(self.top.0);
                let top = self.tops[a] + self.top.1;
                self.more = self.measure_ahead(doc, sh, top, top + vh);
            }
            self.ensure_tops();
            let a = self.para_of(self.top.0);
            // (Kept within its paragraph, which may have shrunk: up to
            // the next one's top, past a page's foot included; the last
            // one's to the end.)
            let within = if a + 1 < self.paras.len() { self.top.1.min(self.tops[a + 1] - self.tops[a]) } else { self.top.1 };
            offset.1 = (o.margin.1 + self.tops[a] + within).clamp(0.0, (content_h(self) - vh).max(0.0));
            let first = self.para_at_y(offset.1 - o.margin.1);
            shown.clear();
            let mut i = first;
            while i < self.paras.len() && o.margin.1 + self.tops[i] < offset.1 + vh {
                let placed = self.placed(doc, sh, i);
                shown.push((i, placed));
                i += 1;
            }
            if !self.tops_dirty {
                break;
            }
        }
        self.ensure_tops();
        offset.0 = offset.0.clamp(0.0, (content_w(self) - vw).max(0.0));
        self.offset = offset;
        let (cw, ch) = (content_w(self), content_h(self).max(vh));
        self.told = point(px(-offset.0), px(-offset.1));
        self.scroll.set_offset(self.told);
        self.scroll.set_extent(view, size(px((cw - vw).max(0.0)), px((ch - vh).max(0.0))));
        // Drawn on whole device pixels, so the text moves as one and
        // doesn't shimmer as it goes; kept exact, so a slow glide's
        // steps add up (rounded each frame, a third of a pixel a frame
        // would never move).
        // Past an end as far as it's stretched.
        self.elastic.set_extent(size(px((cw - vw).max(0.0)), px((ch - vh).max(0.0))), size(px(vw), px(vh)));
        let (at, springing) = self.elastic.offset(point(px(-offset.0), px(-offset.1)));
        self.springing = springing;
        let stretched = (-f32::from(at.x), -f32::from(at.y));
        let drawn = ((stretched.0 * scale).round() / scale, (stretched.1 * scale).round() / scale);
        let origin = point(view.origin.x - px(drawn.0), view.origin.y - px(drawn.1));
        self.span = Some((f32::from(origin.x) + o.margin.0, inner.round()));
        // The sheets showing.
        let mut sheets = Vec::new();
        if let Some(pg) = o.pages {
            for k in 0..self.page_count() {
                let x = f32::from(origin.x) + o.margin.0 - pg.margin;
                let y = f32::from(origin.y) + o.margin.1 - pg.margin + pg.top(k);
                let r = Bounds::new(point(px(snap(x, scale)), px(snap(y, scale))), size(px(pg.paper.0), px(pg.paper.1)));
                if r.bottom() < view.top() - px(4.0) || r.top() > view.bottom() + px(4.0) {
                    continue;
                }
                sheets.push((k, r));
            }
        }
        self.shown = Shown { paras: shown, view, origin, sheets, o };
        None
    }

    /// Measure the paragraphs within a screen above and below the view
    /// (`top` to `bottom` down the text), nearest first, within the
    /// frame's budget; whether some are left.
    fn measure_ahead(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, top: f32, bottom: f32) -> bool {
        self.ensure_tops();
        let view = bottom - top;
        let (a, b) = (self.para_at_y(top), self.para_at_y(bottom));
        let (above, below) = (self.para_at_y(top - view), self.para_at_y(bottom + view));
        let start = Instant::now();
        for i in (b + 1..=below).chain((above..a).rev()) {
            if self.paras[i].key != NEVER && self.paras[i].key != STALE {
                continue;
            }
            if start.elapsed() >= MEASURE_BUDGET {
                return true;
            }
            self.placed(doc, sh, i);
        }
        false
    }

    /// Where content point `(x, y)` is in the window, as last shown.
    fn to_window(&self, (x, y): (f32, f32)) -> Point<Pixels> {
        point(self.shown.origin.x + px(x), self.shown.origin.y + px(y))
    }

    /// Where window point `p` is in the content, as last shown.
    fn to_content(&self, p: Point<Pixels>) -> (f32, f32) {
        (f32::from(p.x - self.shown.origin.x), f32::from(p.y - self.shown.origin.y))
    }

    /// The caret shows (after a keystroke or click), and blinks from
    /// there while the editor has the keyboard, for [`BLINK_FOR`].
    fn show_caret<V: EditorHost>(&mut self, key: usize, cx: &mut Context<V>) {
        self.caret_on = true;
        self.blink = Some(cx.spawn(async move |this, cx| {
            let mut shown = Duration::ZERO;
            loop {
                cx.background_executor().timer(BLINK).await;
                shown += BLINK;
                let stop = shown >= BLINK_FOR;
                let mut keep = false;
                let _ = this.update(cx, |v, cx| {
                    v.editor(key, cx, &mut |ed, _, cx| {
                        // (Shown when it stops.)
                        let on = stop || !ed.caret_on;
                        if ed.caret_on != on {
                            ed.caret_on = on;
                            cx.notify();
                        }
                        keep = !stop && ed.blink.is_some();
                    })
                });
                if !keep {
                    break;
                }
            }
        }));
    }

    /// A command from the keyboard (or a menu). Whether the text changed.
    fn command(&mut self, doc: &mut dyn Document, sh: &dyn Shaper, k: Cmd, cx: &mut App) -> bool {
        let o = self.shown.o;
        let text_len = doc.text().len();
        let sel = self.selection();
        let to: Option<(usize, bool)>;
        let mut keep_h = false;
        let mut extend = false;
        match k {
            Cmd::Enter if o.editable => {
                if doc.key(EditKey::Enter, sel.clone()) {
                    self.relayout_near();
                    return true;
                }
                // In a table, a row below.
                if let Some((i, cs, _)) = self.cell(doc.text()) {
                    return self.add_row(doc, i, cs.len());
                }
                self.edit(doc, sel, "\n");
                return true;
            }
            Cmd::Tab(back) => {
                if o.editable && doc.key(if back { EditKey::BackTab } else { EditKey::Tab }, sel.clone()) {
                    self.relayout_near();
                    return true;
                }
                if self.cell(doc.text()).is_some() {
                    return if o.editable { self.tab(doc, back).unwrap_or(false) } else { false };
                }
                if o.editable && !back {
                    self.edit(doc, sel, "\t");
                    return true;
                }
                return false;
            }
            Cmd::Backspace(word) | Cmd::Delete(word) if o.editable => {
                let back = matches!(k, Cmd::Backspace(_));
                if back && sel.is_empty() && doc.key(EditKey::Backspace, sel.clone()) {
                    self.relayout_near();
                    return true;
                }
                let r = if !sel.is_empty() {
                    sel
                } else {
                    let t = doc.text();
                    match (back, word) {
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
            Cmd::SelectAll => {
                self.anchor = 0;
                self.caret = text_len;
                return false;
            }
            Cmd::Copy | Cmd::Cut => {
                if !sel.is_empty() {
                    doc.copied(sel.clone());
                    cx.write_to_clipboard(ClipboardItem::new_string(doc.text()[sel.clone()].to_owned()));
                    if matches!(k, Cmd::Cut) && o.editable {
                        self.edit(doc, sel, "");
                        return true;
                    }
                }
                return false;
            }
            Cmd::Paste if o.editable => {
                let Some(item) = cx.read_from_clipboard() else { return false };
                return match doc.paste(&item, sel.clone()) {
                    Some(t) if !t.is_empty() => {
                        self.edit(doc, sel, &t.replace("\r\n", "\n").replace('\r', "\n"));
                        true
                    }
                    // (The document took it: a picture.)
                    None => {
                        self.relayout_near();
                        true
                    }
                    _ => false,
                };
            }
            Cmd::Left(s, _) | Cmd::Right(s, _) if !sel.is_empty() && !s && !matches!(k, Cmd::Left(_, true) | Cmd::Right(_, true)) => {
                let at = if matches!(k, Cmd::Left(..)) { sel.start } else { sel.end };
                to = Some((at, false));
            }
            Cmd::Left(s, word) => {
                let t = doc.text();
                extend = s;
                to = Some((if word { prev_word(t, self.caret) } else { prev_char(t, self.caret) }, true));
            }
            Cmd::Right(s, word) => {
                let t = doc.text();
                extend = s;
                to = Some((if word { next_word(t, self.caret) } else { next_char(t, self.caret) }, false));
            }
            Cmd::Up(s) | Cmd::Down(s) => {
                extend = s;
                to = Some(self.vertical(doc, sh, matches!(k, Cmd::Down(_))));
                keep_h = true;
            }
            Cmd::DocStart(s) | Cmd::DocEnd(s) => {
                extend = s;
                to = Some((if matches!(k, Cmd::DocStart(_)) { 0 } else { text_len }, false));
            }
            Cmd::Home(s) | Cmd::End(s) => {
                extend = s;
                to = Some(self.row_end(doc, sh, matches!(k, Cmd::End(_))));
            }
            Cmd::PageUp(s) | Cmd::PageDown(s) => {
                // A view's height up or down, the view going along.
                extend = s;
                let (x, top, h) = self.caret_rect(doc, sh, &o, self.caret, self.prefer_next);
                let hp = *self.h_pos.get_or_insert(x - o.margin.0);
                let vh = f32::from(self.shown.view.size.height);
                let dy = if matches!(k, Cmd::PageUp(_)) { -vh } else { vh };
                to = Some(self.byte_at(doc, sh, &o, (o.margin.0 + hp, top + h / 2.0 + dy)));
                keep_h = true;
                self.scroll_by.1 += dy;
            }
            _ => return false,
        }
        if let Some((at, next)) = to {
            if !extend {
                self.anchor = at;
            }
            self.caret = at;
            self.prefer_next = next;
            if !keep_h {
                self.h_pos = None;
            }
            self.reveal = self.scroll_by == (0.0, 0.0);
        }
        false
    }

    /// The document changed a paragraph's look itself (a list's key):
    /// everything is measured again.
    fn relayout_near(&mut self) {
        self.reveal = true;
    }

    /// Paints what [`Editor::prepare`] laid out.
    #[allow(clippy::too_many_arguments)]
    fn paint(&mut self, doc: &mut dyn Document, window: &mut Window, cx: &mut App, hitbox: &Hitbox) {
        let p = theme::palette(cx);
        let s = window.scale_factor();
        let view = self.shown.view;
        let o = self.shown.o;
        let origin = self.shown.origin;
        let focused = self.focus.is_focused(window) && window.is_window_active();
        let sel = self.selection();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let fill = style::c(p.control_highlight).opacity(if !focused { 0.3 } else if dark { 0.75 } else { 0.55 });
        let text_color = style::c(p.document_text);
        let clip = (f32::from(view.top()), f32::from(view.bottom()));
        let px1 = 1.0 / s;
        // On pages: the sheets showing, in the document's colour, each
        // with a shadow on the desk.
        if o.pages.is_some() {
            let paper = style::c(o.background.unwrap_or(p.document_background));
            for (_, r) in &self.shown.sheets {
                let at = |dx: f32, dy: f32| Bounds { origin: point(r.origin.x + px(dx), r.origin.y + px(dy)), size: r.size };
                style::fill_rect(window, at(snap(2.0, s), snap(3.0, s)), gpui::black().opacity(28.0 / 255.0));
                style::fill_rect(window, at(px1, px1), gpui::black().opacity(46.0 / 255.0));
                style::fill_rect(window, *r, paper);
            }
        }
        let checker = (o.spelling && o.editable).then(|| crate::spell::checker(cx)).flatten();
        let squiggle_color = style::c(p.failure);
        // What the worker found since (as of the words known now).
        if let Ok(mut done) = self.spell_done.lock() {
            for (key, generation, wrong) in done.drain(..) {
                if generation == self.spell_gen {
                    self.spell_pending.remove(&key);
                    self.misspelled.insert(key, wrong);
                }
            }
        }
        let mut to_check: Vec<(u64, String)> = Vec::new();
        // Rules in the text's colour, faint (a border's colour is lost on a
        // dark page); a header shaded with the controls'.
        let rule = text_color.opacity(0.35);
        let shade = style::c(p.control_background);
        let dragging = style::c(p.keyboard_navigation);
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
        let shown = std::mem::take(&mut self.shown.paras);
        let text = doc.text();
        let line = |v: f32| ((v * s).floor() + 0.5) / s;
        // The text in one layer: one place in the scene for all its
        // glyphs, lines and fills (each glyph its own, outside one, made
        // painting a page's worth cost milliseconds). Fills go under
        // lines, lines under glyphs, as wanted.
        // Selected pictures' tints, over them once the layer is done (in
        // a layer, fills go under its sprites).
        let mut tints = Vec::new();
        window.paint_layer(view, |window| {
            for (i, placed) in &shown {
                let para = self.paras[*i];
                let (ox, oy) = self.origin(*i, &o);
                let at = (snap(f32::from(origin.x) + ox, s), snap(f32::from(origin.y) + oy, s));
                let end = para.start + para.len;
                // A row: its cells' rules, a device pixel wide on device
                // pixels (rows meet on one), and a header's shade.
                if let Some((h, header)) = placed.row {
                    // From the rows' tops before snapping, so a row's bottom
                    // rule and the next's top fall on the same pixel.
                    let y0 = f32::from(origin.y) + o.margin.1 + self.tops[*i];
                    let (top, bottom) = (line(y0), line(y0 + h));
                    let mut segs = Vec::new();
                    for q in &placed.parts {
                        let (l, r) = (line(at.0 + q.frame.0), line(at.0 + q.frame.0 + q.frame.1));
                        if header {
                            let half = px1 / 2.0;
                            style::fill_rect(window, Bounds::from_corners(point(px(l - half), px(top - half)), point(px(r + half), px(bottom + half))), shade);
                        }
                        segs.push((point(px(l), px(top)), point(px(l), px(bottom))));
                        segs.push((point(px(r), px(top)), point(px(r), px(bottom))));
                        segs.push((point(px(l), px(top)), point(px(r), px(top))));
                        segs.push((point(px(l), px(bottom)), point(px(r), px(bottom))));
                    }
                    window.paint_segments(&segs, px(px1), false, rule);
                    // The inner rules, to take hold of (a few points either side).
                    let widths: Vec<f32> = placed.parts.iter().map(|q| q.frame.1).collect();
                    for (k, q) in placed.parts.iter().enumerate().skip(1) {
                        let x = at.0 + q.frame.0;
                        self.rules.push((Bounds::from_corners(point(px(x - 3.0), px(top)), point(px(x + 3.0), px(bottom))), *i, k, widths.clone()));
                    }
                    // One being dragged: where it would go, across the table.
                    if let (Some(d), Some(t)) = (&self.column_drag, &drag_table)
                        && t.contains(i)
                        && let Some(q) = placed.parts.get(d.k)
                    {
                        let x = line(at.0 + q.frame.0 + d.moved());
                        window.paint_segment(point(px(x), px(top)), point(px(x), px(bottom)), px(2.0 * px1), false, dragging);
                    }
                }
                for q in &placed.parts {
                    let (qs, qe) = (para.start + q.start, para.start + q.start + q.len);
                    let mut qat = (snap(at.0 + q.at.0, s), snap(at.1 + q.at.1, s));
                    if placed.row.is_some() {
                        // Up a part pixel rather than down: capitals sit a
                        // little below their line's middle, so a cell's text
                        // rounded down would look low in its row.
                        qat.1 = ((at.1 + q.at.1) * s).floor() / s;
                    }
                    // The selection behind the text (the paragraph's newline,
                    // or a cell's tab, too, as a sliver).
                    if !sel.is_empty() && sel.start <= qe && sel.end > qs {
                        let local = sel.start.clamp(qs, qe) - qs..sel.end.clamp(qs, qe) - qs;
                        for (x0, y0, x1, y1) in q.laid.range_rects(local, sel.end > qe) {
                            let b = Bounds::from_corners(point(px(snap(qat.0 + x0, s)), px(snap(qat.1 + y0, s))), point(px(snap(qat.0 + x1, s)), px(snap(qat.1 + y1, s))));
                            style::fill_rect(window, b, fill);
                        }
                    }
                    // What an input method is composing, underlined.
                    if let Some(mk) = self.marked.clone().filter(|mk| mk.start < qe && mk.end > qs) {
                        let local = mk.start.clamp(qs, qe) - qs..mk.end.clamp(qs, qe) - qs;
                        for (x0, _, x1, y1) in q.laid.range_rects(local, false) {
                            let y = snap(qat.1 + y1, s) - px1;
                            style::fill_rect(window, Bounds::new(point(px(qat.0 + x0), px(y)), size(px(x1 - x0), px(px1))), text_color);
                        }
                    }
                    q.laid.paint(window, point(px(qat.0), px(qat.1)), text_color, clip);
                    // Its pictures, over their characters.
                    for (byte, (x0, y0, x1, y1)) in q.laid.pictures() {
                        let Some((_, pic)) = placed.pictures.iter().find(|(b, _)| *b == q.start + byte) else { continue };
                        let b = Bounds::from_corners(point(px(snap(qat.0 + x0, s)), px(snap(qat.1 + y0, s))), point(px(snap(qat.0 + x1, s)), px(snap(qat.1 + y1, s))));
                        match &pic.image {
                            Some(img) => {
                                let _ = window.paint_image(b, Default::default(), img.clone(), 0, false);
                            }
                            None => {
                                style::fill_rect(window, b, style::c(p.control_background));
                                window.paint_quad(gpui::outline(b, style::disabled_text(&p), gpui::BorderStyle::Solid).border_widths(px(px1)));
                            }
                        }
                        // Selected: tinted as selected text's ground is.
                        if !sel.is_empty() && (sel.start..sel.end).contains(&(qs + byte)) {
                            tints.push(b);
                        }
                    }
                }
                if checker.is_some() {
                    let ptext = &text[para.start..end];
                    let key = {
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        ptext.hash(&mut h);
                        h.finish()
                    };
                    if self.misspelled.len() > 4096 {
                        self.misspelled.clear();
                    }
                    // Checked once, on a worker (marked when it's done).
                    let wrong = match self.misspelled.get(&key) {
                        Some(w) => w.clone(),
                        None => {
                            if self.spell_pending.insert(key) {
                                to_check.push((key, ptext.to_owned()));
                            }
                            Arc::new(Vec::new())
                        }
                    };
                    let mut segs = Vec::new();
                    for r in wrong.iter() {
                        // Not the word being typed.
                        if focused && sel.is_empty() && (para.start + r.start..=para.start + r.end).contains(&self.caret) {
                            continue;
                        }
                        // In its cell (a word never crosses a tab).
                        let q = &placed.parts[placed.part_of(r.start)];
                        let local = r.start.saturating_sub(q.start)..(r.end - q.start).min(q.len);
                        let qat = (at.0 + q.at.0, at.1 + q.at.1);
                        for (x0, _, x1, y1) in q.laid.range_rects(local, false) {
                            squiggle(&mut segs, snap(qat.0 + x0, s), snap(qat.0 + x1, s), snap(qat.1 + y1, s) - 2.0 * px1, s);
                        }
                    }
                    if !segs.is_empty() {
                        window.paint_segments(&segs, px(px1), false, squiggle_color);
                    }
                }
                if let Some(mk) = &placed.marker {
                    let laid = &placed.parts[0].laid;
                    let first_top = laid.rows.first().map_or(0.0, |r| r.top);
                    let w = mk.width();
                    let gap = (mk.height() * 0.35).round();
                    let x = f32::from(origin.x) + o.margin.0 + para.set.left - gap - w;
                    // On the first row's baseline.
                    let dy = laid.rows.first().map_or(0.0, |r| r.ascent) - mk.rows.first().map_or(0.0, |r| r.ascent);
                    mk.paint(window, point(px(snap(x, s)), px(at.1 + first_top + dy)), text_color, clip);
                }
            }
        });
        for b in tints {
            style::fill_rect(window, b, fill.opacity(fill.a * 0.6));
        }
        self.shown.paras = shown;
        if let Some(c) = checker
            && !to_check.is_empty()
        {
            let (done, generation, handle) = (self.spell_done.clone(), self.spell_gen, window.window_handle());
            let checked = cx.background_executor().spawn(async move { to_check.into_iter().map(|(key, text)| (key, generation, Arc::new(c.misspelled(&text)))).collect::<Vec<_>>() });
            cx.spawn(async move |cx| {
                let found = checked.await;
                if let Ok(mut d) = done.lock() {
                    d.extend(found);
                }
                let _ = handle.update(cx, |_, window, _| window.refresh());
            })
            .detach();
        }
        // The caret, a line wide, on device pixels.
        let sh = window.text_system().clone();
        if focused && sel.is_empty() && self.caret_on && self.marked.is_none() {
            let (x, top, h) = self.caret_rect(doc, &*sh, &o, self.caret, self.prefer_next);
            let at = self.to_window((x, top));
            let right = f32::from(view.right()) - px1;
            let x = snap(f32::from(at.x).min(right), s);
            style::fill_rect(window, Bounds::new(point(px(x), px(snap(f32::from(at.y), s))), size(px(px1.max(if s < 2.0 { px1 } else { 1.0 })), px(snap(h, s)))), style::c(p.navigation_base));
        }
        // Text dragged within it: where it would go.
        if let Gesture::Moving { to: Some(to) } = self.gesture {
            let (x, top, h) = self.caret_rect(doc, &*sh, &o, to, false);
            let at = self.to_window((x, top));
            let x = snap(f32::from(at.x), s);
            style::fill_rect(window, Bounds::new(point(px(x), at.y), size(px(2.0 * px1.max(0.5)), px(h))), style::c(p.keyboard_navigation));
        }
        // The pointer: a text beam, a column's rule to size it.
        let on_rule = window.mouse_position();
        let rule_here = o.editable && self.rules.iter().any(|(r, ..)| r.contains(&on_rule));
        window.set_cursor_style(if rule_here || self.column_drag.is_some() { CursorStyle::ResizeColumn } else { CursorStyle::IBeam }, hitbox);
        // For agents (and screen readers): the whole text, where it shows.
        agent::record(window, cx, agent::Node { role: "TextInput", name: None, value: Some(self.agent_text.get_or_insert_with(|| SharedString::from(doc.text().to_owned())).clone()), bounds: view, enabled: o.editable, clickable: true, focused, ..Default::default() });
        self.kept.roll();
        self.shapes.roll();
    }
}

/// What a key (or menu) asks of the editor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmd {
    /// Extend the selection; by words.
    Left(bool, bool),
    Right(bool, bool),
    Up(bool),
    Down(bool),
    Home(bool),
    End(bool),
    DocStart(bool),
    DocEnd(bool),
    PageUp(bool),
    PageDown(bool),
    /// By words.
    Backspace(bool),
    Delete(bool),
    SelectAll,
    Copy,
    Cut,
    Paste,
    Enter,
    /// ⇧Tab.
    Tab(bool),
}

actions!(
    olo_editor,
    [
        Left, Right, WordLeft, WordRight, SelectLeft, SelectRight, SelectWordLeft, SelectWordRight, Up, Down, SelectUp, SelectDown, Home, End, SelectHome, SelectEnd, DocStart, DocEnd, SelectDocStart, SelectDocEnd, PageUp, PageDown, SelectPageUp, SelectPageDown,
        Backspace, DeleteWordLeft, Delete, DeleteWordRight, SelectAll, Copy, Cut, Paste, Enter, Tab, BackTab
    ]
);

const CONTEXT: &str = "OloEditor";

/// The editor's keys (the command key reaches apps as Ctrl).
pub(crate) fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("ctrl-home", DocStart, c),
        KeyBinding::new("ctrl-end", DocEnd, c),
        KeyBinding::new("ctrl-up", DocStart, c),
        KeyBinding::new("ctrl-down", DocEnd, c),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
        KeyBinding::new("ctrl-shift-up", SelectDocStart, c),
        KeyBinding::new("ctrl-shift-down", SelectDocEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("ctrl-x", Cut, c),
        KeyBinding::new("ctrl-v", Paste, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", Enter, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("shift-tab", BackTab, c),
    ]);
}

/// The editor's own edits an app's menu (or its keys) may ask for: they
/// go through the [`Document`] as typing does (a paste through
/// [`Document::paste`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// Does `edit` in editor `key` of `host` (Edit ▸ Paste, from the menu).
pub fn edit<V: EditorHost>(host: &Entity<V>, key: usize, edit: Edit, window: &mut Window, cx: &mut App) {
    let k = match edit {
        Edit::Cut => Cmd::Cut,
        Edit::Copy => Cmd::Copy,
        Edit::Paste => Cmd::Paste,
        Edit::SelectAll => Cmd::SelectAll,
    };
    run(host, key, k, window, cx);
}

/// Runs command `k` on editor `key` of `host`.
fn run<V: EditorHost>(host: &Entity<V>, key: usize, k: Cmd, window: &mut Window, cx: &mut App) {
    let sh = window.text_system().clone();
    host.update(cx, |v, cx| {
        let mut changed = false;
        v.editor(key, cx, &mut |ed, doc, cx| {
            let before = (ed.anchor, ed.caret);
            changed = ed.command(doc, &*sh, k, cx);
            if changed || (ed.anchor, ed.caret) != before {
                ed.show_caret(key, cx);
            }
            cx.notify();
        });
        if changed {
            v.edited(key, cx);
        }
    });
}

/// Shows editor `key` of `host` (which lends it, with its document): the
/// text in a BeOS scroll view that fills the box it's given, the bars on
/// the editor's own scrolling.
pub fn view<V: EditorHost>(host: &Entity<V>, key: usize, editor: &Editor, options: Options, window: &Window) -> gpui::AnyElement {
    let focused = editor.focus.is_focused(window);
    let mut sv = ScrollView::new(("olo-editor-scroll", key), &editor.scroll).scrolls_itself(true).horizontal(!options.wrap || options.pages.is_some()).focused(focused);
    // On pages, the desk under the sheets (each in the document's own
    // colour).
    sv = match (options.pages, options.background) {
        (Some(_), _) => sv.desk(),
        (None, Some(bg)) => sv.background(bg),
        (None, None) => sv,
    };
    let mut d = div().id(("olo-editor", key)).key_context(CONTEXT).track_focus(&editor.focus).size_full();
    macro_rules! on {
        ($($A:ty => $k:expr),* $(,)?) => {
            $(
                let h = host.clone();
                d = d.on_action(move |_: &$A, window: &mut Window, cx: &mut App| run(&h, key, $k, window, cx));
            )*
        };
    }
    on!(
        Left => Cmd::Left(false, false), Right => Cmd::Right(false, false), WordLeft => Cmd::Left(false, true), WordRight => Cmd::Right(false, true),
        SelectLeft => Cmd::Left(true, false), SelectRight => Cmd::Right(true, false), SelectWordLeft => Cmd::Left(true, true), SelectWordRight => Cmd::Right(true, true),
        Up => Cmd::Up(false), Down => Cmd::Down(false), SelectUp => Cmd::Up(true), SelectDown => Cmd::Down(true),
        Home => Cmd::Home(false), End => Cmd::End(false), SelectHome => Cmd::Home(true), SelectEnd => Cmd::End(true),
        DocStart => Cmd::DocStart(false), DocEnd => Cmd::DocEnd(false), SelectDocStart => Cmd::DocStart(true), SelectDocEnd => Cmd::DocEnd(true),
        PageUp => Cmd::PageUp(false), PageDown => Cmd::PageDown(false), SelectPageUp => Cmd::PageUp(true), SelectPageDown => Cmd::PageDown(true),
        Backspace => Cmd::Backspace(false), DeleteWordLeft => Cmd::Backspace(true), Delete => Cmd::Delete(false), DeleteWordRight => Cmd::Delete(true),
        SelectAll => Cmd::SelectAll, Copy => Cmd::Copy, Cut => Cmd::Cut, Paste => Cmd::Paste, Enter => Cmd::Enter, Tab => Cmd::Tab(false), BackTab => Cmd::Tab(true),
    );
    d.child(sv.child(EditorElement { host: host.clone(), key, options })).into_any_element()
}

/// Lets `f` at editor `key` of `host` with its document and a text system
/// (outside a frame: a press, a key).
fn with<V: EditorHost, R: Default>(host: &Entity<V>, key: usize, window: &mut Window, cx: &mut App, mut f: impl FnMut(&mut Editor, &mut dyn Document, &dyn Shaper, &mut Window, &mut Context<V>) -> R) -> R {
    let sh = window.text_system().clone();
    let mut out = R::default();
    host.update(cx, |v, cx| {
        v.editor(key, cx, &mut |ed, doc, cx| out = f(ed, doc, &*sh, window, cx));
    });
    out
}

/// The editor in its view: laid out as it shows, painted, and the pointer
/// and typing taken.
struct EditorElement<V: EditorHost> {
    host: Entity<V>,
    key: usize,
    options: Options,
}

impl<V: EditorHost> IntoElement for EditorElement<V> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<V: EditorHost> Element for EditorElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Hitbox {
        let o = self.options;
        let s = window.scale_factor();
        let mut again = false;
        // A press held past an edge: the view scrolls that way.
        let pointer = window.mouse_position();
        let mut moved = None;
        let ground = o.background.unwrap_or(theme::palette(cx).document_background);
        let paper = (olo_theme::luminance(ground) < 0.18).then_some(ground);
        with(&self.host, self.key, window, cx, |ed, doc, sh, _, _| {
            let was = ed.span;
            if ed.paper != paper {
                ed.paper = paper;
                ed.legible.clear();
            }
            if ed.gesture == Gesture::Selecting
                && let Some(p) = ed.pointer
            {
                let _ = pointer;
                let past = if p.y < bounds.top() { f32::from(p.y - bounds.top()) } else if p.y > bounds.bottom() { f32::from(p.y - bounds.bottom()) } else { 0.0 };
                let past_x = if o.wrap { 0.0 } else if p.x < bounds.left() { f32::from(p.x - bounds.left()) } else if p.x > bounds.right() { f32::from(p.x - bounds.right()) } else { 0.0 };
                if past != 0.0 || past_x != 0.0 {
                    // Faster the further out, a frame at a time.
                    let speed = |d: f32| d.signum() * (d.abs() * 0.25).clamp(1.0, 40.0);
                    ed.scroll_by.0 += if past_x != 0.0 { speed(past_x) } else { 0.0 };
                    ed.scroll_by.1 += if past != 0.0 { speed(past) } else { 0.0 };
                    again = true;
                }
            }
            ed.prepare(doc, sh, &o, bounds, s);
            if ed.span != was {
                moved = ed.span;
            }
            if again {
                // The caret follows along the edge.
                let edge = (
                    f32::from(p_clamp(pointer.x, bounds.left() + px(0.5), bounds.right() - px(0.5))),
                    f32::from(p_clamp(pointer.y, bounds.top() + px(0.5), bounds.bottom() - px(0.5))),
                );
                let c = ed.to_content(point(px(edge.0), px(edge.1)));
                let (at, next) = ed.byte_at(doc, sh, &ed.shown.o.clone(), c);
                ed.caret = at;
                ed.prefer_next = next;
            }
            again |= ed.more || ed.springing;
        });
        if let Some(span) = moved {
            let key = self.key;
            self.host.update(cx, |v, cx| v.text_moved(key, span, cx));
        }
        if again {
            window.request_animation_frame();
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        let (host, key) = (self.host.clone(), self.key);
        let mut focus = None;
        let mut editable = false;
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            let hb = hitbox.clone();
            host.update(cx, |v, cx| {
                v.editor(key, cx, &mut |ed, doc, cx| {
                    // Blink only while it has the keyboard in an active window.
                    let focused = ed.focus.is_focused(window) && window.is_window_active();
                    if focused && ed.blink.is_none() {
                        ed.show_caret(key, cx);
                    } else if !focused && ed.blink.is_some() {
                        ed.blink = None;
                        ed.caret_on = true;
                    }
                    ed.paint(doc, window, cx, &hb);
                    focus = Some(ed.focus.clone());
                    editable = ed.shown.o.editable;
                });
            });
        });
        if let Some(f) = &focus
            && editable
        {
            window.handle_input(f, EditorInput { host: host.clone(), key }, cx);
        }

        // The pointer.
        let hb = hitbox.clone();
        let h = host.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hb.is_hovered(window) {
                return;
            }
            match e.button {
                MouseButton::Left => press(&h, key, e, window, cx),
                MouseButton::Right => {
                    let at = e.position;
                    with(&h, key, window, cx, |ed, doc, sh, window, _| {
                        window_focus(ed, window);
                        // A right-click outside the selection puts the
                        // caret there first, so the menu is about what was
                        // clicked (a misspelled word's suggestions).
                        let o = ed.shown.o;
                        let (b, next) = ed.byte_at(doc, sh, &o, ed.to_content(at));
                        let sel = ed.selection();
                        if !(sel.start..=sel.end).contains(&b) || sel.is_empty() {
                            ed.anchor = b;
                            ed.caret = b;
                            ed.prefer_next = next;
                            ed.h_pos = None;
                        }
                    });
                    h.update(cx, |v, cx| v.context_menu(key, at, window, cx));
                    cx.stop_propagation();
                }
                _ => {}
            }
        });
        let h = host.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            drag(&h, key, e, window, cx);
        });
        let h = host.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            release(&h, key, e, window, cx);
        });
        let hb = hitbox.clone();
        let h = host.clone();
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hb.should_handle_scroll(window) {
                return;
            }
            let d = e.delta.pixel_delta(window.line_height());
            h.update(cx, |v, cx| {
                v.editor(key, cx, &mut |ed, _, cx| {
                    let o = ed.shown.o;
                    let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                    let sideways = !o.wrap || o.pages.is_some();
                    // Fingers and a glide stretch past an end and spring
                    // back (their ends too: the fingers lifting).
                    if e.kind != gpui::ScrollKind::Wheel {
                        let at = (ed.offset.0 + ed.scroll_by.0, ed.offset.1 + ed.scroll_by.1);
                        let mut to = point(px(-at.0), px(-at.1));
                        let by = point(px(if sideways { dx } else { 0.0 }), px(dy));
                        let stretched = ed.elastic.scroll(&mut to, by, e.kind, e.touch_phase);
                        ed.scroll_by.0 += -f32::from(to.x) - at.0;
                        ed.scroll_by.1 += -f32::from(to.y) - at.1;
                        if stretched || (-f32::from(to.x), -f32::from(to.y)) != at {
                            cx.notify();
                        }
                        return;
                    }
                    // (A scroll's start or end moves nothing: no frame.)
                    if dx == 0.0 && dy == 0.0 {
                        return;
                    }
                    // Nowhere to go that way (an edge): no frame, so the
                    // rest of an eased scroll stops too.
                    let max = ed.scroll.max_offset();
                    let can = |at: f32, d: f32, max: Pixels| (d > 0.0 && at > 0.0) || (d < 0.0 && at < f32::from(max) - 0.01);
                    if !can(ed.offset.1, dy, max.height) && !(sideways && can(ed.offset.0, dx, max.width)) {
                        return;
                    }
                    ed.scroll_by.1 -= dy;
                    if sideways {
                        ed.scroll_by.0 -= dx;
                    }
                    cx.notify();
                });
            });
        });
        let _ = TypeId::of::<V>();
    }
}

fn p_clamp(v: Pixels, lo: Pixels, hi: Pixels) -> Pixels {
    v.max(lo).min(hi.max(lo))
}

fn window_focus(ed: &Editor, window: &mut Window) {
    window.focus(&ed.focus);
}

/// The primary button went down on the editor.
fn press<V: EditorHost>(host: &Entity<V>, key: usize, e: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    let pos = e.position;
    let (clicks, shift, alt) = (e.click_count, e.modifiers.shift, e.modifiers.alt);
    with(host, key, window, cx, |ed, doc, sh, window, cx| {
        let o = ed.shown.o;
        let had = ed.focus.is_focused(window);
        window.focus(&ed.focus);
        // A table's inner rule: dragged, it sizes the columns either side.
        if o.editable
            && let Some((_, para, k, widths)) = ed.rules.iter().find(|(r, ..)| r.contains(&pos)).cloned()
        {
            ed.column_drag = Some(ColumnDrag { para, k, widths, press: f32::from(pos.x), by: 0.0 });
            cx.notify();
            return;
        }
        let (at, next) = ed.byte_at(doc, sh, &o, ed.to_content(pos));
        let sel = ed.selection();
        if clicks >= 2 {
            let text = doc.text();
            let at = at.min(text.len());
            let r = if clicks >= 3 {
                let p = ed.paras[ed.para_of(at)];
                p.start..(p.start + p.len + 1).min(text.len())
            } else {
                word_at(text, at)
            };
            ed.anchor = r.start;
            ed.caret = r.end;
            ed.gesture = Gesture::Selecting;
        } else if had && !shift && !alt && !sel.is_empty() && sel.contains(&at) {
            ed.gesture = Gesture::Armed { press: pos, at };
        } else {
            if !shift {
                ed.anchor = at;
            }
            ed.caret = at;
            ed.prefer_next = next;
            ed.h_pos = None;
            ed.gesture = Gesture::Selecting;
        }
        ed.pointer = Some(pos);
        ed.show_caret(key, cx);
        cx.notify();
    });
    cx.stop_propagation();
}

/// The pointer moved (a press held: selecting, dragging text or a rule).
fn drag<V: EditorHost>(host: &Entity<V>, key: usize, e: &MouseMoveEvent, window: &mut Window, cx: &mut App) {
    let pos = e.position;
    let pressed = e.pressed_button == Some(MouseButton::Left);
    let mut out: Option<(String, Range<usize>)> = None;
    with(host, key, window, cx, |ed, doc, sh, window, cx| {
        if let Some(d) = &mut ed.column_drag {
            d.by = f32::from(pos.x) - d.press;
            cx.notify();
            return;
        }
        if !pressed {
            return;
        }
        let o = ed.shown.o;
        match ed.gesture.clone() {
            Gesture::Selecting => {
                ed.pointer = Some(pos);
                let view = ed.shown.view;
                let inside = point(p_clamp(pos.x, view.left() + px(0.5), view.right() - px(0.5)), p_clamp(pos.y, view.top() + px(0.5), view.bottom() - px(0.5)));
                let (at, next) = ed.byte_at(doc, sh, &o, ed.to_content(inside));
                if at != ed.caret {
                    ed.caret = at;
                    ed.prefer_next = next;
                    ed.show_caret(key, cx);
                }
                cx.notify();
            }
            Gesture::Armed { press, .. } => {
                let d = pos - press;
                if f32::from(d.x).hypot(f32::from(d.y)) > SLOP {
                    ed.gesture = Gesture::Moving { to: None };
                    cx.notify();
                }
            }
            Gesture::Moving { .. } => {
                let view = ed.shown.view;
                if view.contains(&pos) {
                    let (at, _) = ed.byte_at(doc, sh, &o, ed.to_content(pos));
                    ed.gesture = Gesture::Moving { to: Some(at) };
                } else if !window.bounds().contains(&(pos + window.bounds().origin)) || !view.contains(&pos) {
                    // Out of the view: the text goes as a drag, to
                    // wherever it's dropped.
                    let sel = ed.selection();
                    ed.gesture = Gesture::None;
                    out = Some((doc.text()[sel.clone()].to_owned(), sel));
                }
                cx.notify();
            }
            Gesture::None => {}
        }
    });
    if let Some((text, range)) = out {
        let bytes: Arc<[u8]> = Arc::from(text.into_bytes());
        let drag = gpui::OutgoingDrag {
            types: vec![("text/plain;charset=utf-8".into(), bytes.clone()), ("UTF8_STRING".into(), bytes.clone()), ("text/plain".into(), bytes)],
            copy: true,
            move_: true,
            ask: false,
            icon: None,
        };
        let h = host.clone();
        crate::dnd::start(window, cx, drag, move |action, cx| {
            // Moved out: it goes from here.
            if action == Some(gpui::DragAction::Move) {
                h.update(cx, |v, cx| {
                    let mut changed = false;
                    v.editor(key, cx, &mut |ed, doc, cx| {
                        if ed.shown.o.editable && range.end <= doc.text().len() {
                            ed.edit(doc, range.clone(), "");
                            changed = true;
                            cx.notify();
                        }
                    });
                    if changed {
                        v.edited(key, cx);
                    }
                });
            }
        });
    }
}

/// The primary button came up.
fn release<V: EditorHost>(host: &Entity<V>, key: usize, e: &MouseUpEvent, window: &mut Window, cx: &mut App) {
    let pos = e.position;
    let sh = window.text_system().clone();
    host.update(cx, |v, cx| {
        let mut changed = false;
        v.editor(key, cx, &mut |ed, doc, cx| {
            ed.pointer = None;
            if let Some(d) = ed.column_drag.take() {
                let mut widths = d.widths.clone();
                let by = d.moved();
                widths[d.k - 1] += by;
                widths[d.k] -= by;
                if by != 0.0 && d.para < ed.paras.len() {
                    let p = ed.paras[d.para];
                    doc.set_columns(p.start..p.start + p.len, widths);
                    ed.relayout();
                    changed = true;
                }
                cx.notify();
                return;
            }
            match std::mem::replace(&mut ed.gesture, Gesture::None) {
                Gesture::Armed { at, .. } => {
                    // A click on the selection: the caret goes there.
                    ed.anchor = at;
                    ed.caret = at;
                    ed.show_caret(key, cx);
                }
                Gesture::Moving { .. } if ed.shown.o.editable => {
                    let o = ed.shown.o;
                    let (to, _) = ed.byte_at(doc, &*sh, &o, ed.to_content(pos));
                    let from = ed.selection();
                    if !(from.start..=from.end).contains(&to) {
                        let region = from.start.min(to)..from.end.max(to);
                        let sel = ed.selection();
                        let r = doc.move_text(from, to, sel);
                        ed.replaced(doc.text(), region.clone(), region.len());
                        ed.select(doc.text(), r);
                        changed = true;
                    }
                }
                _ => {}
            }
            cx.notify();
        });
        if changed {
            v.edited(key, cx);
        }
    });
}

/// Typing and input methods, for editor `key` of `host`.
struct EditorInput<V: EditorHost> {
    host: Entity<V>,
    key: usize,
}

fn utf16_of(text: &str, at: usize) -> usize {
    let t = &text[..floor(text, at)];
    if t.is_ascii() { t.len() } else { t.chars().map(char::len_utf16).sum() }
}

fn byte_of_utf16(text: &str, n: usize) -> usize {
    if text.len() >= n && text.as_bytes()[..n].is_ascii() {
        return n;
    }
    let mut seen = 0;
    for (i, ch) in text.char_indices() {
        if seen >= n {
            return i;
        }
        seen += ch.len_utf16();
    }
    text.len()
}

impl<V: EditorHost> gpui::InputHandler for EditorInput<V> {
    fn selected_text_range(&mut self, _: bool, window: &mut Window, cx: &mut App) -> Option<UTF16Selection> {
        with(&self.host, self.key, window, cx, |ed, doc, _, _, _| {
            let t = doc.text();
            let r = ed.selection();
            Some(UTF16Selection { range: utf16_of(t, r.start)..utf16_of(t, r.end), reversed: ed.caret < ed.anchor })
        })
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        with(&self.host, self.key, window, cx, |ed, doc, _, _, _| ed.marked.clone().map(|r| utf16_of(doc.text(), r.start)..utf16_of(doc.text(), r.end)))
    }

    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, window: &mut Window, cx: &mut App) -> Option<String> {
        let (text, got) = with(&self.host, self.key, window, cx, |_, doc, _, _, _| {
            let t = doc.text();
            let (a, b) = (byte_of_utf16(t, range.start), byte_of_utf16(t, range.end));
            Some((t[a..b].to_owned(), utf16_of(t, a)..utf16_of(t, b)))
        })?;
        *actual = Some(got);
        Some(text)
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut App) {
        // (Return is the Enter key's.)
        if text == "\n" || text == "\r" {
            return;
        }
        let key = self.key;
        let sh = window.text_system().clone();
        self.host.update(cx, |v, cx| {
            let mut changed = false;
            v.editor(key, cx, &mut |ed, doc, cx| {
                if !ed.shown.o.editable {
                    return;
                }
                let t = doc.text();
                let r = range.as_ref().map(|r| byte_of_utf16(t, r.start)..byte_of_utf16(t, r.end)).or(ed.marked.clone()).unwrap_or(ed.selection());
                ed.marked = None;
                if text.is_empty() && r.is_empty() {
                    return;
                }
                ed.edit(doc, r, text);
                let _ = &sh;
                ed.show_caret(key, cx);
                changed = true;
                cx.notify();
            });
            if changed {
                v.edited(key, cx);
            }
        });
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, window: &mut Window, cx: &mut App) {
        let key = self.key;
        let _ = window;
        self.host.update(cx, |v, cx| {
            v.editor(key, cx, &mut |ed, doc, cx| {
                if !ed.shown.o.editable {
                    return;
                }
                let t = doc.text();
                let r = range.as_ref().map(|r| byte_of_utf16(t, r.start)..byte_of_utf16(t, r.end)).or(ed.marked.clone()).unwrap_or(ed.selection());
                ed.edit_raw(doc, r.clone(), text, text.len());
                ed.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
                if let Some(s) = &selected {
                    let a = r.start + byte_of_utf16(text, s.start);
                    let b = r.start + byte_of_utf16(text, s.end);
                    ed.anchor = a;
                    ed.caret = b;
                }
                cx.notify();
            });
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        with(&self.host, self.key, window, cx, |ed, _, _, _, _| ed.marked = None);
    }

    fn bounds_for_range(&mut self, range: Range<usize>, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        with(&self.host, self.key, window, cx, |ed, doc, sh, _, _| {
            let at = byte_of_utf16(doc.text(), range.start);
            let o = ed.shown.o;
            let (x, top, h) = ed.caret_rect(doc, sh, &o, at, false);
            let p = ed.to_window((x, top));
            Some(Bounds::new(p, size(px(1.0), px(h))))
        })
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut App) -> Option<usize> {
        with(&self.host, self.key, window, cx, |ed, doc, sh, _, _| {
            let o = ed.shown.o;
            let (b, _) = ed.byte_at(doc, sh, &o, ed.to_content(p));
            Some(utf16_of(doc.text(), b))
        })
    }
}

/// Makes `spans` cover `len` bytes (the last one longer or shorter; one
/// in the UI's font for none).
fn cover(spans: &mut Vec<Span>, len: usize) {
    spans.retain(|s| s.len > 0 || len == 0);
    if spans.is_empty() {
        spans.push(Span::new(len, style::ui_font(gpui::FontWeight::NORMAL), style::PLAIN_SIZE));
        return;
    }
    let total: usize = spans.iter().map(|s| s.len).sum();
    if total != len {
        let last = spans.len() - 1;
        let others: usize = spans[..last].iter().map(|s| s.len).sum();
        if others <= len {
            spans[last].len = len - others;
        } else {
            // (Shouldn't be: cut back to fit.)
            let mut left = len;
            for s in spans.iter_mut() {
                s.len = s.len.min(left);
                left -= s.len;
            }
            spans.retain(|s| s.len > 0);
            if spans.is_empty() {
                spans.push(Span::new(len, style::ui_font(gpui::FontWeight::NORMAL), style::PLAIN_SIZE));
            }
        }
    }
}

/// A space's width in `span`'s font and size.
fn space_width(sh: &dyn Shaper, span: &Span) -> f32 {
    let run = gpui::TextRun { len: 1, font: span.font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: Some(px(span.size)) };
    f32::from(sh.layout(" ", span.size, &[run]).width)
}

fn hash_spans(spans: &[Span], h: &mut impl Hasher) {
    for s in spans {
        s.len.hash(h);
        s.font.hash(h);
        s.size.to_bits().hash(h);
        s.color.map(|c| (c.h.to_bits(), c.s.to_bits(), c.l.to_bits(), c.a.to_bits())).hash(h);
        (s.underline, s.strike).hash(h);
        s.picture.map(|(w, ht)| (w.to_bits(), ht.to_bits())).hash(h);
    }
}

fn hash_params(p: &Params, h: &mut impl Hasher) {
    p.wrap.map(f32::to_bits).hash(h);
    p.width.to_bits().hash(h);
    p.first.to_bits().hash(h);
    p.align.hash(h);
    p.justify.hash(h);
    p.tabs.every.to_bits().hash(h);
    for t in &p.tabs.stops {
        (t.x.to_bits(), t.align).hash(h);
    }
}

fn snap(v: f32, s: f32) -> f32 {
    (v * s).round() / s
}

/// A wavy line from `x0` to `x1` along `y` (a device pixel thick, up and
/// down every two device pixels), as segments.
fn squiggle(segs: &mut Vec<(Point<Pixels>, Point<Pixels>)>, x0: f32, x1: f32, y: f32, s: f32) {
    let px1 = 1.0 / s;
    let step = 2.0 * px1;
    let mut x = x0;
    let mut k = 0;
    let mut last: Option<Point<Pixels>> = None;
    while x <= x1 {
        let p = point(px(x), px(y + if k % 2 == 0 { 1.5 * px1 } else { -0.5 * px1 }));
        if let Some(l) = last {
            segs.push((l, p));
        }
        last = Some(p);
        x += step;
        k += 1;
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

/// The desk sheets of paper lie on ([`Options::pages`]): the window's
/// ground a shade darker; in the dark, the page's half way to black (so
/// a dark sheet still stands out on it).
pub fn desk(p: &olo_theme::Palette) -> olo_theme::Rgba {
    let [r, g, b, _] = p.document_background;
    let dark = (r as u32 * 3 + g as u32 * 6 + b as u32) / 10 < 128;
    if dark { olo_theme::mix(p.document_background, [0, 0, 0, 255], 0.5) } else { olo_theme::mix(p.panel_background, [0, 0, 0, 255], 0.12) }
}

/// What changed from `old` to `new`: the bytes of `old` that went, and
/// how many bytes came in their place (what is the same at either end
/// stays).
pub fn difference(old: &str, new: &str) -> (Range<usize>, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut p = a.chunks(256).zip(b.chunks(256)).take_while(|(x, y)| x == y).count() * 256;
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

/// Back `i` off to a char boundary of `s`.
fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
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

    #[test]
    fn utf16_offsets() {
        let t = "aé😀b";
        assert_eq!(utf16_of(t, 0), 0);
        assert_eq!(utf16_of(t, 3), 2);
        assert_eq!(utf16_of(t, 7), 4);
        assert_eq!(byte_of_utf16(t, 4), 7);
        assert_eq!(byte_of_utf16("abc", 2), 2);
    }

    #[test]
    fn spans_cover_the_paragraph() {
        let f = style::ui_font(gpui::FontWeight::NORMAL);
        let mut s = vec![Span::new(3, f.clone(), 12.0), Span::new(2, f.clone(), 12.0)];
        cover(&mut s, 7);
        assert_eq!(s.iter().map(|s| s.len).collect::<Vec<_>>(), [3, 4]);
        let mut s = vec![Span::new(0, f.clone(), 12.0)];
        cover(&mut s, 0);
        assert_eq!(s.len(), 1);
        let mut s = vec![];
        cover(&mut s, 5);
        assert_eq!(s[0].len, 5);
    }
}
