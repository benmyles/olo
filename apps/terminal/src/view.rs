//! Drawing the terminal grid and turning keys and the mouse into bytes for
//! the program inside.
//!
//! Every cell sits on a grid snapped to device pixels: each character's
//! glyph is placed at its own cell (wide characters centred in their two),
//! from whichever font has it; box-drawing and block characters are drawn
//! as shapes so lines join seamlessly.
//!
//! Each row is worked out once (`RowCache`): a frame compares each row's
//! cells with those it was drawn from and lays out again only rows that
//! changed (an agent's spinner, a line typed); the rest are painted as
//! they were, their glyphs straight from where the atlas had them
//! (`Window::paint_glyph_run`), so a frame neither resolves colours, lays
//! out nor looks up glyphs for the screen again. A row laid out is the
//! same wherever it is, so when the screen scrolls (output, the
//! scrollback) each row is found where it was: only the rows new to the
//! screen are laid out. The cursor, the hovered link and pictures are
//! drawn live over them.

use std::{
    cell::{Cell as StdCell, RefCell},
    collections::HashMap,
    sync::Arc,
};

use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionRange, SelectionType},
    term::{
        TermDamage, TermMode,
        cell::{Cell, Flags},
        color::COUNT,
    },
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use olo_ui::{
    gpui::{self, App, Bounds, ContentMask, FontId, FontWeight, GlyphId, GlyphRun, Hsla, Keystroke, Modifiers, PathBuilder, Pixels, RenderImage, TextRun, Window, point, px, size},
    style,
};

use crate::{
    colors::{Scheme, mix},
    graphics, links,
    session::{Session, Size, Source},
};

/// An emulator colour as GPUI's.
pub fn hsla(c: Rgb) -> Hsla {
    style::c([c.r, c.g, c.b, 255])
}

/// Space between the window edge and the text, in logical pixels: across,
/// a window's inset, so the text is under the menu bar's first title and
/// the tabs' icons; less down, as a cell's line box already has some.
pub const PAD: (f32, f32) = (olo_kit_base::metrics::WINDOW_INSET, 6.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub size: f32,
    /// Cell size, a whole number of device pixels.
    pub cell: (f32, f32),
    /// The font's own advance.
    advance: f32,
    /// A cell's top to its baseline, on a device pixel.
    baseline: f32,
    /// The font's line box (ascent and descent).
    row: f32,
    pub scale: f32,
}

impl Metrics {
    pub fn new(window: &Window, size: f32) -> Metrics {
        let scale = window.scale_factor();
        let ts = window.text_system();
        let id = ts.resolve_font(&style::mono_font(FontWeight::NORMAL));
        let fs = px(size);
        let advance = ts.advance(id, fs, 'M').map_or(size * 0.6, |a| f32::from(a.width));
        let ascent = f32::from(ts.ascent(id, fs));
        let descent = f32::from(ts.descent(id, fs)).abs();
        let row = ascent + descent;
        let snap = |v: f32| (v * scale).ceil() / scale;
        let cell = (snap(advance), snap(row));
        // The font's line box centred in the (slightly taller) cell, its
        // baseline on a device pixel.
        let baseline = (((cell.1 - row) / 2.0 + ascent) * scale).round() / scale;
        Metrics { size, cell, advance, baseline, row, scale }
    }

    /// Cell size in device pixels, for the PTY's pixel-size report.
    pub fn device_cell(&self) -> (u16, u16) {
        ((self.cell.0 * self.scale).round() as u16, (self.cell.1 * self.scale).round() as u16)
    }

    /// How many cells fit in `w`×`h`.
    pub fn grid_size(&self, w: f32, h: f32) -> Size {
        Size { cols: (((w - 2.0 * PAD.0) / self.cell.0).floor() as usize).max(2), rows: (((h - 2.0 * PAD.1) / self.cell.1).floor() as usize).max(1) }
    }

    /// Content size for a `cols`×`rows` window.
    pub fn content_size(&self, cols: usize, rows: usize) -> (f32, f32) {
        ((cols as f32 * self.cell.0 + 2.0 * PAD.0).ceil(), (rows as f32 * self.cell.1 + 2.0 * PAD.1).ceil())
    }
}

/// A screen's background and text colours as it draws them (a program
/// may have changed them).
pub fn screen_colors(s: &Session, scheme: &Scheme) -> (Hsla, Hsla) {
    // (Its own while being reflowed, the scheme's.)
    let term = if s.reflowing() { s.term.try_lock_unfair() } else { Some(s.term.lock()) };
    let scheme_only = alacritty_terminal::term::color::Colors::default();
    let colors = term.as_ref().map_or(&scheme_only, |t| t.colors());
    let named = |n| hsla(scheme.resolve(Color::Named(n), colors));
    (named(NamedColor::Background), named(NamedColor::Foreground))
}

/// Mouse state between events.
#[derive(Default)]
pub struct Mouse {
    /// When, where and how many times the button last went down.
    last_click: Option<(std::time::Instant, Point, u8)>,
    selecting: bool,
    /// Button held while reporting to the program (for drag reports).
    reporting: Option<u8>,
    last_cell: Option<Point>,
    /// Scroll that hasn't added up to a whole line yet.
    scroll: f32,
    /// The link under the pointer while ⌘ is held: its cells and target.
    pub hover: Option<Hover>,
}

/// What a screen keeps between frames for drawing: its rows as last laid
/// out, its pictures, where it is.
#[derive(Default)]
pub struct Pane {
    rows: RefCell<RowCache>,
    /// Pictures by placement, with the image generation and source they
    /// were made from.
    images: RefCell<HashMap<u32, (u64, [u32; 4], Arc<RenderImage>)>>,
    /// Where the screen was last drawn, in the window.
    pub bounds: StdCell<Bounds<Pixels>>,
    /// The frame drawn while the screen is being reflowed (`Session::resize`).
    last: RefCell<Option<Frame>>,
    /// The scrollback scrolled by the point, and stretched past its ends.
    smooth: RefCell<Smooth>,
    /// How far the rows were drawn below their places (part of the row
    /// above showing, or stretched past an end), for the mouse's cells.
    pub shift: StdCell<f32>,
}

/// The normal screen's scrollback scrolled by the point, as everything
/// else that scrolls: fingers, their glide and a wheel's eased notches
/// move it as far as they go (not a line at a time), past its ends against
/// GPUI's rubber band (`Elastic`), springing back. alacritty's display
/// offset is its whole rows; the rest shifts the rows drawn.
#[derive(Default)]
struct Smooth {
    elastic: gpui::Elastic,
    /// How far up the scrollback, in points (0: at its end, the live
    /// screen).
    pos: f32,
    /// The display offset `pos` last set: if the terminal's is another
    /// (output came, typing scrolled it down) that's where it is.
    rows: usize,
}

impl Smooth {
    /// In step with the terminal's display offset (`display`), its ends
    /// for `history` rows above a view `view` tall, rows `ch` tall: where
    /// it's scrolled to as GPUI has it (from minus its end to zero).
    fn sync(&mut self, display: usize, history: usize, view: f32, ch: f32) -> (gpui::Point<Pixels>, f32) {
        if display != self.rows {
            self.pos = display as f32 * ch;
            self.rows = display;
        }
        let max = history as f32 * ch;
        self.pos = self.pos.clamp(0.0, max);
        self.elastic.set_extent(size(px(0.0), px(max)), size(px(1.0), px(view)));
        (point(px(0.0), px(self.pos - max)), max)
    }
}

/// The screen's rows as last laid out, each with the cells (and
/// selection) it was laid out from. Dropped whole when anything they were
/// laid out for changes (`RowKey`).
#[derive(Default)]
struct RowCache {
    key: Option<RowKey>,
    rows: Vec<Option<CachedRow>>,
    /// Each style's font and its glyphs for printable ASCII, as found
    /// (None: not looked for yet; Some(None): the font hasn't it).
    ascii: Vec<(Style, FontId, Box<[Option<Option<GlyphId>>; 95]>)>,
    /// Where the screen's first row was in the scrollback (its history's
    /// length less the view's offset up it): how far it has scrolled.
    top: i64,
}

/// What every row is laid out for: the cell metrics and scale, the
/// colours. (Not the grid's size: a row the same but for blanks at its end
/// is laid out the same, so a resize lays out only what it changes.)
#[derive(PartialEq)]
struct RowKey {
    metrics: Metrics,
    scheme: Scheme,
    /// The program's own colours (OSC 4, 10, 11, …).
    colors: Vec<Option<Rgb>>,
}

/// A row as laid out, relative to its top left: backgrounds, glyphs,
/// shapes (boxes, lines), and pictures' cells.
struct CachedRow {
    cells: Vec<Cell>,
    /// The columns selected in it, first and last.
    selected: Option<(usize, usize)>,
    bgs: Vec<(f32, f32, Hsla)>,
    glyphs: Vec<Glyph>,
    /// Its glyphs as last painted (where the atlas has them).
    run: RefCell<GlyphRun>,
    shapes: Vec<Shape>,
    /// Pictures' cells: column, placement, row of the picture.
    images: Vec<(usize, u32, usize)>,
}

#[derive(Clone, Copy)]
struct Glyph {
    x: f32,
    font: FontId,
    id: GlyphId,
    size: f32,
    color: Hsla,
    emoji: bool,
}

/// What box drawing draws: rectangles (with an opacity), lines through
/// points, filled polygons; relative to the row's top left.
#[derive(Clone)]
enum Shape {
    Rect(f32, f32, f32, f32, Hsla),
    Line(Vec<(f32, f32)>, f32, Hsla),
    Poly(Vec<(f32, f32)>, Hsla),
}

/// The screen as last drawn whole, for the frames while it's reflowed.
struct Frame {
    background: Hsla,
}

/// The columns of `line` in a selection, first and last.
fn selected_in(s: &SelectionRange, line: i32) -> Option<(usize, usize)> {
    if line < s.start.line.0 || line > s.end.line.0 {
        return None;
    }
    let first = if s.start.line.0 != line && !s.is_block { 0 } else { s.start.column.0 };
    let last = if s.end.line.0 != line && !s.is_block { usize::MAX } else { s.end.column.0 };
    (first <= last).then_some((first, last))
}

#[derive(Clone)]
pub struct Hover {
    /// Grid point the pointer was over when this was found.
    pub at: Point,
    cells: Vec<Point>,
    pub target: Option<links::Target>,
}

pub struct Output {
    pub paste: bool,
    pub context_menu: Option<gpui::Point<Pixels>>,
    /// A link ⌘-clicked.
    pub open: Option<links::Target>,
}

fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c) || ('\u{e0b0}'..='\u{e0b3}').contains(&c)
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Hsla,
    bold: bool,
    italic: bool,
}

fn font_for(st: Style) -> gpui::Font {
    let mut f = style::mono_font(if st.bold { FontWeight::BOLD } else { FontWeight::NORMAL });
    if st.italic {
        f.style = gpui::FontStyle::Italic;
    }
    f
}

/// Whether a colour cluster (an emoji) wants its colour glyph: a wide one
/// that isn't ASCII, one with the emoji presentation selector, a flag.
fn wants_color(cluster: &str, wide: bool) -> bool {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else { return false };
    (wide && !first.is_ascii()) || cluster.contains('\u{FE0F}') || is_regional(first)
}

fn is_regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Whether `next` (a cell's text) goes on with the sequence in `prev`:
/// after a zero-width joiner, a skin tone, or a flag's second letter.
fn continues(prev: &str, next: &str) -> bool {
    let Some(first) = next.chars().next() else { return false };
    let skin = ('\u{1F3FB}'..='\u{1F3FF}').contains(&first);
    let flag = is_regional(first) && prev.chars().count() == 1 && prev.chars().all(is_regional);
    prev.ends_with('\u{200D}') || skin || flag
}

/// What [`paint`] draws a screen with.
pub struct Paint<'a> {
    pub source: &'a Source,
    pub scheme: &'a Scheme,
    pub metrics: Metrics,
    pub focused: bool,
    /// The link under the pointer, underlined.
    pub hover: Option<&'a Hover>,
    /// Text being composed with an input method, at the cursor.
    pub preedit: Option<&'a str>,
}

/// `text`'s glyphs (printable ASCII) in `st`'s font, from what's been
/// found already: None if the font lacks one (it's shaped, with fallback).
fn ascii_glyphs(window: &Window, found: &mut Vec<(Style, FontId, Box<[Option<Option<GlyphId>>; 95]>)>, st: Style, text: &str) -> Option<(FontId, Vec<GlyphId>)> {
    let ts = window.text_system();
    let k = match found.iter().position(|(s, _, _)| s.bold == st.bold && s.italic == st.italic) {
        Some(k) => k,
        None => {
            found.push((st, ts.resolve_font(&font_for(st)), Box::new([None; 95])));
            found.len() - 1
        }
    };
    let (_, font, table) = &mut found[k];
    let mut ids = Vec::with_capacity(text.len());
    for b in text.bytes() {
        let slot = &mut table[usize::from(b - 0x20)];
        let id = *slot.get_or_insert_with(|| ts.glyph_for_char(*font, char::from(b)));
        ids.push(id?);
    }
    Some((*font, ids))
}

/// Whether a row laid out from `a` is laid out the same from `b`: the
/// same cells, or (the grid resized) the same but for blanks at the end
/// of the longer, the shorter's last cell blank too (a narrow emoji at
/// the edge borrows the cell beside it only where there is one).
fn same_cells(a: &[Cell], b: &[Cell]) -> bool {
    if a.len() == b.len() {
        return a == b;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let blank = Cell::default();
    long[..short.len()] == *short && short.last().is_none_or(|c| *c == blank) && long[short.len()..].iter().all(|c| *c == blank)
}

/// Draw the screen into `rect`.
pub fn paint(window: &mut Window, cx: &mut App, rect: Bounds<Pixels>, p: Paint, pane: &Pane) {
    let Paint { source: s, scheme, metrics: m, focused, hover, preedit } = p;
    pane.bounds.set(rect);
    // Being reflowed to a new width: its background, while that takes (the
    // new width shows the next frame).
    let term = if s.reflowing() { s.term.try_lock_unfair() } else { Some(s.term.lock()) };
    let Some(mut term) = term else {
        if let Some(f) = &*pane.last.borrow() {
            window.paint_quad(gpui::fill(rect, f.background));
        }
        return;
    };
    // The lines changed since the screen was last drawn (alacritty's
    // damage, taken here, its only reader): the others needn't be looked
    // at. Scrolling (output, the view), a resize, a colour or a cleared
    // screen damage it all (None).
    let lines = term.screen_lines();
    let damaged: Option<Vec<bool>> = match term.damage() {
        TermDamage::Full => None,
        TermDamage::Partial(changed) => {
            let mut d = vec![false; lines];
            for l in changed {
                if let Some(x) = d.get_mut(l.line) {
                    *x = true;
                }
            }
            Some(d)
        }
    };
    term.reset_damage();
    let mut origin = (f32::from(rect.left()) + PAD.0, f32::from(rect.top()) + PAD.1);
    let history = term.grid().history_size();
    let content = term.renderable_content();
    let colors = content.colors;
    let bg_default = scheme.resolve(Color::Named(NamedColor::Background), colors);
    let background = hsla(bg_default);
    window.paint_quad(gpui::fill(rect, background));
    *pane.last.borrow_mut() = Some(Frame { background });
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden;
    let cursor = content.cursor;
    let cols = term.columns();
    let rows = term.screen_lines();
    let (cw, ch) = m.cell;
    // The scrollback scrolled by the point: the rows drawn as far below
    // their places as it's scrolled past a whole row, or as it's stretched
    // past an end (on device pixels); the row above them too when part of
    // it shows. Still springing back: another frame.
    let (slide, max, seen, moving) = {
        let mut sm = pane.smooth.borrow_mut();
        let (at, max) = sm.sync(offset as usize, history, rows as f32 * ch, ch);
        let (shown, moving) = sm.elastic.offset(at);
        let slide = sm.pos - offset as f32 * ch + f32::from(shown.y - at.y);
        ((slide * m.scale).round() / m.scale, max, -f32::from(shown.y), moving)
    };
    window.note_scroll([0.0, seen, 0.0, max]);
    if moving {
        window.request_animation_frame();
    }
    pane.shift.set(slide);
    let extra = usize::from(slide > 0.0 && (offset as usize) < history);
    origin.1 += slide - extra as f32 * ch;
    // The lines drawn: from `first`, `drawn` of them.
    let first = -offset - extra as i32;
    let drawn = rows + extra;
    let at = |line: i32, col: usize| (origin.0 + col as f32 * cw, origin.1 + (line - first) as f32 * ch);
    // A cell's text and background colours (before any selection). (The
    // last pair made legible kept: colours come in runs along a row.)
    let legible_last = std::cell::Cell::new(None::<(Rgb, Rgb, Rgb)>);
    let cell_colors = |c: &Cell| {
        let mut fg = scheme.resolve(c.fg, colors);
        let mut bg = scheme.resolve(c.bg, colors);
        if c.flags.contains(Flags::DIM) {
            fg = mix(fg, bg, 0.66);
        }
        if c.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        // Readable whatever background the program had in mind
        // (`colors::legible`); pictures drawn in characters as they are.
        if !crate::colors::graphic(c.c) {
            fg = match legible_last.get() {
                Some((f, b, made)) if f == fg && b == bg => made,
                _ => {
                    let made = crate::colors::legible(fg, bg, crate::colors::MIN_CONTRAST);
                    legible_last.set(Some((fg, bg, made)));
                    made
                }
            };
        }
        (fg, bg)
    };
    let style_of = |c: &Cell, fg| Style { fg: hsla(fg), bold: c.flags.contains(Flags::BOLD), italic: c.flags.contains(Flags::ITALIC) };
    let spacer = |c: &Cell| c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);

    let key = RowKey { metrics: m, scheme: scheme.clone(), colors: (0..COUNT).map(|i| colors[i]).collect() };
    let mut cache = pane.rows.borrow_mut();
    if cache.key.as_ref() != Some(&key) {
        cache.key = Some(key);
        cache.rows.clear();
    }
    let grid = term.grid();
    // The rows as last drawn, each to be found where it is now: moved by
    // as far as the screen scrolled (new row r was old row r + shift),
    // else as the row before was, else (twice a frame at most, when output
    // scrolls a full scrollback, which doesn't move `top`) looked for.
    let top = grid.history_size() as i64 + i64::from(first);
    let mut shift = top - std::mem::replace(&mut cache.top, top);
    let mut old = std::mem::take(&mut cache.rows);
    old.resize_with(drawn, || None);
    cache.rows.resize_with(drawn, || None);
    let mut searches = 0;
    let line_px = 1.0 / m.scale;
    for r in 0..drawn {
        let line = first + r as i32;
        let row = &grid[Line(line)];
        let cells = &row[..Column(cols.min(row.len()))];
        let selected = selection.and_then(|s| selected_in(&s, line));
        // Not damaged, where it was, the selection as it was: as drawn.
        // (The row above the screen isn't in the damage: compared.)
        if shift == 0 && r >= extra && damaged.as_ref().is_some_and(|d| d.get(r - extra) == Some(&false)) && old[r].as_ref().is_some_and(|c| c.selected == selected) {
            cache.rows[r] = old[r].take();
            continue;
        }
        let same = |c: &Option<CachedRow>| c.as_ref().is_some_and(|c| c.selected == selected && same_cells(&c.cells, cells));
        let at = |j: i64| usize::try_from(j).ok().filter(|&j| j < drawn);
        let mut was = [r as i64 + shift, r as i64].into_iter().filter_map(at).find(|&j| same(&old[j]));
        if was.is_none() && searches < 2 {
            searches += 1;
            was = (0..drawn).find(|&j| same(&old[j]));
            if let Some(j) = was {
                shift = j as i64 - r as i64;
            }
        }
        if let Some(j) = was {
            cache.rows[r] = old[j].take();
            continue;
        }
        let mut bgs: Vec<(f32, f32, Hsla)> = Vec::new();
        // Runs of text in one style, each character at its cell.
        let mut runs: Vec<Run> = Vec::new();
        let mut shapes = Vec::new();
        let mut images = Vec::new();
        let mut last_bg: Option<(usize, Rgb)> = None;
        // Emoji clusters: column, text, style, cells.
        let mut emoji: Vec<(usize, String, Style, usize)> = Vec::new();
        let mut used = vec![false; cols + 1];
        // Blank cells draw nothing: up to the last that isn't.
        let blank = Cell::default();
        let end = cells.iter().rposition(|c| *c != blank).map_or(0, |i| i + 1);
        for (col, cell) in cells[..end].iter().enumerate() {
            let flags = cell.flags;
            if spacer(cell) {
                continue;
            }
            if let Some(h) = cell.hyperlink()
                && let Some((uid, row)) = graphics::parse_uri(h.uri())
            {
                images.push((col, uid, row));
            }
            let (fg, mut bg) = cell_colors(cell);
            if selected.is_some_and(|(a, z)| a <= col && col <= z) {
                bg = scheme.selection;
            }
            let wide = flags.contains(Flags::WIDE_CHAR);
            let width = if wide { 2 } else { 1 };
            if bg != bg_default {
                // One rectangle for a stretch of one colour.
                match (bgs.last_mut(), last_bg) {
                    (Some(b), Some((end, c))) if end == col && c == bg => {
                        b.1 += width as f32 * cw;
                    }
                    _ => bgs.push((col as f32 * cw, width as f32 * cw, hsla(bg))),
                }
                last_bg = Some((col + width, bg));
            }
            if flags.contains(Flags::HIDDEN) {
                continue;
            }
            let style = style_of(cell, fg);
            let x = col as f32 * cw;
            let w = cw * width as f32;
            if flags.intersects(Flags::ALL_UNDERLINES) {
                let uc = cell.underline_color().map(|c| hsla(scheme.resolve(c, colors))).unwrap_or(style.fg);
                let y = m.baseline + (m.row * 0.12).max(1.0);
                let t = (m.size / 12.0).round().max(1.0) * line_px.max(0.5);
                shapes.push(Shape::Rect(x, y, w, t, uc));
                if flags.contains(Flags::DOUBLE_UNDERLINE) {
                    shapes.push(Shape::Rect(x, y + 2.0 * t, w, t, uc));
                }
            }
            if flags.contains(Flags::STRIKEOUT) {
                let y = m.baseline - m.row * 0.22;
                shapes.push(Shape::Rect(x, y, w, line_px.max(0.5), style.fg));
            }
            let c = cell.c;
            if c == ' ' || c == '\0' {
                continue;
            }
            used[col] = true;
            if is_box(c) {
                draw_box(&mut shapes, (x, 0.0, cw, ch), c, style.fg, m.scale);
                continue;
            }
            let mut cluster = String::from(c);
            cluster.extend(cell.zerowidth().into_iter().flatten());
            // alacritty gives each part of a sequence its own cell: join
            // skin tones, ZWJ sequences and flags back up.
            if let Some(prev) = emoji.last_mut()
                && prev.0 + prev.3 == col
                && continues(&prev.1, &cluster)
            {
                prev.1.push_str(&cluster);
                prev.3 += width;
                continue;
            }
            if wants_color(&cluster, wide) {
                emoji.push((col, cluster, style, width));
                continue;
            }
            match runs.last_mut() {
                Some(r) if r.style == style && r.next == col => {
                    r.starts.push((r.text.len(), col));
                    r.text.push_str(&cluster);
                    r.next = col + width;
                }
                _ => runs.push(Run { text: cluster, style, starts: vec![(0, col)], next: col + width }),
            }
        }
        // Lay out each run, every glyph at its character's cell.
        let mut glyphs = Vec::new();
        for Run { text, style: st, starts, .. } in &runs {
            // Printable ASCII the style's font has: each glyph straight
            // from the font at its cell, not shaped (19 µs a run, a full
            // screen of short lines 2 ms at 1×); the same as shaping gives
            // a monospaced font's own characters.
            if text.bytes().all(|b| (0x21..0x7f).contains(&b))
                && let Some(ids) = ascii_glyphs(window, &mut cache.ascii, *st, text)
            {
                let (font, ids) = ids;
                for (&(_, col), id) in starts.iter().zip(ids) {
                    glyphs.push(Glyph { x: col as f32 * cw, font, id, size: m.size, color: st.fg, emoji: false });
                }
                continue;
            }
            let font = font_for(*st);
            let run = TextRun { len: text.len(), font, color: st.fg, background_color: None, underline: None, strikethrough: None, size: None };
            let layout = window.text_system().layout_line_now(text, px(m.size), &[run]);
            for shaped in &layout.runs {
                // A cluster's glyphs after its first keep their place from it.
                let mut cluster: Option<(usize, f32)> = None;
                for g in &shaped.glyphs {
                    let k = starts.partition_point(|&(byte, _)| byte <= g.index).saturating_sub(1);
                    let (byte, col) = starts[k];
                    let gx = f32::from(g.position.x);
                    let base = match cluster {
                        Some((b, x0)) if b == byte => x0,
                        _ => {
                            cluster = Some((byte, gx));
                            gx
                        }
                    };
                    // Wider than its cell (a fallback font's): centred in it.
                    let adv = if cells.get(col).is_some_and(|c| c.flags.contains(Flags::WIDE_CHAR)) { 2.0 * cw } else { cw };
                    let glyph_w = next_x(&shaped.glyphs, g.index, layout.width).map_or(m.advance, |nx| f32::from(nx) - gx).max(0.0);
                    let centre = if (glyph_w - m.advance).abs() > 0.5 { ((adv - glyph_w) / 2.0 * m.scale).round() / m.scale } else { 0.0 };
                    let x = col as f32 * cw + (gx - base) + centre;
                    glyphs.push(Glyph { x, font: shaped.font_id, id: g.id, size: f32::from(shaped.font_size), color: st.fg, emoji: g.is_emoji });
                }
            }
        }
        // Colour emoji, two cells wide as an emoji should be: a narrow one
        // borrows a free cell beside it.
        for (col, text, st, n) in emoji {
            let span = if n == 1 && col + 1 < cols && !used[col + 1] { 2 } else { n };
            let run = TextRun { len: text.len(), font: font_for(st), color: st.fg, background_color: None, underline: None, strikethrough: None, size: None };
            let layout = window.text_system().layout_line_now(&text, px(m.size), &[run]);
            let w = f32::from(layout.width);
            let x0 = col as f32 * cw + ((span.min(2) as f32 * cw - w) / 2.0 * m.scale).round() / m.scale + (span.saturating_sub(2)) as f32 * cw / 2.0;
            for shaped in &layout.runs {
                for g in &shaped.glyphs {
                    glyphs.push(Glyph { x: x0 + f32::from(g.position.x), font: shaped.font_id, id: g.id, size: f32::from(shaped.font_size), color: st.fg, emoji: g.is_emoji });
                }
            }
        }
        cache.rows[r] = Some(CachedRow { cells: cells.to_vec(), selected, bgs, glyphs, run: RefCell::default(), shapes, images });
    }
    // The rows in one layer: one place in the scene's order for all of
    // them, not each glyph's own, worked out against everything drawn so
    // far (2 µs a glyph: 1.4 ms a frame for a screen of short lines at 1×
    // on a big display). In it every row's backgrounds are under every
    // row's text, so a glyph reaching into the next row isn't covered;
    // lines and shapes over the backgrounds. The cursor, links and
    // pictures come after, over it.
    window.paint_layer(rect, |window| {
        for (r, row) in cache.rows.iter().enumerate() {
            let Some(row) = row else { continue };
            let y = origin.1 + r as f32 * ch;
            for &(x, w, color) in &row.bgs {
                window.paint_quad(gpui::fill(Bounds { origin: point(px(origin.0 + x), px(y)), size: size(px(w), px(ch)) }, color));
            }
        }
        for (r, row) in cache.rows.iter().enumerate() {
            let Some(row) = row else { continue };
            let y = origin.1 + r as f32 * ch;
            paint_shapes(window, &row.shapes, (origin.0, y));
            let base = point(px(origin.0), px(y + m.baseline));
            window.paint_glyph_run(&mut row.run.borrow_mut(), base, 0, |glyph| {
                for g in row.glyphs.iter().filter(|g| !g.emoji) {
                    glyph(point(px(g.x), px(0.0)), g.font, g.id, px(g.size), g.color);
                }
            });
            for g in row.glyphs.iter().filter(|g| g.emoji) {
                let _ = window.paint_emoji(point(base.x + px(g.x), base.y), g.font, g.id, px(g.size));
            }
        }
    });
    let image_cells: Vec<(i32, usize, u32, usize)> =
        cache.rows.iter().enumerate().filter_map(|(r, d)| Some((first + r as i32, d.as_ref()?))).flat_map(|(line, d)| d.images.iter().map(move |&(col, uid, row)| (line, col, uid, row))).collect();
    drop(cache);
    draw_images(window, s, &image_cells, pane, &at, m.cell);

    // The hovered link, underlined.
    let cursor_color = hsla(scheme.resolve(Color::Named(NamedColor::Cursor), colors));
    if let Some(h) = hover.filter(|h| h.target.is_some()) {
        let t = line_px.max(0.5) * 2.0;
        for p in &h.cells {
            let (x, y) = at(p.line.0, p.column.0);
            window.paint_quad(gpui::fill(Bounds { origin: point(px(x), px(y + m.baseline + (m.row * 0.12).max(1.0))), size: size(px(cw), px(t)) }, cursor_color));
        }
    }
    // Text being composed: at the cursor, underlined, over what's there.
    let mut cursor_x = 0.0;
    if let Some(text) = preedit.filter(|t| !t.is_empty()) {
        let (x, y) = at(cursor.point.line.0, cursor.point.column.0);
        let fg = hsla(scheme.resolve(Color::Named(NamedColor::Foreground), colors));
        let run = TextRun { len: text.len(), font: style::mono_font(FontWeight::NORMAL), color: fg, background_color: None, underline: None, strikethrough: None, size: None };
        let line = window.text_system().shape_line(text.to_owned().into(), px(m.size), &[run], Some(px(cw)));
        let w = f32::from(line.width).max(cw);
        window.paint_quad(gpui::fill(Bounds { origin: point(px(x), px(y)), size: size(px(w), px(ch)) }, background));
        paint_at_baseline(window, cx, &line, x, y + m.baseline);
        window.paint_quad(gpui::fill(Bounds { origin: point(px(x), px(y + ch - 2.0 * line_px)), size: size(px(w), px((2.0 * line_px).max(1.0))) }, fg));
        cursor_x = w;
    }
    // The cursor: a solid block when focused (with its character in the
    // background colour), an outline when not.
    let cursor_on_screen = show_cursor && (0..rows as i32).contains(&(cursor.point.line.0 + offset));
    if cursor_on_screen {
        let cell = &grid[cursor.point];
        let wide = !spacer(cell) && cell.flags.contains(Flags::WIDE_CHAR);
        let (x, y) = at(cursor.point.line.0, cursor.point.column.0);
        let x = x + cursor_x;
        let w = if wide { cw * 2.0 } else { cw };
        let r = Bounds { origin: point(px(x), px(y)), size: size(px(w), px(ch)) };
        match (focused, cursor.shape) {
            (true, CursorShape::Beam) => window.paint_quad(gpui::fill(Bounds { origin: r.origin, size: size(px((2.0 * line_px).max(1.0)), px(ch)) }, cursor_color)),
            (true, CursorShape::Underline) => {
                let t = 2.0 * line_px.max(0.5) * 2.0;
                window.paint_quad(gpui::fill(Bounds { origin: point(px(x), px(y + ch - t)), size: size(px(w), px(t)) }, cursor_color));
            }
            (true, _) => {
                window.paint_quad(gpui::fill(r, cursor_color));
                let c = cell.c;
                if !spacer(cell) && c != ' ' && c != '\0' && !is_box(c) && preedit.is_none_or(str::is_empty) {
                    let st = style_of(cell, bg_default);
                    let text: String = std::iter::once(c).chain(cell.zerowidth().into_iter().flatten().copied()).collect();
                    let run = TextRun { len: text.len(), font: font_for(st), color: background, background_color: None, underline: None, strikethrough: None, size: None };
                    let line = window.text_system().shape_line(text.into(), px(m.size), &[run], None);
                    let lx = x + ((w - f32::from(line.width)) / 2.0 * m.scale).round() / m.scale;
                    paint_at_baseline(window, cx, &line, lx, y + m.baseline);
                }
            }
            (false, _) => {
                let t = line_px.max(0.5);
                window.paint_quad(
                    gpui::outline(Bounds { origin: point(px(x + t / 2.0), px(y + t / 2.0)), size: size(px(w - t), px(ch - t)) }, cursor_color, gpui::BorderStyle::Solid).border_widths(px(2.0 * t)),
                );
            }
        }
    }
    // (Where it is in the scrollback: the kit's slim scroll bar over the
    // screen, ui.rs.)
}

/// Characters in one style on adjacent cells, laid out together.
struct Run {
    text: String,
    style: Style,
    /// Each character's byte in `text` and its column.
    starts: Vec<(usize, usize)>,
    /// The column the run goes on at.
    next: usize,
}

/// Paints `line` with its baseline at `baseline`.
fn paint_at_baseline(window: &mut Window, cx: &mut App, line: &gpui::ShapedLine, x: f32, baseline: f32) {
    let h = line.ascent + line.descent;
    let _ = line.paint(point(px(x), px(baseline) - line.ascent), h, window, cx);
}

/// Where the glyph after the ones for text byte `index` starts: the next
/// cluster's first glyph, else the line's end.
fn next_x(glyphs: &[gpui::ShapedGlyph], index: usize, end: Pixels) -> Option<Pixels> {
    Some(glyphs.iter().find(|g| g.index > index).map_or(end, |g| g.position.x))
}

fn paint_shapes(window: &mut Window, shapes: &[Shape], (ox, oy): (f32, f32)) {
    for s in shapes {
        match s {
            Shape::Rect(x, y, w, h, c) => window.paint_quad(gpui::fill(Bounds { origin: point(px(ox + x), px(oy + y)), size: size(px(*w), px(*h)) }, *c)),
            Shape::Line(pts, width, c) => {
                let mut b = PathBuilder::stroke(px(*width));
                for (k, &(x, y)) in pts.iter().enumerate() {
                    let p = point(px(ox + x), px(oy + y));
                    if k == 0 { b.move_to(p) } else { b.line_to(p) }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, *c);
                }
            }
            Shape::Poly(pts, c) => {
                let mut b = PathBuilder::fill();
                for (k, &(x, y)) in pts.iter().enumerate() {
                    let p = point(px(ox + x), px(oy + y));
                    if k == 0 { b.move_to(p) } else { b.line_to(p) }
                }
                b.close();
                if let Ok(path) = b.build() {
                    window.paint_path(path, *c);
                }
            }
        }
    }
}

/// Box-drawing (U+2500–257F), blocks (U+2580–259F) and the Powerline
/// arrows, drawn to the cell (`x, y, w, h`) so adjacent cells connect
/// exactly.
fn draw_box(out: &mut Vec<Shape>, (rx, ry, rw, rh): (f32, f32, f32, f32), c: char, color: Hsla, scale: f32) {
    let one = 1.0 / scale;
    let snap = |v: f32| (v * scale).round() / scale;
    let light = (rw / 8.0 * scale).round().max(1.0) / scale;
    let heavy = light * 2.0;
    let (cx, cy) = (rx + rw / 2.0, ry + rh / 2.0);
    let (x1, y1) = (rx + rw, ry + rh);
    let mut fill = |x0: f32, y0: f32, x1: f32, y1: f32| out.push(Shape::Rect(x0, y0, x1 - x0, y1 - y0, color));
    let code = c as u32;
    // Lines: which of left/right/up/down are drawn, and how thick.
    let seg = |fill: &mut dyn FnMut(f32, f32, f32, f32), l: f32, rgt: f32, u: f32, d: f32| {
        let hy = |t: f32| snap(cy - t / 2.0);
        let vx = |t: f32| snap(cx - t / 2.0);
        if l > 0.0 {
            let t = u.max(d).max(l);
            fill(rx, hy(l), vx(t) + t, hy(l) + l);
        }
        if rgt > 0.0 {
            fill(vx(u.max(d).max(rgt)), hy(rgt), x1, hy(rgt) + rgt);
        }
        if u > 0.0 {
            let t = l.max(rgt).max(u);
            fill(vx(u), ry, vx(u) + u, hy(t) + t);
        }
        if d > 0.0 {
            fill(vx(d), hy(l.max(rgt).max(d)), vx(d) + d, y1);
        }
    };
    let (n, h) = (light, heavy);
    match code {
        0x2500 => seg(&mut fill, n, n, 0.0, 0.0),
        0x2501 => seg(&mut fill, h, h, 0.0, 0.0),
        0x2502 => seg(&mut fill, 0.0, 0.0, n, n),
        0x2503 => seg(&mut fill, 0.0, 0.0, h, h),
        0x250c => seg(&mut fill, 0.0, n, 0.0, n),
        0x250f => seg(&mut fill, 0.0, h, 0.0, h),
        0x2510 => seg(&mut fill, n, 0.0, 0.0, n),
        0x2513 => seg(&mut fill, h, 0.0, 0.0, h),
        0x2514 => seg(&mut fill, 0.0, n, n, 0.0),
        0x2517 => seg(&mut fill, 0.0, h, h, 0.0),
        0x2518 => seg(&mut fill, n, 0.0, n, 0.0),
        0x251b => seg(&mut fill, h, 0.0, h, 0.0),
        0x251c => seg(&mut fill, 0.0, n, n, n),
        0x2523 => seg(&mut fill, 0.0, h, h, h),
        0x2524 => seg(&mut fill, n, 0.0, n, n),
        0x252b => seg(&mut fill, h, 0.0, h, h),
        0x252c => seg(&mut fill, n, n, 0.0, n),
        0x2533 => seg(&mut fill, h, h, 0.0, h),
        0x2534 => seg(&mut fill, n, n, n, 0.0),
        0x253b => seg(&mut fill, h, h, h, 0.0),
        0x253c => seg(&mut fill, n, n, n, n),
        0x254b => seg(&mut fill, h, h, h, h),
        0x2574 => seg(&mut fill, n, 0.0, 0.0, 0.0),
        0x2575 => seg(&mut fill, 0.0, 0.0, n, 0.0),
        0x2576 => seg(&mut fill, 0.0, n, 0.0, 0.0),
        0x2577 => seg(&mut fill, 0.0, 0.0, 0.0, n),
        // Rounded corners: straight arms meeting a quarter circle.
        0x256d..=0x2570 => {
            let (hx, vy) = match code {
                0x256d => (1.0, 1.0),
                0x256e => (-1.0, 1.0),
                0x256f => (-1.0, -1.0),
                _ => (1.0, -1.0),
            };
            let c = (snap(cx - n / 2.0) + n / 2.0, snap(cy - n / 2.0) + n / 2.0);
            let radius = (rw / 2.0).min(rh / 2.0);
            let arc = (c.0 + hx * radius, c.1 + vy * radius);
            let mut pts = vec![(c.0, if vy > 0.0 { y1 } else { ry })];
            for k in 0..=12 {
                let t = k as f32 / 12.0 * std::f32::consts::FRAC_PI_2;
                pts.push((arc.0 - hx * radius * t.cos(), arc.1 - vy * radius * t.sin()));
            }
            pts.push((if hx > 0.0 { x1 } else { rx }, c.1));
            out.push(Shape::Line(pts, n, color));
        }
        // Double lines. Each arm is two lines; where arms meet, a line
        // stops short (inner) or runs on to the corner (outer) depending on
        // whether another arm is on its side.
        0x2550 | 0x2551 | 0x2554 | 0x2557 | 0x255a | 0x255d | 0x2560 | 0x2563 | 0x2566 | 0x2569 | 0x256c => {
            let (al, ar, au, ad) = match code {
                0x2550 => (true, true, false, false),
                0x2551 => (false, false, true, true),
                0x2554 => (false, true, false, true),
                0x2557 => (true, false, false, true),
                0x255a => (false, true, true, false),
                0x255d => (true, false, true, false),
                0x2560 => (false, true, true, true),
                0x2563 => (true, false, true, true),
                0x2566 => (true, true, false, true),
                0x2569 => (true, true, true, false),
                _ => (true, true, true, true),
            };
            let g = snap(light * 1.5).max(light * 1.5);
            let c = (snap(cx - n / 2.0), snap(cy - n / 2.0));
            for side in [-1.0f32, 1.0] {
                let y = c.1 + side * g;
                let inner = if if side < 0.0 { au } else { ad } { g } else { -g };
                if ar {
                    fill(c.0 + inner, y, x1, y + n);
                }
                if al {
                    fill(rx, y, c.0 - inner + n, y + n);
                }
                let x = c.0 + side * g;
                let inner = if if side < 0.0 { al } else { ar } { g } else { -g };
                if ad {
                    fill(x, c.1 + inner, x + n, y1);
                }
                if au {
                    fill(x, ry, x + n, c.1 - inner + n);
                }
            }
        }
        // Blocks.
        0x2580 => fill(rx, ry, x1, snap(cy)),
        0x2581..=0x2588 => {
            let eighths = (code - 0x2580) as f32;
            fill(rx, snap(y1 - rh * eighths / 8.0), x1, y1);
        }
        0x2589..=0x258f => {
            let eighths = (0x2590 - code) as f32;
            fill(rx, ry, snap(rx + rw * eighths / 8.0), y1);
        }
        0x2590 => fill(snap(cx), ry, x1, y1),
        0x2591..=0x2593 => {
            let a = [0.25, 0.5, 0.75][(code - 0x2591) as usize];
            out.push(Shape::Rect(rx, ry, rw, rh, color.opacity(a)));
        }
        0x2594 => fill(rx, ry, x1, snap(ry + rh / 8.0)),
        0x2595 => fill(snap(x1 - rw / 8.0), ry, x1, y1),
        0x2596..=0x259f => {
            // Quadrants: bit order upper-left, upper-right, lower-left, lower-right.
            let q = [0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111][(code - 0x2596) as usize];
            let (mx, my) = (snap(cx), snap(cy));
            let quads = [(rx, ry, mx, my), (mx, ry, x1, my), (rx, my, mx, y1), (mx, my, x1, y1)];
            for (i, (a, b, c2, d)) in quads.into_iter().enumerate() {
                if q & (0b1000 >> i) != 0 {
                    fill(a, b, c2, d);
                }
            }
        }
        // Powerline separators.
        0xe0b0 => out.push(Shape::Poly(vec![(rx, ry), (x1, cy), (rx, y1)], color)),
        0xe0b2 => out.push(Shape::Poly(vec![(x1, ry), (x1, y1), (rx, cy)], color)),
        0xe0b1 => out.push(Shape::Line(vec![(rx, ry), (x1 - one, cy), (rx, y1)], light, color)),
        0xe0b3 => out.push(Shape::Line(vec![(x1, ry), (rx + one, cy), (x1, y1)], light, color)),
        // Anything else in the range: a light box, as nothing better is known.
        _ => seg(&mut fill, n, n, n, n),
    }
}

// ---- images and links -------------------------------------------------------

/// Paint the pictures in tagged cells: each placement whole, cut to the
/// runs of its cells on the screen.
fn draw_images(window: &mut Window, s: &Source, cells: &[(i32, usize, u32, usize)], pane: &Pane, at: &dyn Fn(i32, usize) -> (f32, f32), (cw, ch): (f32, f32)) {
    let mut made = pane.images.borrow_mut();
    if cells.is_empty() {
        for (_, (_, _, img)) in made.drain() {
            let _ = window.drop_image(img);
        }
        return;
    }
    let store = s.shared.images.lock().unwrap();
    let mut i = 0;
    while i < cells.len() {
        let (line, col, uid, row) = cells[i];
        let mut j = i + 1;
        while j < cells.len() && cells[j].0 == line && cells[j].2 == uid && cells[j].3 == row && cells[j].1 == cells[j - 1].1 + 1 {
            j += 1;
        }
        let last_col = cells[j - 1].1;
        i = j;
        let Some(p) = store.placements.get(&uid) else { continue };
        let Some(img) = store.images.get(&p.image) else { continue };
        let fresh = made.get(&uid).is_some_and(|(g, src, _)| *g == img.generation && *src == p.src);
        if !fresh {
            let Some(picture) = crop(&img.rgba, img.width, img.height, p.src) else { continue };
            if let Some((_, _, old)) = made.insert(uid, (img.generation, p.src, picture)) {
                let _ = window.drop_image(old);
            }
        }
        let Some((_, _, picture)) = made.get(&uid) else { continue };
        // The placement's whole box, cut to these cells.
        let (x0, y0) = at(line - row as i32, p.start_col);
        let whole = Bounds { origin: point(px(x0), px(y0)), size: size(px(cw * p.cols as f32), px(ch * p.rows as f32)) };
        let (sx, sy) = at(line, col);
        let shown = Bounds { origin: point(px(sx), px(sy)), size: size(px(cw * (last_col + 1 - col) as f32), px(ch)) };
        let picture = picture.clone();
        window.with_content_mask(Some(ContentMask { bounds: shown }), |window| {
            let _ = window.paint_image(whole, gpui::Corners::default(), picture, 0, false);
        });
    }
    // Forget pictures that are gone.
    let gone: Vec<u32> = made.keys().copied().filter(|uid| !store.placements.contains_key(uid)).collect();
    for uid in gone {
        if let Some((_, _, img)) = made.remove(&uid) {
            let _ = window.drop_image(img);
        }
    }
}

/// The part `[x, y, w, h]` of an RGBA image, as GPUI draws pictures (BGRA).
fn crop(rgba: &[u8], width: u32, height: u32, [x, y, w, h]: [u32; 4]) -> Option<Arc<RenderImage>> {
    let (x, y) = (x.min(width), y.min(height));
    let (w, h) = (w.min(width - x).max(1), h.min(height - y).max(1));
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = ((row * width + x) * 4) as usize;
        let line = rgba.get(start..start + (w * 4) as usize)?;
        for p in line.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    let buffer = image::RgbaImage::from_raw(w, h, out)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// The logical line (following wraps) through `pt`, as text with the grid
/// point of each character.
fn logical_line<T>(term: &alacritty_terminal::Term<T>, pt: Point) -> (String, Vec<Point>) {
    let grid = term.grid();
    let cols = term.columns();
    let top = -(grid.history_size() as i32);
    let bottom = term.screen_lines() as i32 - 1;
    let wraps = |line: i32| grid[Line(line)][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    let mut first = pt.line.0;
    while first > top && wraps(first - 1) {
        first -= 1;
    }
    let mut last = pt.line.0;
    while last < bottom && wraps(last) {
        last += 1;
    }
    let mut text = String::new();
    let mut points = Vec::new();
    for line in first..=last {
        for col in 0..cols {
            let c = &grid[Line(line)][Column(col)];
            if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            text.push(if c.c == '\0' { ' ' } else { c.c });
            points.push(Point::new(Line(line), Column(col)));
        }
    }
    (text, points)
}

fn find_link(s: &Session, at: Point, cwd: &dyn Fn() -> Option<std::path::PathBuf>) -> Hover {
    let term = s.term.lock();
    let none = Hover { at, cells: Vec::new(), target: None };
    // An OSC 8 hyperlink the program wrote.
    let cell = &term.grid()[at.line][at.column];
    if let Some(h) = cell.hyperlink()
        && graphics::parse_uri(h.uri()).is_none()
    {
        let (_, points) = logical_line(&term, at);
        let id = h.id().to_owned();
        let cells: Vec<Point> = points.into_iter().filter(|p| term.grid()[p.line][p.column].hyperlink().is_some_and(|o| o.id() == id)).collect();
        let uri = h.uri().to_owned();
        let target = match uri.strip_prefix("file://") {
            Some(rest) => {
                // file://host/path: drop the host.
                let path = rest.find('/').map(|i| &rest[i..]).unwrap_or(rest);
                links::Target::Path(path.into(), None)
            }
            None => links::Target::Url(uri),
        };
        return Hover { at, cells, target: Some(target) };
    }
    let (text, points) = logical_line(&term, at);
    // The rows around, for a word a program broke over them itself.
    let top = (at.line.0 - 24).max(term.grid().topmost_line().0);
    let bottom = (at.line.0 + 24).min(term.grid().bottommost_line().0);
    let cols = term.columns();
    let rows: Vec<Vec<char>> = (top..=bottom).map(|l| (0..cols).map(|c| term.grid()[Line(l)][Column(c)].c).collect()).collect();
    drop(term);
    let Some(col) = points.iter().position(|p| *p == at) else { return none };
    let Some(dir) = cwd() else { return none };
    if let Some((joined, i, cells)) = links::broken(&rows, (at.line.0 - top) as usize, at.column.0, cols)
        && let Some(l) = links::find(&joined, i, &dir, |p| p.exists())
        && cells[l.start].0 != cells[l.end - 1].0
    {
        let cells = cells[l.start..l.end].iter().map(|&(r, c)| Point::new(Line(top + r as i32), Column(c))).collect();
        return Hover { at, cells, target: Some(l.target) };
    }
    match links::find(&text, col, &dir, |p| p.exists()) {
        Some(l) => Hover { at, cells: points[l.start..l.end.min(points.len())].to_vec(), target: Some(l.target) },
        None => none,
    }
}

// ---- keyboard ---------------------------------------------------------------

/// A key as the terminal encodes it: GPUI's name for it, unshifted (it
/// names shifted symbols as themselves: "_" for ⇧-), and its modifiers,
/// Shift with them again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key<'a> {
    pub name: &'a str,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl<'a> Key<'a> {
    pub fn of(k: &'a Keystroke) -> Key<'a> {
        let (name, shifted) = olo_ui::keys::unshifted(&k.key);
        Key { name, shift: k.modifiers.shift || shifted, ctrl: k.modifiers.control, alt: k.modifiers.alt }
    }
}

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(k: Key) -> u8 {
    1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8
}

/// The bytes for a key, or None when it's left to the text it types.
pub fn key_bytes(k: Key, mode: TermMode) -> Option<Vec<u8>> {
    let mp = modifier_param(k);
    let kitty = mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL);
    let csi = |letter: char| -> Vec<u8> {
        if mp > 1 {
            format!("\x1b[1;{mp}{letter}").into_bytes()
        } else if mode.contains(TermMode::APP_CURSOR) {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if mp > 1 { format!("\x1b[{n};{mp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let ss3 = |letter: char| -> Vec<u8> { if mp > 1 { format!("\x1b[1;{mp}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() } };
    let esc = |mut b: Vec<u8>| {
        if k.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    let word = k.alt && !k.ctrl && !k.shift && !kitty;
    Some(match k.name {
        "up" => csi('A'),
        "down" => csi('B'),
        // Option+←/→ alone: a word back or forward, as macOS Terminal
        // sends it (readline, zsh and fish all know ESC b / ESC f; bash
        // leaves xterm's CSI 1;3 D unbound). A program that asked for the
        // kitty keyboard protocol (Claude Code, …) gets the protocol's
        // CSI 1;3 D and reads it as Alt+← itself.
        "right" if word => b"\x1bf".to_vec(),
        "left" if word => b"\x1bb".to_vec(),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" => esc(vec![b'\r']),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => esc(vec![b'\t']),
        "backspace" if k.ctrl => esc(vec![0x08]),
        "backspace" => esc(vec![0x7f]),
        "escape" => esc(vec![0x1b]),
        _ if k.ctrl => esc(vec![ctrl_byte(k)?]),
        // Alt with a character: ESC and the character (the text event's
        // job without Alt).
        name if k.alt => {
            let c = if name == "space" { ' ' } else { single(name)? };
            let c = if k.shift { c.to_ascii_uppercase() } else { c };
            esc(c.to_string().into_bytes())
        }
        _ => return None,
    })
}

/// A one-character key's character.
fn single(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Ctrl+key as a C0 control character (as xterm's: Ctrl+2 and Ctrl+@ are
/// NUL, Ctrl+3–7 the ESC…US block, Ctrl+8 and Ctrl+? DEL).
fn ctrl_byte(k: Key) -> Option<u8> {
    if k.name == "space" {
        return Some(0);
    }
    let c = single(k.name)?;
    if c.is_ascii_alphabetic() {
        return Some(c.to_ascii_lowercase() as u8 & 0x1f);
    }
    // With Shift: the shifted symbol's (Ctrl+_, Ctrl+^, Ctrl+@, Ctrl+?).
    let c = if k.shift { olo_ui::keys::shifted(c).unwrap_or(c) } else { c };
    Some(match c {
        '2' | '@' => 0,
        '3' | '[' | '{' => 0x1b,
        '4' | '\\' | '|' => 0x1c,
        '5' | ']' | '}' => 0x1d,
        '6' | '^' | '~' => 0x1e,
        '7' | '-' | '_' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// The kitty keyboard protocol's code for a key: its Unicode codepoint
/// (unshifted) or a functional-key number. None for the arrows and other
/// keys that keep their legacy `CSI 1;mods X` form under the protocol.
fn kitty_code(name: &str) -> Option<u32> {
    Some(match name {
        "escape" => 27,
        "enter" => 13,
        "tab" => 9,
        "backspace" => 127,
        "space" => 32,
        "insert" => 2,
        "delete" => 3,
        _ => {
            let c = single(name)?;
            if !c.is_ascii() {
                return None;
            }
            c.to_ascii_lowercase() as u32
        }
    })
}

/// `CSI code ; mods [: event] u`, the kitty protocol's key report.
fn csi_u(code: u32, k: Key, event: u8) -> Vec<u8> {
    let mp = modifier_param(k);
    match (mp, event) {
        (1, 1) => format!("\x1b[{code}u"),
        (_, 1) => format!("\x1b[{code};{mp}u"),
        _ => format!("\x1b[{code};{mp}:{event}u"),
    }
    .into_bytes()
}

fn csi_tilde(n: u8, k: Key, event: u8) -> Vec<u8> {
    let mp = modifier_param(k);
    if mp == 1 && event == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{mp}:{event}~") }.into_bytes()
}

fn legacy_with_event(k: Key, event: u8) -> Option<Vec<u8>> {
    let mp = modifier_param(k);
    let letter = |l: char| Some(format!("\x1b[1;{mp}:{event}{l}").into_bytes());
    match k.name {
        "up" => letter('A'),
        "down" => letter('B'),
        "right" => letter('C'),
        "left" => letter('D'),
        "home" => letter('H'),
        "end" => letter('F'),
        "pageup" => Some(csi_tilde(5, k, event)),
        "pagedown" => Some(csi_tilde(6, k, event)),
        _ => None,
    }
}

/// Encode a key under the kitty keyboard protocol, or None for the
/// legacy encoding. `event` is 1 press, 2 repeat, 3 release.
pub fn kitty_key(k: Key, mode: TermMode, event: u8) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    if event == 3 && !events {
        return None;
    }
    let event = if events { event } else { 1 };
    let code = kitty_code(k.name);
    let functional = matches!(k.name, "escape" | "enter" | "tab" | "backspace");
    let insert_delete = matches!(k.name, "insert" | "delete");
    let text_key = code.is_some() && !functional && !insert_delete;
    if let Some(code) = code {
        if all {
            return Some(if insert_delete { csi_tilde(code as u8, k, event) } else { csi_u(code, k, event) });
        }
        if functional {
            // Esc is always disambiguated; Enter, Tab and Backspace only
            // with modifiers (plain ones keep their legacy bytes).
            let plain = !k.ctrl && !k.alt && !k.shift;
            return (k.name == "escape" || !plain || event != 1).then(|| csi_u(code, k, event));
        }
        if text_key && (k.ctrl || k.alt) {
            return Some(csi_u(code, k, event));
        }
        if text_key {
            // Plain typing stays text (releases are reported when asked).
            return (event == 3).then(|| csi_u(code, k, event));
        }
    }
    // Arrows, F-keys and friends keep their legacy form (with the event
    // type when reporting them).
    if event != 1 {
        return legacy_with_event(k, event);
    }
    None
}

/// What a key pressed (or let go) does to the screen: bytes for the
/// program, the history scrolled; false if it's left to the text it types.
/// `pressed`, `repeat`: the event; Ctrl here is the shell's (the command
/// key, ⌘, was taken out by the caller: it's Terminal's or goes on as
/// Ctrl+key).
pub fn key(s: &Session, k: Key, pressed: bool, repeat: bool) -> bool {
    if s.exited.is_some() {
        return false;
    }
    let mode = *s.term.lock().mode();
    if pressed && k.shift && !mode.contains(TermMode::ALT_SCREEN) {
        // Shift+Page Up/Down scroll the history, as everywhere.
        let page = s.size.rows as i32 - 1;
        let scroll = match k.name {
            "pageup" => Some(Scroll::Delta(page)),
            "pagedown" => Some(Scroll::Delta(-page)),
            "home" => Some(Scroll::Top),
            "end" => Some(Scroll::Bottom),
            _ => None,
        };
        if let Some(sc) = scroll {
            s.term.lock().scroll_display(sc);
            return true;
        }
    }
    if mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        let event = if !pressed {
            3
        } else if repeat {
            2
        } else {
            1
        };
        if let Some(bytes) = kitty_key(k, mode, event) {
            s.write(bytes);
            if pressed {
                s.typed();
            }
            return true;
        }
    }
    if !pressed {
        return false;
    }
    match key_bytes(k, mode) {
        Some(bytes) => {
            s.write(bytes);
            s.typed();
            true
        }
        None => false,
    }
}

/// Text typed (or an input method's): to the program.
pub fn text(s: &Session, text: &str) {
    if s.exited.is_some() || text.is_empty() {
        return;
    }
    s.write(text.as_bytes().to_vec());
    s.typed();
}

/// Send pasted text, bracketed when the program asked for that (so a
/// pasted command doesn't run until the user presses Return).
pub fn paste(s: &Session, text: &str) {
    if text.is_empty() {
        return;
    }
    let mode = *s.term.lock().mode();
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A paste can't end the bracket early.
        let clean = text.replace("\x1b[201~", "");
        s.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
    } else {
        s.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
    }
    s.typed();
}

// ---- mouse ------------------------------------------------------------------

/// The cell at `pos` (window coordinates) on the screen drawn in `rect`.
fn cell_at(pos: gpui::Point<Pixels>, rect: Bounds<Pixels>, m: &Metrics, size: Size, offset: usize) -> (Point, Side) {
    let x = (f32::from(pos.x - rect.left()) - PAD.0).max(0.0);
    let y = (f32::from(pos.y - rect.top()) - PAD.1).max(0.0);
    let col = ((x / m.cell.0) as usize).min(size.cols - 1);
    let row = ((y / m.cell.1) as usize).min(size.rows - 1);
    let side = if x - col as f32 * m.cell.0 > m.cell.0 / 2.0 { Side::Right } else { Side::Left };
    (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
}

/// The modifiers as the program is told them.
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// The command key (⌘): links, not the program's.
    pub command: bool,
}

impl Mods {
    pub fn of(m: &Modifiers, command: bool) -> Mods {
        Mods { shift: m.shift, alt: m.alt, ctrl: m.control && !command, command }
    }
}

/// SGR mouse report (1006).
fn report(s: &Session, button: u8, point: Point, offset: usize, pressed: bool, m: Mods) {
    let mods = 4 * m.shift as u8 + 8 * m.alt as u8 + 16 * m.ctrl as u8;
    let line = point.line.0 + offset as i32 + 1;
    let col = point.column.0 + 1;
    let fin = if pressed { 'M' } else { 'm' };
    s.write(format!("\x1b[<{};{col};{line}{fin}", button + mods).into_bytes());
}

fn button_code(b: gpui::MouseButton) -> Option<u8> {
    match b {
        gpui::MouseButton::Left => Some(0),
        gpui::MouseButton::Middle => Some(1),
        gpui::MouseButton::Right => Some(2),
        _ => None,
    }
}

impl Mouse {
    /// The link under the pointer, while ⌘ is held: found once a cell.
    pub fn hover_at(&mut self, s: &Session, m: &Metrics, rect: Bounds<Pixels>, pos: Option<gpui::Point<Pixels>>, command: bool, cwd: &dyn Fn() -> Option<std::path::PathBuf>) -> bool {
        let before = self.hover.as_ref().map(|h| h.cells.clone());
        match pos.filter(|p| command && !self.selecting && rect.contains(p)) {
            Some(pos) => {
                let offset = s.term.lock().grid().display_offset();
                let (pt, _) = cell_at(pos, rect, m, s.size, offset);
                if self.hover.as_ref().is_none_or(|h| h.at != pt) {
                    self.hover = Some(find_link(s, pt, cwd));
                }
            }
            None => self.hover = None,
        }
        self.hover.as_ref().map(|h| h.cells.clone()) != before
    }

    /// A scroll over the screen (`dy` points, up positive; from `kind`,
    /// in `phase`): the scrollback moved by as much (a program that takes
    /// the mouse, or on the alternate screen, gets it a line at a time).
    #[allow(clippy::too_many_arguments)]
    pub fn wheel(&mut self, s: &Session, pane: &Pane, m: &Metrics, rect: Bounds<Pixels>, pos: gpui::Point<Pixels>, dy: f32, (kind, phase): (gpui::ScrollKind, gpui::TouchPhase), mods: Mods) {
        let (mode, offset) = {
            let t = s.term.lock();
            (*t.mode(), t.grid().display_offset())
        };
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;
        if !reporting && !mode.contains(TermMode::ALT_SCREEN) {
            let mut t = s.term.lock();
            let history = t.grid().history_size();
            let ch = m.cell.1;
            let mut sm = pane.smooth.borrow_mut();
            let (mut at, max) = sm.sync(offset, history, t.screen_lines() as f32 * ch, ch);
            sm.elastic.scroll(&mut at, point(px(0.0), px(dy)), kind, phase);
            sm.pos = (f32::from(at.y) + max).clamp(0.0, max);
            let rows = ((sm.pos / ch).floor() as usize).min(history);
            if rows != offset {
                t.scroll_display(Scroll::Delta(rows as i32 - offset as i32));
            }
            sm.rows = rows;
            return;
        }
        let lines = dy / m.cell.1;
        self.scroll += lines;
        let n = self.scroll.trunc() as i32;
        self.scroll -= n as f32;
        if n == 0 {
            return;
        }
        if reporting {
            let (pt, _) = cell_at(pos, rect, m, s.size, offset);
            for _ in 0..n.unsigned_abs() {
                report(s, if n > 0 { 64 } else { 65 }, pt, offset, true, mods);
            }
        } else if mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key = if n > 0 { "A" } else { "B" };
            let seq = if mode.contains(TermMode::APP_CURSOR) { format!("\x1bO{key}") } else { format!("\x1b[{key}") };
            s.write(seq.repeat(n.unsigned_abs() as usize).into_bytes());
        }
    }

    /// A button went down on the screen.
    pub fn down(&mut self, s: &Session, m: &Metrics, rect: Bounds<Pixels>, pos: gpui::Point<Pixels>, button: gpui::MouseButton, count: usize, mods: Mods, out: &mut Output) {
        let (mode, offset) = {
            let t = s.term.lock();
            (*t.mode(), t.grid().display_offset())
        };
        let (point, side) = cell_at(pos, rect, m, s.size, offset);
        // ⌘-click on a link opens it, even in a program that takes the mouse.
        if button == gpui::MouseButton::Left
            && mods.command
            && let Some(target) = self.hover.as_ref().and_then(|h| h.target.clone())
        {
            out.open = Some(target);
            self.last_cell = Some(point);
            return;
        }
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
            if let Some(b) = button_code(button) {
                self.reporting = Some(b);
                report(s, b, point, offset, true, mods);
            }
            self.last_cell = Some(point);
            return;
        }
        self.last_cell = Some(point);
        match button {
            gpui::MouseButton::Middle => out.paste = true,
            gpui::MouseButton::Right => out.context_menu = Some(pos),
            gpui::MouseButton::Left => {
                // Selection: drag for characters, double-click words,
                // triple-click lines; Shift extends.
                let now = std::time::Instant::now();
                let count = match self.last_click {
                    Some((t, p, n)) if now.duration_since(t).as_millis() < 400 && p == point => n % 3 + 1,
                    _ => count.clamp(1, 3) as u8,
                };
                self.last_click = Some((now, point, count));
                let mut t = s.term.lock();
                if mods.shift && t.selection.is_some() {
                    if let Some(sel) = t.selection.as_mut() {
                        sel.update(point, side);
                    }
                } else {
                    let ty = match count {
                        2 => SelectionType::Semantic,
                        3 => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    };
                    let mut sel = Selection::new(ty, point, side);
                    if count > 1 {
                        // Words and lines select on the click itself.
                        sel.update(point, side);
                    }
                    t.selection = Some(sel);
                }
                self.selecting = true;
            }
            _ => {}
        }
    }

    /// The pointer moved (a button held or not): drags select or report.
    /// Returns the lines to scroll while a selection is dragged past an
    /// edge (the caller keeps scrolling while it's held there).
    pub fn moved(&mut self, s: &Session, m: &Metrics, rect: Bounds<Pixels>, pos: gpui::Point<Pixels>, held: bool, mods: Mods) -> bool {
        let (mode, offset) = {
            let t = s.term.lock();
            (*t.mode(), t.grid().display_offset())
        };
        let (point, side) = cell_at(pos, rect, m, s.size, offset);
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
            if self.last_cell != Some(point) {
                if let Some(b) = self.reporting.filter(|_| mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)) {
                    report(s, 32 + b, point, offset, true, mods);
                } else if self.reporting.is_none() && mode.contains(TermMode::MOUSE_MOTION) && rect.contains(&pos) {
                    report(s, 35, point, offset, true, mods);
                }
            }
            self.last_cell = Some(point);
            return false;
        }
        self.last_cell = Some(point);
        if !(self.selecting && held) {
            return false;
        }
        let mut t = s.term.lock();
        if let Some(sel) = t.selection.as_mut() {
            sel.update(point, side);
        }
        // Past the top or bottom: a line scrolled.
        let past = if pos.y < rect.top() {
            1
        } else if pos.y > rect.bottom() {
            -1
        } else {
            0
        };
        if past != 0 {
            t.scroll_display(Scroll::Delta(past));
        }
        true
    }

    /// A button went up.
    pub fn up(&mut self, s: &Session, m: &Metrics, rect: Bounds<Pixels>, pos: gpui::Point<Pixels>, button: gpui::MouseButton, mods: Mods) {
        let offset = s.term.lock().grid().display_offset();
        let (point, _) = cell_at(pos, rect, m, s.size, offset);
        if let Some(b) = self.reporting.filter(|&b| Some(b) == button_code(button)) {
            self.reporting = None;
            report(s, b, point, offset, false, mods);
            return;
        }
        if button == gpui::MouseButton::Left && self.selecting {
            self.selecting = false;
            let mut t = s.term.lock();
            // A plain click (no drag) clears the selection.
            if t.selection.as_ref().is_some_and(|sel| sel.is_empty()) {
                t.selection = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(name: &str, shift: bool, ctrl: bool, alt: bool) -> Key<'_> {
        Key { name, shift, ctrl, alt }
    }

    #[test]
    fn option_arrows_move_by_word() {
        assert_eq!(key_bytes(k("left", false, false, true), TermMode::empty()).unwrap(), b"\x1bb");
        assert_eq!(key_bytes(k("right", false, false, true), TermMode::empty()).unwrap(), b"\x1bf");
        // Under the kitty protocol, the protocol's.
        assert_eq!(key_bytes(k("left", false, false, true), TermMode::KITTY_KEYBOARD_PROTOCOL).unwrap(), b"\x1b[1;3D");
    }

    #[test]
    fn ctrl_underscore_is_the_shells() {
        // Ctrl+Shift+- (GPUI names it "_") is Ctrl+_, readline's undo.
        assert_eq!(key_bytes(k("-", true, true, false), TermMode::empty()).unwrap(), vec![0x1f]);
        assert_eq!(key_bytes(k("2", true, true, false), TermMode::empty()).unwrap(), vec![0]);
        assert_eq!(key_bytes(k("c", false, true, false), TermMode::empty()).unwrap(), vec![3]);
    }

    #[test]
    fn alt_letters_are_escaped() {
        assert_eq!(key_bytes(k("b", false, false, true), TermMode::empty()).unwrap(), b"\x1bb");
        assert_eq!(key_bytes(k("b", true, false, true), TermMode::empty()).unwrap(), b"\x1bB");
        assert_eq!(key_bytes(k("b", false, false, false), TermMode::empty()), None);
    }
}
