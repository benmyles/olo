//! One paragraph of a document laid out: shaped by GPUI's text system a
//! paragraph at a time (its styles as runs, each with its own size), then
//! placed here, since GPUI's own wrapping knows none of what documents
//! need: rows broken at spaces (a word longer than a row, anywhere),
//! tabs to their stops (left, centred, right or on the decimal point,
//! from each row's start), a
//! picture's character as big as the picture, the first row indented,
//! rows aligned or justified. Rows are as high as what is on them (a
//! larger size, a picture), each glyph on its row's baseline.
//!
//! A [`Laid`] maps between bytes of the paragraph and points of its box
//! (the caret, the selection, a click) and paints itself; the editor
//! keeps them while their paragraph and its styles stay the same.

use std::{ops::Range, sync::Arc};

use gpui::{Bounds, Font, FontId, GlyphId, Hsla, LineLayout, Pixels, Point, TextRun, Window, point, px, size};

use crate::style;

/// One style's stretch of a paragraph, as a [`Document`](super::Document)
/// gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// Bytes of the paragraph.
    pub len: usize,
    pub font: Font,
    pub size: f32,
    /// None: the editor's text colour.
    pub color: Option<Hsla>,
    pub underline: bool,
    pub strike: bool,
    /// A picture's character ([`PICTURE`](super::PICTURE)) in this span is
    /// as wide and high as this, in points; the editor draws the picture
    /// there.
    pub picture: Option<(f32, f32)>,
}

impl Span {
    /// Plain text in `font` at `size`.
    pub fn new(len: usize, font: Font, size: f32) -> Span {
        Span { len, font, size, color: None, underline: false, strike: false, picture: None }
    }
}

/// How the text after a tab lines up with its stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabAlign {
    /// It starts at the stop.
    #[default]
    Left,
    /// It's centred on the stop.
    Center,
    /// It ends at the stop.
    Right,
    /// Its decimal point is at the stop (else its end).
    Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabStop {
    /// Points from the row's start.
    pub x: f32,
    pub align: TabAlign,
}

/// Where a paragraph's tabs go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabStops {
    /// From the start of the row, left to right.
    pub stops: Vec<TabStop>,
    /// Past the last of them (or with none), a stop every this many
    /// points; none if 0.
    pub every: f32,
}

impl TabStops {
    /// A stop every `every` points, and no others.
    pub fn every(every: f32) -> TabStops {
        TabStops { stops: Vec::new(), every }
    }

    /// The first stop past `x`.
    pub fn after(&self, x: f32) -> Option<TabStop> {
        // (Text ending at a stop, to within half a point, goes on to the
        // next.)
        const HAIR: f32 = 0.5;
        if let Some(stop) = self.stops.iter().find(|s| s.x > x + HAIR) {
            return Some(*stop);
        }
        (self.every > 0.0).then(|| TabStop { x: ((x + HAIR) / self.every).floor() * self.every + self.every, align: TabAlign::Left })
    }
}

/// Which side rows keep to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// How a paragraph is laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Rows break to fit this wide (None: one row each paragraph).
    pub wrap: Option<f32>,
    /// What rows align across (the wrap width; unwrapped, the view's).
    pub width: f32,
    /// The first row starts this far in.
    pub first: f32,
    pub align: Align,
    /// Rows but the last fill the width, their spaces wider.
    pub justify: bool,
    pub tabs: TabStops,
}

/// The ascent and descent of a face at a size, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
}

/// What lays text out: a window's text system (its lines kept a frame,
/// and the next if used again), or the app's (any thread: printing).
pub trait Shaper {
    fn layout(&self, text: &str, size: f32, runs: &[TextRun]) -> Arc<LineLayout>;
    fn metrics(&self, font: &Font, size: f32) -> Metrics;
}

impl Shaper for gpui::WindowTextSystem {
    fn layout(&self, text: &str, size: f32, runs: &[TextRun]) -> Arc<LineLayout> {
        self.layout_line(text, px(size), runs, None)
    }

    fn metrics(&self, font: &Font, size: f32) -> Metrics {
        let id = self.resolve_font(font);
        Metrics { ascent: f32::from(self.ascent(id, px(size))), descent: f32::from(self.descent(id, px(size))) }
    }
}

impl Shaper for gpui::TextSystem {
    fn layout(&self, text: &str, size: f32, runs: &[TextRun]) -> Arc<LineLayout> {
        Arc::new(self.layout_line_now(text, px(size), runs))
    }

    fn metrics(&self, font: &Font, size: f32) -> Metrics {
        let id = self.resolve_font(font);
        Metrics { ascent: f32::from(self.ascent(id, px(size))), descent: f32::from(self.descent(id, px(size))) }
    }
}

/// What a stretch of a paragraph is, for placing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Rows break after spaces, and spaces at a row's end hang past it.
    Space,
    Tab,
    Picture,
}

/// A character (or a cluster of them shaped as one, a ligature's) placed.
#[derive(Clone, Debug)]
pub struct Unit {
    /// Bytes of the paragraph.
    pub byte: usize,
    pub end: usize,
    /// Its glyphs ([`Laid::glyphs`]).
    pub glyphs: Range<u32>,
    /// From the paragraph box's left, and how far it goes.
    pub x: f32,
    pub adv: f32,
    /// Its span ([`Laid::inks`]).
    pub span: u16,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub id: GlyphId,
    pub font: FontId,
    pub size: f32,
    /// From its unit's left.
    pub dx: f32,
    pub dy: f32,
    pub emoji: bool,
}

/// What a span is drawn in.
#[derive(Clone, Copy, Debug)]
pub struct Ink {
    pub color: Option<Hsla>,
    pub underline: bool,
    pub strike: bool,
    pub size: f32,
    pub metrics: Metrics,
    /// A picture's width and height (its span's).
    pub picture: Option<(f32, f32)>,
}

/// One row of a paragraph.
#[derive(Clone, Debug)]
pub struct Row {
    /// Its units ([`Laid::units`]).
    pub units: Range<usize>,
    /// Down the paragraph's box; its baseline is `ascent` below.
    pub top: f32,
    pub ascent: f32,
    pub height: f32,
    /// Where its text starts and ends (spaces at its end not counted).
    pub left: f32,
    pub right: f32,
}

/// A paragraph laid out.
#[derive(Clone, Debug, Default)]
pub struct Laid {
    /// Its length in bytes.
    pub len: usize,
    pub units: Vec<Unit>,
    pub glyphs: Vec<Glyph>,
    pub inks: Vec<Ink>,
    pub rows: Vec<Row>,
    /// Its rows' widths added up (as wide as it would be unwrapped).
    pub natural: f32,
    /// Each row's glyphs as last painted, to paint again from (scrolled,
    /// they are the same a whole number of pixels away).
    painted: Painted,
}

/// Rows' glyph runs (`Laid::painted`).
#[derive(Clone, Default)]
struct Painted(std::cell::RefCell<Vec<gpui::GlyphRun>>);

impl std::fmt::Debug for Painted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Painted")
    }
}

/// Points per pixel, for the caret and lines to fall on pixels.
fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// Lays out paragraph `text` styled by `spans` (covering it, in order).
pub fn lay_out(shaper: &dyn Shaper, text: &str, spans: &[Span], p: &Params) -> Laid {
    let mut laid = shape(shaper, text, spans);
    laid.place(text, p);
    laid
}

/// Paragraph `text` styled by `spans` shaped, not yet placed in rows:
/// what doesn't change with the width it's set in ([`Laid::placed`]
/// sets it in one).
pub fn shape(shaper: &dyn Shaper, text: &str, spans: &[Span]) -> Laid {
    let first_size = spans.first().map_or(style::PLAIN_SIZE, |s| s.size);
    let mut inks: Vec<Ink> =
        spans.iter().map(|s| Ink { color: s.color, underline: s.underline, strike: s.strike, size: s.size, metrics: shaper.metrics(&s.font, s.size), picture: s.picture }).collect();
    if inks.is_empty() {
        let font = style::ui_font(gpui::FontWeight::NORMAL);
        inks.push(Ink { color: None, underline: false, strike: false, size: first_size, metrics: shaper.metrics(&font, first_size), picture: None });
    }
    let runs: Vec<TextRun> = spans
        .iter()
        .filter(|s| s.len > 0)
        .map(|s| TextRun { len: s.len, font: s.font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None, size: Some(px(s.size)) })
        .collect();
    // Each span's bytes, to find a byte's span.
    let mut starts = Vec::with_capacity(spans.len());
    let mut at = 0;
    for s in spans {
        starts.push(at);
        at += s.len;
    }
    let span_of = |byte: usize| -> usize { starts.partition_point(|&s| s <= byte).saturating_sub(1) };

    // The shaped glyphs, in order, and the units they make.
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut units: Vec<Unit> = Vec::new();
    if !text.is_empty() {
        let line = shaper.layout(text, first_size, &runs);
        // (index, x) of every glyph, and its glyph.
        let mut flat: Vec<(usize, f32, Glyph)> = Vec::new();
        for run in &line.runs {
            for g in &run.glyphs {
                flat.push((g.index, f32::from(g.position.x), Glyph { id: g.id, font: run.font_id, size: f32::from(run.font_size), dx: 0.0, dy: f32::from(g.position.y), emoji: g.is_emoji }));
            }
        }
        flat.sort_by_key(|f| f.0);
        let width = f32::from(line.width);
        let mut k = 0;
        while k < flat.len() {
            let byte = flat[k].0;
            let x0 = flat[k].1;
            let mut j = k + 1;
            while j < flat.len() && flat[j].0 == byte {
                j += 1;
            }
            let end = flat.get(j).map_or(text.len(), |f| f.0);
            let next_x = flat.get(j).map_or(width, |f| f.1);
            let g0 = glyphs.len() as u32;
            for f in &flat[k..j] {
                glyphs.push(Glyph { dx: f.1 - x0, ..f.2 });
            }
            let ch = text[byte..].chars().next().unwrap_or(' ');
            let span = span_of(byte);
            let picture = spans.get(span).and_then(|s| s.picture).filter(|_| ch == super::PICTURE);
            let kind = match ch {
                '\t' => Kind::Tab,
                _ if picture.is_some() => Kind::Picture,
                c if c.is_whitespace() && c != '\u{a0}' => Kind::Space,
                _ => Kind::Text,
            };
            let adv = match picture {
                Some((w, _)) => w,
                None => (next_x - x0).max(0.0),
            };
            units.push(Unit { byte, end, glyphs: g0..glyphs.len() as u32, x: 0.0, adv, span: span.min(u16::MAX as usize) as u16, kind });
            k = j;
        }
        // Bytes before the first glyph (none, but for safety) join it.
        if let Some(u) = units.first_mut() {
            u.byte = 0;
        }
    }

    Laid { len: text.len(), units, glyphs, inks, rows: Vec::new(), natural: 0.0, painted: Painted::default() }
}

impl Laid {
    /// This shaped paragraph ([`shape`]), of `text`, placed in rows by
    /// `p`: shaped once, set again at each width (a window being resized).
    pub fn placed(&self, text: &str, p: &Params) -> Laid {
        let mut laid = Laid { len: self.len, units: self.units.clone(), glyphs: self.glyphs.clone(), inks: self.inks.clone(), rows: Vec::new(), natural: 0.0, painted: Painted::default() };
        laid.place(text, p);
        laid
    }

    /// Places the units in rows: wrapping, tabs, alignment, justification.
    fn place(&mut self, text: &str, p: &Params) {
        let n = self.units.len();
        let mut rows: Vec<Row> = Vec::new();
        let mut start = 0;
        let mut first = true;
        loop {
            // One row from `start`: units placed until one doesn't fit.
            let indent = if first { p.first } else { 0.0 };
            let mut pen = indent;
            // Where the row may break: before unit `k` (after spaces).
            let mut brk: Option<usize> = None;
            let mut end = n;
            let mut i = start;
            while i < n {
                let kind = self.units[i].kind;
                if kind == Kind::Tab {
                    let to = self.tab_to(text, i, pen, &p.tabs);
                    self.units[i].adv = (to - pen).max(0.0);
                }
                self.units[i].x = pen;
                let adv = self.units[i].adv;
                if let Some(w) = p.wrap
                    && kind != Kind::Space
                    && i > start
                    && pen + adv > w + 0.01
                {
                    end = brk.unwrap_or(i);
                    break;
                }
                pen += adv;
                // A break after spaces (before what follows them), or
                // after a hyphen.
                let next_is_space = self.units.get(i + 1).is_some_and(|u| u.kind == Kind::Space);
                if (kind == Kind::Space && !next_is_space) || (kind == Kind::Text && text[self.units[i].byte..].starts_with('-')) {
                    brk = Some(i + 1);
                }
                i += 1;
            }
            rows.push(Row { units: start..end, top: 0.0, ascent: 0.0, height: 0.0, left: indent, right: indent });
            first = false;
            if end >= n {
                break;
            }
            start = end;
        }
        // Each row's extent, alignment and justification.
        let last = rows.len() - 1;
        for (r, row) in rows.iter_mut().enumerate() {
            let us = &self.units[row.units.clone()];
            let left = us.first().map_or(row.left, |u| u.x);
            let right = us.iter().rev().find(|u| u.kind != Kind::Space).map_or(left, |u| u.x + u.adv);
            row.left = left;
            row.right = right;
            let room = p.width - right;
            let justify = p.justify && p.wrap.is_some() && r < last && room > 0.0;
            if justify {
                // The spaces after the row's last tab, before its last word.
                let from = us.iter().rposition(|u| u.kind == Kind::Tab).map_or(0, |k| k + 1);
                let to = us.iter().rposition(|u| u.kind != Kind::Space).map_or(0, |k| k + 1);
                let spaces = us[from.min(to)..to].iter().filter(|u| u.kind == Kind::Space).count();
                if spaces > 0 {
                    let extra = room / spaces as f32;
                    let mut by = 0.0;
                    for k in row.units.start + from..row.units.end {
                        self.units[k].x += by;
                        if k < row.units.start + to && self.units[k].kind == Kind::Space {
                            self.units[k].adv += extra;
                            by += extra;
                        }
                    }
                    row.right += by;
                }
                continue;
            }
            let shift = match p.align {
                Align::Left => 0.0,
                Align::Center => (room / 2.0).max(0.0),
                Align::Right => room.max(0.0),
            };
            if shift != 0.0 {
                for u in &mut self.units[row.units.clone()] {
                    u.x += shift;
                }
                row.left += shift;
                row.right += shift;
            }
        }
        // Heights: what's on each row, its baseline below the tallest.
        let mut y = 0.0;
        let base = self.inks[0].metrics;
        for row in &mut rows {
            let (mut ascent, mut descent) = (0.0f32, 0.0f32);
            for u in &self.units[row.units.clone()] {
                match u.kind {
                    Kind::Picture => ascent = ascent.max(self.picture_height(u)),
                    _ => {
                        let m = self.inks[u.span as usize].metrics;
                        ascent = ascent.max(m.ascent);
                        descent = descent.max(m.descent);
                    }
                }
            }
            if row.units.is_empty() {
                let m = self.units.get(row.units.start.saturating_sub(1)).map_or(base, |u| self.inks[u.span as usize].metrics);
                ascent = m.ascent;
                descent = m.descent;
            }
            if descent == 0.0 {
                descent = base.descent;
            }
            row.top = y;
            row.ascent = ascent;
            row.height = ascent + descent;
            y += row.height;
        }
        self.natural = rows.iter().map(|r| r.right - r.left).sum();
        self.rows = rows;
    }

    fn picture_height(&self, u: &Unit) -> f32 {
        self.inks[u.span as usize].picture.map_or(0.0, |p| p.1)
    }

    /// Where tab `i` (with the row's text before it ending at `pen`) takes
    /// the text after it.
    fn tab_to(&self, text: &str, i: usize, pen: f32, tabs: &TabStops) -> f32 {
        let Some(stop) = tabs.after(pen) else { return pen };
        // What it lines up: the text after it, up to the next tab.
        let after = &self.units[i + 1..];
        let stretch = after.iter().position(|u| u.kind == Kind::Tab).map_or(after, |k| &after[..k]);
        let width: f32 = stretch.iter().map(|u| u.adv).sum();
        let lead = match stop.align {
            TabAlign::Left => 0.0,
            TabAlign::Center => width / 2.0,
            TabAlign::Right => width,
            TabAlign::Decimal => {
                let mut w = 0.0;
                let mut found = None;
                for u in stretch {
                    if text[u.byte..].starts_with('.') {
                        found = Some(w);
                        break;
                    }
                    w += u.adv;
                }
                found.unwrap_or(width)
            }
        };
        (stop.x - lead).max(pen)
    }

    /// How high it is, its rows added up.
    pub fn height(&self) -> f32 {
        self.rows.last().map_or(0.0, |r| r.top + r.height)
    }

    /// How wide its widest row is (to its text's end).
    pub fn width(&self) -> f32 {
        self.rows.iter().map(|r| r.right).fold(0.0, f32::max)
    }

    /// The first byte of row `r`.
    pub fn row_start(&self, r: usize) -> usize {
        let row = &self.rows[r];
        self.units.get(row.units.start).filter(|_| !row.units.is_empty()).map_or_else(|| if r == 0 { 0 } else { self.row_end(r - 1) }, |u| u.byte)
    }

    /// The byte after row `r` (its last unit's end).
    pub fn row_end(&self, r: usize) -> usize {
        let row = &self.rows[r];
        if row.units.is_empty() {
            return self.row_start(r);
        }
        if r + 1 == self.rows.len() { self.len } else { self.units[row.units.end - 1].end }
    }

    /// The row byte `at` shows on: at a wrap, the end of the row before
    /// unless `next` (the start of the one after).
    pub fn row_of(&self, at: usize, next: bool) -> usize {
        let mut r = self.rows.partition_point(|row| !row.units.is_empty() && self.units[row.units.start].byte <= at).saturating_sub(1);
        if !next && r > 0 && at == self.row_start(r) {
            r -= 1;
        }
        r
    }

    /// Where byte `at` is across row `r` (the caret's x).
    pub fn x_in_row(&self, r: usize, at: usize) -> f32 {
        let row = &self.rows[r];
        let us = &self.units[row.units.clone()];
        for u in us {
            if at <= u.byte {
                return u.x;
            }
            if at < u.end {
                // Inside a cluster: a share of it by characters.
                let all = u.end - u.byte;
                return u.x + u.adv * (at - u.byte) as f32 / all.max(1) as f32;
            }
        }
        us.last().map_or(row.left, |u| u.x + u.adv)
    }

    /// The byte nearest `(x, y)` in the paragraph's box, and whether it is
    /// at the start of a row (a wrap's, not the end of the row before).
    pub fn byte_at(&self, x: f32, y: f32) -> (usize, bool) {
        let r = self.rows.partition_point(|row| row.top + row.height <= y).min(self.rows.len().saturating_sub(1));
        self.byte_in_row(r, x)
    }

    /// The byte nearest `x` on row `r`.
    pub fn byte_in_row(&self, r: usize, x: f32) -> (usize, bool) {
        let Some(row) = self.rows.get(r) else { return (0, false) };
        for (k, u) in self.units[row.units.clone()].iter().enumerate() {
            if x < u.x + u.adv / 2.0 {
                return (u.byte, k == 0 && r > 0);
            }
        }
        // Past the row's end: its end (at a wrap, before the next row).
        (self.row_end(r), false)
    }

    /// The rectangles bytes `range` cover, a row each (in the paragraph's
    /// box); `past`: it goes on past the paragraph, so its last row's end
    /// shows a sliver (the paragraph's newline).
    pub fn range_rects(&self, range: Range<usize>, past: bool) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            let (a, b) = (self.row_start(r), self.row_end(r));
            let last = r + 1 == self.rows.len();
            // A row's end counts when the range goes on past it.
            let on = if last { past } else { range.end > b };
            if range.start > b || range.end < a || (range.start == b && !on) {
                continue;
            }
            let x0 = self.x_in_row(r, range.start.max(a));
            let x1 = if on {
                let end = self.x_in_row(r, b);
                if last { end + row.height * 0.5 } else { end }
            } else {
                self.x_in_row(r, range.end.min(b))
            };
            if x1 > x0 {
                out.push((x0, row.top, x1, row.top + row.height));
            }
        }
        out
    }

    /// Its pictures: each one's byte and rectangle (in the box).
    pub fn pictures(&self) -> impl Iterator<Item = (usize, (f32, f32, f32, f32))> + '_ {
        self.rows.iter().flat_map(move |row| {
            self.units[row.units.clone()].iter().filter(|u| u.kind == Kind::Picture).map(move |u| {
                let h = self.picture_height(u);
                let base = row.top + row.ascent;
                (u.byte, (u.x, base - h, u.x + u.adv, base))
            })
        })
    }

    /// Paints it with its box's top left at `origin`, the text in `color`
    /// where its spans have none. Rows above `clip_top` or below
    /// `clip_bottom` (window coordinates) are passed over.
    pub fn paint(&self, window: &mut Window, origin: Point<Pixels>, color: Hsla, clip: (f32, f32)) {
        let s = window.scale_factor();
        let (ox, oy) = (f32::from(origin.x), f32::from(origin.y));
        // (What the glyphs were found for: the text's own colour.)
        let stamp = (u64::from(color.h.to_bits()) << 32 | u64::from(color.s.to_bits())) ^ (u64::from(color.l.to_bits()) << 16) ^ u64::from(color.a.to_bits()).rotate_left(48);
        let mut painted = self.painted.0.borrow_mut();
        painted.resize_with(self.rows.len(), Default::default);
        for (r, row) in self.rows.iter().enumerate() {
            if oy + row.top + row.height < clip.0 || oy + row.top > clip.1 {
                continue;
            }
            let base = snap(oy + row.top + row.ascent, s);
            let us = &self.units[row.units.clone()];
            let drawn = |u: &&Unit| !matches!(u.kind, Kind::Tab | Kind::Picture | Kind::Space);
            // The row's glyphs in one go, from where they were found last
            // time; emoji (colour sprites) each.
            window.paint_glyph_run(&mut painted[r], point(px(ox), px(base)), stamp, |glyph| {
                for u in us.iter().filter(drawn) {
                    let c = self.inks[u.span as usize].color.unwrap_or(color);
                    for g in self.glyphs[u.glyphs.start as usize..u.glyphs.end as usize].iter().filter(|g| !g.emoji) {
                        glyph(point(px(u.x + g.dx), px(g.dy)), g.font, g.id, px(g.size), c);
                    }
                }
            });
            for u in us.iter().filter(drawn) {
                for g in self.glyphs[u.glyphs.start as usize..u.glyphs.end as usize].iter().filter(|g| g.emoji) {
                    let _ = window.paint_emoji(point(px(ox + u.x + g.dx), px(base + g.dy)), g.font, g.id, px(g.size));
                }
            }
            // Underlines and strikeouts, a stretch of the same at a time.
            let mut k = 0;
            while k < us.len() {
                let ink = self.inks[us[k].span as usize];
                if !(ink.underline || ink.strike) || us[k].kind == Kind::Picture {
                    k += 1;
                    continue;
                }
                let mut j = k + 1;
                while j < us.len() && us[j].span == us[k].span {
                    j += 1;
                }
                // (Not under the spaces a row ends with.)
                let mut e = j;
                while e > k + 1 && us[e - 1].kind == Kind::Space {
                    e -= 1;
                }
                let (x0, x1) = (ox + us[k].x, ox + us[e - 1].x + us[e - 1].adv);
                let c = ink.color.unwrap_or(color);
                let thick = snap((ink.size / 14.0).max(1.0 / s), s).max(1.0 / s);
                if ink.underline {
                    let y = snap(base + (ink.size * 0.12).max(1.0), s);
                    style::fill_rect(window, Bounds::new(point(px(x0), px(y)), size(px(x1 - x0), px(thick))), c);
                }
                if ink.strike {
                    let y = snap(base - ink.metrics.ascent * 0.3, s);
                    style::fill_rect(window, Bounds::new(point(px(x0), px(y)), size(px(x1 - x0), px(thick))), c);
                }
                k = j;
            }
        }
    }

    /// Rows moved down where they'd cross `foot` (a page's text bottom, in
    /// the paragraph's box), each to `next` (the next page's text top):
    /// how much taller it got. `foots` gives the foot and next top for any
    /// y down the box.
    pub fn break_rows(&mut self, foots: impl Fn(f32) -> (f32, f32)) -> f32 {
        let mut shift = 0.0;
        for (r, row) in self.rows.iter_mut().enumerate() {
            let y = row.top + shift;
            let (foot, next) = foots(y);
            if r > 0 && y + row.height > foot + 0.5 {
                shift += next - y;
            }
            row.top += shift;
        }
        shift
    }
}

/// One line of text in `span`'s style, unwrapped (a list's marker).
pub fn line(shaper: &dyn Shaper, text: &str, span: &Span) -> Laid {
    let p = Params { wrap: None, width: 0.0, first: 0.0, align: Align::Left, justify: false, tabs: TabStops::default() };
    lay_out(shaper, text, &[Span { len: text.len(), ..span.clone() }], &p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontStyle, FontWeight, TextSystem};

    fn params() -> Params {
        Params { wrap: Some(200.0), width: 200.0, first: 0.0, align: Align::Left, justify: false, tabs: TabStops::every(36.0) }
    }

    #[test]
    fn styles_are_drawn_in_their_faces() {
        let ts = TextSystem::standalone();
        let text = "word word word";
        let regular = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let italic = Font { style: FontStyle::Italic, ..regular.clone() };
        let spans = [Span::new(5, regular, 12.0), Span::new(5, bold, 12.0), Span::new(4, italic, 12.0)];
        let l = lay_out(&*ts, text, &spans, &params());
        let face = |byte: usize| l.units.iter().find(|u| u.byte == byte).map(|u| l.glyphs[u.glyphs.start as usize].font).unwrap();
        let width = |from: usize, to: usize| l.units.iter().filter(|u| u.byte >= from && u.byte < to).map(|u| u.adv).sum::<f32>();
        assert_ne!(face(0), face(5), "bold in its own face");
        assert_ne!(face(0), face(10), "italic in its own face");
        assert!(width(5, 9) > width(0, 4), "bold is wider: {} {}", width(5, 9), width(0, 4));
    }

    #[test]
    fn rows_wrap_at_spaces_and_tabs_go_to_stops() {
        let ts = TextSystem::standalone();
        let font = style::ui_font(FontWeight::NORMAL);
        let text = "a\tb ".repeat(20);
        let l = lay_out(&*ts, &text, &[Span::new(text.len(), font, 12.0)], &params());
        assert!(l.rows.len() > 1);
        for r in &l.rows {
            assert!(r.right <= 200.5, "{}", r.right);
        }
        // The text after the first tab starts at the first stop.
        let b = l.units.iter().find(|u| u.byte == 2).unwrap();
        assert!((b.x - 36.0).abs() < 0.01, "{}", b.x);
        // Every row starts at the left.
        assert!(l.rows.iter().all(|r| r.left.abs() < 0.01));
    }
}
