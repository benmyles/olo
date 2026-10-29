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
//! Shift extends. A press on the selection drags its text (as
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
    /// Replace bytes `range` with `text` (typing, deleting, pasting, a
    /// drop); the selection was `selection` (for undo).
    fn replace(&mut self, range: Range<usize>, text: &str, selection: Range<usize>);
    /// Bytes `range` were copied or cut (to paste them back with styles).
    fn copied(&mut self, _range: Range<usize>) {}
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
}

impl Default for Options {
    fn default() -> Self {
        Options { editable: true, wrap: true, align: Align::Min, justify: false, margin: vec2(m::TEXT_INSET, 4.0), background: None }
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
}

/// Never laid out: its size is a guess from its length.
const NEVER: u64 = 0;
/// Laid out, but not as it is now (restyled): its size is a good guess.
const STALE: u64 = 1;

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
}

impl Editor {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug) -> Editor {
        let mut e = Editor {
            id: Id::unique(id),
            paras: Vec::new(),
            tops: Vec::new(),
            tops_dirty: true,
            widest: 0.0,
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
        };
        e.reset("");
        e
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
        let mut p = Para { start, len, height: 0.0, natural: len as f32 * self.char_w, row_h: self.row_h, key: NEVER };
        p.height = self.estimate(&p);
        p
    }

    fn estimate(&self, p: &Para) -> f32 {
        let rows = match self.params.and_then(|p| p.0) {
            Some(w) if w > 0.0 => (p.natural / w).ceil().max(1.0),
            _ => 1.0,
        };
        rows * p.row_h
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
    /// measured.
    fn galley(&mut self, ui: &Ui, doc: &mut dyn Document, i: usize) -> Arc<Galley> {
        let (wrap, _, align, justify) = self.params.unwrap_or((None, 1.0, Align::Min, false));
        let p = self.paras[i];
        let range = p.start..p.start + p.len;
        let mut job = LayoutJob { text: doc.text()[range.clone()].to_owned(), halign: align, justify, keep_trailing_whitespace: true, ..Default::default() };
        job.wrap.max_width = wrap.unwrap_or(f32::INFINITY);
        doc.sections(range, &mut job);
        if job.sections.is_empty() {
            job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(p.len), format: TextFormat::default() });
        }
        let g = ui.fonts_mut(|f| f.layout_job(job));
        if p.key != self.key {
            self.measured(i, &g);
        }
        g
    }

    fn measured(&mut self, i: usize, g: &Galley) {
        let wrapped = self.params.is_some_and(|p| p.0.is_some());
        let rows = g.rows.len().max(1) as f32;
        let p = &mut self.paras[i];
        let first = p.key == NEVER;
        let h = g.size().y;
        if p.height != h {
            self.tops_dirty = true;
        }
        p.height = h;
        p.natural = if wrapped { g.rows.iter().map(|r| r.size.x).sum() } else { g.size().x };
        p.row_h = h / rows;
        p.key = self.key;
        let p = *p;
        if first && p.len > 0 {
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

    /// The x of the galleys' origin from the text's left (alignment).
    fn shift(&self, inner: f32) -> f32 {
        let (wrap, _, align, _) = self.params.unwrap_or((None, 1.0, Align::Min, false));
        let w = wrap.unwrap_or(inner.max(self.widest));
        match align {
            Align::Min => 0.0,
            Align::Center => w / 2.0,
            Align::Max => w,
        }
    }

    /// Paragraph `i`'s galley origin in content coordinates.
    fn origin(&self, i: usize, o: &Options, inner: f32) -> Vec2 {
        vec2(o.margin.x + self.shift(inner), o.margin.y + self.tops[i])
    }

    /// The byte at content point `pos` (and whether it is at the start of
    /// a wrapped row rather than the end of the one before).
    fn byte_at(&mut self, ui: &Ui, doc: &mut dyn Document, o: &Options, inner: f32, pos: Pos2) -> (usize, bool) {
        self.ensure_tops();
        let i = self.para_at_y(pos.y - o.margin.y);
        let g = self.galley(ui, doc, i);
        self.ensure_tops();
        let c = g.cursor_from_pos(pos.to_vec2() - self.origin(i, o, inner));
        let p = self.paras[i];
        (p.start + byte_of_col(&doc.text()[p.start..p.start + p.len], c.index.0), c.prefer_next_row)
    }

    /// Byte `at`'s paragraph, galley and cursor in it.
    fn cursor(&mut self, ui: &Ui, doc: &mut dyn Document, at: usize, prefer_next: bool) -> (usize, Arc<Galley>, CCursor) {
        let i = self.para_of(at);
        let g = self.galley(ui, doc, i);
        let p = self.paras[i];
        let col = doc.text()[p.start..at.clamp(p.start, p.start + p.len)].chars().count();
        (i, g, CCursor { index: CharIndex(col), prefer_next_row: prefer_next })
    }

    /// The caret's rectangle (zero wide) in content coordinates.
    fn caret_rect(&mut self, ui: &Ui, doc: &mut dyn Document, o: &Options, inner: f32, at: usize, prefer_next: bool) -> Rect {
        let (i, g, c) = self.cursor(ui, doc, at, prefer_next);
        self.ensure_tops();
        g.pos_from_cursor(c).translate(self.origin(i, o, inner))
    }

    /// The byte a row up or down from the caret, keeping to `h_pos`.
    fn vertical(&mut self, ui: &Ui, doc: &mut dyn Document, down: bool) -> (usize, bool) {
        let (i, g, c) = self.cursor(ui, doc, self.caret, self.prefer_next);
        let lc = g.layout_from_cursor(c);
        let h = *self.h_pos.get_or_insert_with(|| g.pos_from_cursor(c).center().x);
        let (pi, gal, row) = if down && lc.row + 1 < g.rows.len() {
            (i, g, lc.row + 1)
        } else if !down && lc.row > 0 {
            (i, g, lc.row - 1)
        } else if down && i + 1 < self.paras.len() {
            (i + 1, self.galley(ui, doc, i + 1), 0)
        } else if !down && i > 0 {
            let g = self.galley(ui, doc, i - 1);
            let last = g.rows.len().saturating_sub(1);
            (i - 1, g, last)
        } else {
            // Past the first or last row: to the very start or end.
            let to = if down { doc.text().len() } else { 0 };
            return (to, false);
        };
        let Some(r) = gal.rows.get(row) else { return (self.paras[pi].start, false) };
        let column = r.char_at(h - r.pos.x).0.min(r.char_count_excluding_newline().0);
        let col: usize = gal.rows[..row].iter().map(|r| r.char_count_including_newline().0).sum::<usize>() + column;
        let p = self.paras[pi];
        (p.start + byte_of_col(&doc.text()[p.start..p.start + p.len], col), column < r.char_count_excluding_newline().0)
    }

    /// The start or end of the caret's row.
    fn row_end(&mut self, ui: &Ui, doc: &mut dyn Document, end: bool) -> (usize, bool) {
        let (i, g, c) = self.cursor(ui, doc, self.caret, self.prefer_next);
        let c = if end { g.cursor_end_of_row(&c) } else { g.cursor_begin_of_row(&c) };
        let p = self.paras[i];
        (p.start + byte_of_col(&doc.text()[p.start..p.start + p.len], c.index.0), c.prefer_next_row)
    }

    fn edit(&mut self, doc: &mut dyn Document, range: Range<usize>, text: &str) {
        let sel = self.selection();
        doc.replace(range.clone(), text, sel);
        self.replaced(doc.text(), range.clone(), text.len());
        self.anchor = range.start + text.len();
        self.caret = self.anchor;
        self.prefer_next = false;
        self.h_pos = None;
        self.reveal = true;
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
        let mut shown: Vec<(usize, Arc<Galley>)> = Vec::new();
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
                let g = self.galley(ui, doc, i);
                shown.push((i, g));
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
        for (i, g) in &shown {
            let p = self.paras[*i];
            let at = snap(origin + self.origin(*i, o, inner));
            let end = p.start + p.len;
            // The selection behind the text (its newline too, as a sliver).
            if !sel.is_empty() && sel.start <= end && sel.end > p.start {
                let para = &text[p.start..end];
                let a = para[..sel.start.clamp(p.start, end) - p.start].chars().count();
                let b = para[..sel.end.clamp(p.start, end) - p.start].chars().count();
                let past = sel.end > end;
                let mut c0 = 0;
                for (ri, row) in g.rows.iter().enumerate() {
                    let n = row.char_count_excluding_newline().0;
                    let last = ri + 1 == g.rows.len();
                    // On past this row's end: to its edge (and the
                    // paragraph's newline, a sliver after the last).
                    let on = if last { past } else { b > c0 + n };
                    if a <= c0 + n && b >= c0 {
                        let left = row.x_offset(CharIndex(a.max(c0) - c0));
                        let right = if on { row.size.x + if last { row.size.y * 0.5 } else { 0.0 } } else { row.x_offset(CharIndex(b.min(c0 + n) - c0)) };
                        if right > left {
                            let r = Rect::from_min_max(snap(at + row.pos.to_vec2() + vec2(left, 0.0)), snap(at + row.pos.to_vec2() + vec2(right, row.size.y)));
                            painter.rect_filled(r, 0.0, fill);
                        }
                    }
                    c0 += n;
                }
            }
            painter.galley(at, g.clone(), visuals.text_color());
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
            self.galley(ui, doc, i);
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
                self.edit(doc, sel, "\n");
                return true;
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
                to = Some(self.vertical(ui, doc, key == K::ArrowDown));
                keep_h = true;
            }
            K::Home | K::End if mods.ctrl => to = Some((if key == K::Home { 0 } else { text_len }, false)),
            K::Home | K::End => to = Some(self.row_end(ui, doc, key == K::End)),
            K::PageUp | K::PageDown => {
                // A view's height up or down, the view going along.
                let r = self.caret_rect(ui, doc, o, inner, self.caret, self.prefer_next);
                let h = *self.h_pos.get_or_insert(r.center().x - o.margin.x - self.shift(inner));
                let dy = if key == K::PageUp { -vp.height() } else { vp.height() };
                let target = Pos2::new(o.margin.x + self.shift(inner) + h, r.center().y + dy);
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
