//! Drawing the terminal grid and turning keys and the mouse into bytes for
//! the program inside.
//!
//! Every cell sits on a grid snapped to device pixels: runs of ordinary
//! characters are laid out as one galley with letter spacing padding each
//! glyph to the cell width; wide characters and glyphs from fallback fonts
//! are placed cell by cell; box-drawing and block characters are drawn as
//! shapes so lines join seamlessly.
//!
//! Each row is laid out and tessellated once (`RowCache`): a frame that
//! changes one row (an agent's spinner, a line typed) rebuilds that row's
//! meshes and hands egui the rest as they were, so it neither lays out nor
//! tessellates the screen again. The cursor, the hovered link, colour
//! emoji and pictures are drawn live over them.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags},
    vte::ansi::{Color, CursorShape, NamedColor},
};
use olo_kit::egui::{
    self, Color32, FontFamily, FontId, Key, Pos2, Rect, Sense, Stroke, Ui, pos2, text::LayoutJob, vec2,
};
use olo_kit::{emoji, style};

use crate::{
    colors::{Scheme, c32, mix},
    graphics, links,
    session::{Session, Size},
};

/// Space between the window edge and the text, in logical pixels: across,
/// a window's inset, so the text is under the menu bar's first title and
/// the tabs' icons; less down, as a cell's line box already has some.
pub const PAD: egui::Vec2 = egui::vec2(olo_kit::metrics::WINDOW_INSET, 6.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub size: f32,
    /// Cell size, a whole number of device pixels.
    pub cell: egui::Vec2,
    /// The font's own advance (the galley's glyph spacing).
    advance: f32,
    /// Galley top to baseline within a cell.
    ascent: f32,
    row: f32,
    pub ppp: f32,
}

impl Metrics {
    pub fn new(ctx: &egui::Context, size: f32) -> Metrics {
        let ppp = ctx.pixels_per_point();
        let font = FontId::monospace(size);
        let (advance, row) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let snap = |v: f32| (v * ppp).ceil() / ppp;
        let cell = vec2(snap(advance), snap(row));
        // Centre the font's line box in the (slightly taller) cell.
        let ascent = ((cell.y - row) / 2.0 * ppp).round() / ppp;
        Metrics { size, cell, advance, ascent, row, ppp }
    }

    /// Cell size in device pixels, for the PTY's pixel-size report.
    pub fn device_cell(&self) -> (u16, u16) {
        ((self.cell.x * self.ppp).round() as u16, (self.cell.y * self.ppp).round() as u16)
    }

    /// How many cells fit in `area`.
    pub fn grid_size(&self, area: egui::Vec2) -> Size {
        Size {
            cols: (((area.x - 2.0 * PAD.x) / self.cell.x).floor() as usize).max(2),
            rows: (((area.y - 2.0 * PAD.y) / self.cell.y).floor() as usize).max(1),
        }
    }

    /// Content size for a `cols`×`rows` window.
    pub fn content_size(&self, cols: usize, rows: usize) -> (u32, u32) {
        (
            (cols as f32 * self.cell.x + 2.0 * PAD.x).ceil() as u32,
            (rows as f32 * self.cell.y + 2.0 * PAD.y).ceil() as u32,
        )
    }
}

/// A screen's background and text colours as it draws them (a program
/// may have changed them).
pub fn screen_colors(s: &Session, scheme: &Scheme) -> (Color32, Color32) {
    // (Its own while being reflowed, the scheme's.)
    let term = if s.reflowing() { s.term.try_lock_unfair() } else { Some(s.term.lock()) };
    let scheme_only = alacritty_terminal::term::color::Colors::default();
    let colors = term.as_ref().map_or(&scheme_only, |t| t.colors());
    let named = |n| c32(scheme.resolve(Color::Named(n), colors));
    (named(NamedColor::Background), named(NamedColor::Foreground))
}

/// Mouse state between frames.
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
    /// The same for a selection dragged past the top or bottom.
    edge: f32,
    /// The link under the pointer while ⌘ is held: its cells and target.
    hover: Option<Hover>,
    /// Image textures by image id, with the generation they were made from.
    textures: HashMap<u32, (u64, egui::TextureHandle)>,
    /// The screen's rows as last tessellated.
    rows: RowCache,
}

/// The screen's rows as last drawn: for each, a hash of what it was made
/// from and its tessellated backgrounds and foreground (text, boxes,
/// lines). Dropped whole when anything they were drawn for changes (the
/// cell metrics, scale, colours, where the screen is, the font atlas).
#[derive(Default)]
struct RowCache {
    key: u64,
    rows: Vec<Option<(u64, Arc<egui::Mesh>, Arc<egui::Mesh>)>>,
}

/// A row's cells, gathered before they're drawn (only rows that changed are).
#[derive(Default)]
struct RowBuf {
    hash: u64,
    /// Backgrounds: column, width in cells, colour.
    bgs: Vec<(usize, f32, Color32)>,
    runs: Vec<(usize, String, Style)>,
    singles: Vec<(usize, char, Style, bool)>,
    boxes: Vec<(usize, char, Color32)>,
    lines: Vec<(Rect, Color32)>,
}

/// FNV-1a over 64-bit words: what a row is made from, cheaply.
fn mix_in(h: &mut u64, v: u64) {
    *h = (*h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
}

/// Where shapes go to be tessellated once rather than painted: the calls
/// `draw_box` makes on a painter.
struct Shapes<'a> {
    ctx: &'a egui::Context,
    list: RefCell<Vec<egui::Shape>>,
}

impl<'a> Shapes<'a> {
    fn new(ctx: &'a egui::Context) -> Self {
        Shapes { ctx, list: RefCell::new(Vec::new()) }
    }
    fn rect_filled(&self, r: Rect, _radius: f32, color: Color32) {
        self.list.borrow_mut().push(egui::Shape::rect_filled(r, 0.0, color));
    }
    fn add(&self, shape: egui::Shape) {
        self.list.borrow_mut().push(shape);
    }
    fn layout_no_wrap(&self, text: String, font: FontId, color: Color32) -> Arc<egui::Galley> {
        self.ctx.fonts_mut(|f| f.layout_no_wrap(text, font, color))
    }
    fn galley(&self, pos: Pos2, galley: Arc<egui::Galley>, color: Color32) {
        self.list.borrow_mut().push(egui::Shape::galley(pos, galley, color));
    }
    /// The shapes as one mesh (they all draw from the font texture).
    fn tessellate(self) -> Arc<egui::Mesh> {
        let ctx = self.ctx;
        let font_size = ctx.fonts(|f| f.font_image_size());
        let options = ctx.tessellation_options(|o| *o);
        let mut t = egui::epaint::Tessellator::new(ctx.pixels_per_point(), options, font_size, Vec::new());
        let mut mesh = egui::Mesh::default();
        for shape in self.list.into_inner() {
            t.tessellate_shape(shape, &mut mesh);
        }
        Arc::new(mesh)
    }
}

#[derive(Clone)]
struct Hover {
    /// Grid point the pointer was over when this was computed.
    at: Point,
    cells: Vec<Point>,
    target: Option<links::Target>,
}

/// Which chars are drawn from the primary monospace face at its normal
/// advance (cached: asking egui for each cell every frame is wasteful).
#[derive(Default)]
pub struct GlyphCache {
    simple: HashMap<char, bool>,
    for_size: f32,
}

impl GlyphCache {
    fn simple(&mut self, ui: &Ui, m: &Metrics, c: char) -> bool {
        if self.for_size != m.size {
            self.simple.clear();
            self.for_size = m.size;
        }
        if c.is_ascii() {
            return true;
        }
        *self.simple.entry(c).or_insert_with(|| {
            let font = FontId::monospace(m.size);
            ui.fonts_mut(|f| (f.glyph_width(&font, c) - m.advance).abs() < 0.01)
        })
    }
}

/// A frame of the screen as drawn: its background and shapes, for the
/// frames while the screen is reflowed to a new width on a worker
/// (`Session::resize`).
pub struct Frame {
    background: Color32,
    shapes: Vec<egui::Shape>,
}

pub struct Output {
    /// The grid size the window now has room for.
    pub size: Size,
    pub copy: Option<String>,
    pub paste: bool,
    pub context_menu: Option<Pos2>,
    /// A link ⌘-clicked.
    pub open: Option<links::Target>,
}

impl Output {
    /// Take in another pane's: what it asked for, if this one didn't.
    pub fn merge(&mut self, o: Output) {
        self.copy = self.copy.take().or(o.copy);
        self.paste |= o.paste;
        self.context_menu = self.context_menu.or(o.context_menu);
        self.open = self.open.take().or(o.open);
    }
}

fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c) || ('\u{e0b0}'..='\u{e0b3}').contains(&c)
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Color32,
    bold: bool,
    italic: bool,
}

/// Draw the session into `ui` and handle input. `focused` is whether the
/// window has the keyboard.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    s: &mut Session,
    scheme: &Scheme,
    m: &Metrics,
    glyphs: &mut GlyphCache,
    mouse: &mut Mouse,
    focused: bool,
    cwd: &dyn Fn() -> Option<std::path::PathBuf>,
) -> Output {
    let rect = ui.max_rect();
    let resp = ui.interact(rect, ui.make_persistent_id("terminal"), Sense::click_and_drag());
    let size = m.grid_size(rect.size());
    let mut out = Output { size, copy: None, paste: false, context_menu: None, open: None };
    if focused && s.exited.is_none() {
        keyboard(ui, s, &mut out);
    }
    // ⌘ held over a link (the command key: Ctrl with the Windows keys):
    // find it, once per cell the pointer is on. Before the pointer's
    // handling, so a click acts on the link under it now: a ⌘-click that
    // focuses the window is the first frame it knows ⌘ is held.
    let link_mode = ui.input(|i| i.modifiers.command) && resp.hovered() && !mouse.selecting;
    if link_mode {
        if let Some(pos) = resp.hover_pos() {
            let offset = s.term.lock().grid().display_offset();
            let (pt, _) = cell_at(pos, rect, m, s.size, offset);
            if mouse.hover.as_ref().is_none_or(|h| h.at != pt) {
                mouse.hover = Some(find_link(s, pt, cwd));
            }
        }
    } else {
        mouse.hover = None;
    }
    pointer(ui, s, m, &resp, rect, mouse, &mut out);
    let painter = ui.painter_at(rect);
    let origin = rect.min + PAD;
    // Its lines are a cell apart, not on the window's grid.
    olo_kit::layout_review::own_grid(ui, rect);
    if mouse.hover.as_ref().is_some_and(|h| h.target.is_some()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // Being reflowed to a new width: the last frame again, while that
    // takes (the new width shows the next frame).
    let term = if s.reflowing() { s.term.try_lock_unfair() } else { Some(s.term.lock()) };
    let Some(term) = term else {
        if let Some(f) = &s.last_frame {
            painter.rect_filled(rect, 0.0, f.background);
            painter.extend(f.shapes.iter().cloned());
        }
        return out;
    };
    // A frame drawn as the window changes size is kept for that.
    let keep = size != s.size || s.reflowing();
    let layer = painter.layer_id();
    let first = if keep { ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.all_entries().len())) } else { 0 };
    let content = term.renderable_content();
    let colors = content.colors;
    let bg_default = scheme.resolve(Color::Named(NamedColor::Background), colors);
    painter.rect_filled(rect, 0.0, c32(bg_default));
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden;
    let cursor = content.cursor;

    let cols = term.columns();
    let rows = term.screen_lines();
    let cell = m.cell;
    let at = |line: i32, col: usize| pos2(origin.x + col as f32 * cell.x, origin.y + (line + offset) as f32 * cell.y);

    // Each row's cells: backgrounds, runs of simple glyphs, glyphs placed
    // one by one, boxes and lines; drawn (and tessellated) only for rows
    // that changed. Emoji and their fallbacks are drawn live.
    let mut bufs: Vec<RowBuf> = (0..rows).map(|_| RowBuf { hash: 0xcbf2_9ce4_8422_2325, ..RowBuf::default() }).collect();
    let row_of = |line: i32| (line + offset).clamp(0, rows as i32 - 1) as usize;
    let mut runs: Vec<(i32, usize, String, Style)> = Vec::new();
    let mut singles: Vec<(i32, usize, char, Style, bool)> = Vec::new();
    let mut cursor_cell: Option<(char, Style, bool)> = None;
    let mut image_cells: Vec<(i32, usize, u32, usize)> = Vec::new();
    // Colour emoji: (line, column, cluster, style, cells).
    let mut emoji: Vec<(i32, usize, String, Style, usize)> = Vec::new();
    // Cells with something in them, so a narrow emoji can use a free one.
    let mut used: std::collections::HashSet<(i32, usize)> = std::collections::HashSet::new();

    for indexed in content.display_iter {
        let point = indexed.point;
        let cell_ = indexed.cell;
        let flags = cell_.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let col = point.column.0;
        let line = point.line.0;
        if let Some(h) = cell_.hyperlink()
            && let Some((uid, row)) = graphics::parse_uri(h.uri())
        {
            image_cells.push((line, col, uid, row));
        }
        let mut fg = scheme.resolve(cell_.fg, colors);
        let mut bg = scheme.resolve(cell_.bg, colors);
        if flags.contains(Flags::DIM) {
            fg = mix(fg, bg, 0.66);
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = selection.is_some_and(|r| r.contains(point));
        if selected {
            bg = scheme.selection;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2.0 } else { 1.0 };
        let buf = &mut bufs[row_of(line)];
        // What this cell adds to its row's look.
        mix_in(&mut buf.hash, col as u64 | (cell_.c as u64) << 16 | (flags.bits() as u64) << 40);
        mix_in(&mut buf.hash, (c32(fg).to_array().iter().fold(0u64, |a, &b| a << 8 | b as u64)) << 32 | c32(bg).to_array().iter().fold(0u64, |a, &b| a << 8 | b as u64));
        if let Some(z) = cell_.zerowidth() {
            for ch in z {
                mix_in(&mut buf.hash, *ch as u64);
            }
        }
        if let Some(u) = cell_.underline_color() {
            mix_in(&mut buf.hash, c32(scheme.resolve(u, colors)).to_array().iter().fold(1u64, |a, &b| a << 8 | b as u64));
        }
        if bg != bg_default {
            buf.bgs.push((col, width, c32(bg)));
        }
        let style = Style { fg: c32(fg), bold: flags.contains(Flags::BOLD), italic: flags.contains(Flags::ITALIC) };
        let is_cursor = show_cursor && point == cursor.point;
        if is_cursor {
            cursor_cell = Some((cell_.c, style, wide));
        }
        if flags.contains(Flags::HIDDEN) {
            continue;
        }
        // Underline and strike-through.
        let p = at(line, col);
        let w = cell.x * width;
        let px = 1.0 / m.ppp;
        if flags.intersects(Flags::ALL_UNDERLINES) {
            let uc = cell_.underline_color().map(|c| c32(scheme.resolve(c, colors))).unwrap_or(style.fg);
            let y = p.y + m.ascent + m.row * 0.86;
            let t = (m.size / 12.0).round().max(1.0) * px.max(0.5);
            buf.lines.push((Rect::from_min_size(pos2(p.x, y), vec2(w, t)), uc));
            if flags.contains(Flags::DOUBLE_UNDERLINE) {
                buf.lines.push((Rect::from_min_size(pos2(p.x, y + 2.0 * t), vec2(w, t)), uc));
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            let y = p.y + m.ascent + m.row * 0.52;
            buf.lines.push((Rect::from_min_size(pos2(p.x, y), vec2(w, px.max(0.5))), style.fg));
        }
        let c = cell_.c;
        if c == ' ' || c == '\0' {
            continue;
        }
        used.insert((line, col));
        if is_box(c) {
            buf.boxes.push((col, c, style.fg));
        } else if wide || !glyphs.simple(ui, m, c) || cell_.zerowidth().is_some() {
            let mut cluster = String::from(c);
            cluster.extend(cell_.zerowidth().into_iter().flatten());
            let cells = if wide { 2 } else { 1 };
            // alacritty gives each part of a sequence its own cell: join
            // skin tones, ZWJ sequences and flags back up.
            if let Some(prev) = emoji.last_mut()
                && prev.0 == line
                && prev.1 + prev.4 == col
                && emoji::continues(&prev.2, &cluster)
            {
                prev.2.push_str(&cluster);
                prev.4 += cells;
                continue;
            }
            if emoji::wants_color(&cluster, wide) {
                emoji.push((line, col, cluster, style, cells));
                continue;
            }
            if let Some(z) = cell_.zerowidth() {
                // Combining marks: lay the cluster out together.
                let mut t = String::from(c);
                t.extend(z);
                buf.runs.push((col, t, style));
            } else {
                buf.singles.push((col, c, style, wide));
            }
        } else if let Some(last) = buf.runs.last_mut()
            && last.2 == style
            && last.0 + last.1.chars().count() == col
            && last.1.chars().all(|ch| glyphs.simple(ui, m, ch))
        {
            last.1.push(c);
        } else {
            buf.runs.push((col, c.to_string(), style));
        }
    }

    let font_for = |st: Style| {
        let family = if st.bold {
            style::mono_bold()
        } else if st.italic {
            style::mono_italic()
        } else {
            FontFamily::Monospace
        };
        FontId::new(m.size, family)
    };
    let spacing = cell.x - m.advance;
    let run_galley = |text: &str, st: Style| {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, egui::TextFormat { font_id: font_for(st), color: st.fg, extra_letter_spacing: spacing, ..Default::default() });
        ui.fonts_mut(|f| f.layout_job(job))
    };

    // The rows: as they were where nothing changed, else drawn and
    // tessellated again. Every row's backgrounds go under every row's
    // text, so a glyph reaching into the next row isn't covered.
    let ctx = ui.ctx().clone();
    let cache_key = {
        let mut k = 0xcbf2_9ce4_8422_2325u64;
        for v in [m.size.to_bits() as u64, cell.x.to_bits() as u64, cell.y.to_bits() as u64, m.ppp.to_bits() as u64, origin.x.to_bits() as u64, origin.y.to_bits() as u64, cols as u64, rows as u64] {
            mix_in(&mut k, v);
        }
        mix_in(&mut k, c32(bg_default).to_array().iter().fold(0u64, |a, &b| a << 8 | b as u64));
        mix_in(&mut k, ctx.fonts(|f| f.galley_generation()));
        let [w, h] = ctx.fonts(|f| f.font_image_size());
        mix_in(&mut k, (w as u64) << 32 | h as u64);
        k
    };
    let cache = &mut mouse.rows;
    if cache.key != cache_key {
        cache.key = cache_key;
        cache.rows.clear();
    }
    cache.rows.resize(rows, None);
    let mut fronts = Vec::with_capacity(rows);
    for (r, buf) in bufs.into_iter().enumerate() {
        let (back, front) = match &cache.rows[r] {
            Some((h, back, front)) if *h == buf.hash => (back.clone(), front.clone()),
            _ => {
                let line = r as i32 - offset;
                let back_shapes = Shapes::new(&ctx);
                for (col, width, color) in &buf.bgs {
                    back_shapes.rect_filled(Rect::from_min_size(at(line, *col), vec2(cell.x * width, cell.y)), 0.0, *color);
                }
                let front_shapes = Shapes::new(&ctx);
                for (col, text, st) in &buf.runs {
                    let p = at(line, *col);
                    front_shapes.galley(pos2(p.x, p.y + m.ascent), run_galley(text, *st), st.fg);
                }
                for (col, c, st, wide) in &buf.singles {
                    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(c.to_string(), font_for(*st), st.fg));
                    let p = at(line, *col);
                    let w = if *wide { cell.x * 2.0 } else { cell.x };
                    // Centre glyphs that aren't cell-sized in their cell(s).
                    let x = p.x + ((w - galley.size().x) / 2.0 * m.ppp).round() / m.ppp;
                    front_shapes.galley(pos2(x, p.y + m.ascent), galley, st.fg);
                }
                for (col, c, color) in &buf.boxes {
                    draw_box(&front_shapes, Rect::from_min_size(at(line, *col), cell), *c, *color, m.ppp);
                }
                for (rr, c) in &buf.lines {
                    front_shapes.rect_filled(*rr, 0.0, *c);
                }
                let (back, front) = (back_shapes.tessellate(), front_shapes.tessellate());
                cache.rows[r] = Some((buf.hash, back.clone(), front.clone()));
                (back, front)
            }
        };
        if !back.is_empty() {
            painter.add(egui::Shape::Mesh(back));
        }
        fronts.push(front);
    }
    for front in fronts {
        if !front.is_empty() {
            painter.add(egui::Shape::Mesh(front));
        }
    }

    draw_images(ui, &painter, s, &image_cells, &mut mouse.textures, at, cell);

    // Colour emoji, else (no colour glyph) the cluster as text.
    for (line, col, cluster, st, cells) in emoji {
        // Two cells wide, as an emoji should be: a narrow one borrows a
        // free cell beside it; a sequence alacritty spread wider is centred
        // over its cells.
        let span = if cells == 1 && col + 1 < cols && !used.contains(&(line, col + 1)) { 2 } else { cells };
        let wide = cells >= 2;
        let full = Rect::from_min_size(at(line, col), vec2(cell.x * span as f32, cell.y));
        let r = Rect::from_center_size(full.center(), vec2(cell.x * span.min(2) as f32, cell.y));
        if !emoji::paint(&painter, r, &cluster) {
            let mut chars = cluster.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => singles.push((line, col, c, st, wide)),
                _ => runs.push((line, col, cluster, st)),
            }
        }
    }
    for (line, col, text, st) in runs {
        let p = at(line, col);
        painter.galley(pos2(p.x, p.y + m.ascent), run_galley(&text, st), st.fg);
    }
    for (line, col, c, st, wide) in singles {
        if c == '\u{0}' {
            continue;
        }
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(c.to_string(), font_for(st), st.fg));
        let p = at(line, col);
        let w = if wide { cell.x * 2.0 } else { cell.x };
        // Centre glyphs that aren't cell-sized in their cell(s).
        let x = p.x + ((w - galley.size().x) / 2.0 * m.ppp).round() / m.ppp;
        painter.galley(pos2(x, p.y + m.ascent), galley, st.fg);
    }
    // The hovered link, underlined.
    if let Some(h) = &mouse.hover
        && h.target.is_some()
    {
        let lc = c32(scheme.cursor);
        let t = (1.0 / m.ppp).max(0.5) * 2.0;
        for p in &h.cells {
            let tl = at(p.line.0, p.column.0);
            painter.rect_filled(Rect::from_min_size(pos2(tl.x, tl.y + m.ascent + m.row * 0.86), vec2(cell.x, t)), 0.0, lc);
        }
    }

    // The cursor: a solid block when focused (with its character in the
    // background colour), an outline when not.
    if let Some((c, st, wide)) = cursor_cell.or(show_cursor.then_some((' ', Style { fg: Color32::BLACK, bold: false, italic: false }, false))) {
        let p = at(cursor.point.line.0, cursor.point.column.0);
        let w = if wide { cell.x * 2.0 } else { cell.x };
        let cc = c32(scheme.resolve(Color::Named(NamedColor::Cursor), colors));
        let r = Rect::from_min_size(p, vec2(w, cell.y));
        let px = 1.0 / m.ppp;
        match (focused, cursor.shape) {
            (true, CursorShape::Beam) => {
                painter.rect_filled(Rect::from_min_size(p, vec2((2.0 * px).max(1.0), cell.y)), 0.0, cc);
            }
            (true, CursorShape::Underline) => {
                painter.rect_filled(Rect::from_min_max(pos2(r.min.x, r.max.y - 2.0 * px.max(0.5) * 2.0), r.max), 0.0, cc);
            }
            (true, _) => {
                painter.rect_filled(r, 0.0, cc);
                if c != ' ' && c != '\0' && !is_box(c) {
                    let galley = ui.fonts_mut(|f| f.layout_no_wrap(c.to_string(), font_for(st), c32(bg_default)));
                    let x = p.x + ((w - galley.size().x) / 2.0 * m.ppp).round() / m.ppp;
                    painter.galley(pos2(x, p.y + m.ascent), galley, c32(bg_default));
                }
            }
            (false, _) => {
                let inset = px.max(0.5);
                painter.rect_stroke(r.shrink(inset / 2.0), 0.0, Stroke::new(inset * 2.0, cc), egui::StrokeKind::Inside);
            }
        }
    }

    // Scrollback position, shown only while scrolled back.
    if offset > 0 {
        let history = term.history_size().max(1) as f32;
        let track = rect.shrink2(vec2(2.0, PAD.y));
        let visible = rows as f32 / (history + rows as f32);
        let h = (track.height() * visible).max(16.0);
        let t = 1.0 - offset as f32 / history;
        let y = track.min.y + (track.height() - h) * t;
        let thumb = Rect::from_min_size(pos2(track.max.x - 4.0, y), vec2(4.0, h));
        let fg = scheme.resolve(Color::Named(NamedColor::Foreground), colors);
        painter.rect_filled(thumb, 2.0, c32(mix(fg, bg_default, 0.35)));
    }
    drop(term);
    s.last_frame = keep.then(|| Frame {
        background: c32(bg_default),
        shapes: ui.ctx().graphics(|g| g.get(layer).map(|l| l.all_entries().skip(first).map(|c| c.shape.clone()).collect()).unwrap_or_default()),
    });
    out
}

/// Box-drawing (U+2500–257F), blocks (U+2580–259F) and the Powerline
/// arrows, drawn to the cell so adjacent cells connect exactly.
fn draw_box(p: &Shapes, r: Rect, c: char, color: Color32, ppp: f32) {
    let px = 1.0 / ppp;
    let snap = |v: f32| (v * ppp).round() / ppp;
    let light = (r.width() / 8.0 * ppp).round().max(1.0) / ppp;
    let heavy = light * 2.0;
    let cx = snap(r.center().x - light / 2.0);
    let cy = snap(r.center().y - light / 2.0);
    let fill = |rr: Rect| {
        p.rect_filled(rr, 0.0, color);
    };
    let code = c as u32;
    // Lines: which of left/right/up/down are drawn, and how thick.
    let seg = |l: f32, rgt: f32, u: f32, d: f32| {
        let hx = |t: f32| snap(r.center().y - t / 2.0);
        let vx = |t: f32| snap(r.center().x - t / 2.0);
        if l > 0.0 {
            fill(Rect::from_min_max(pos2(r.min.x, hx(l)), pos2(vx(u.max(d).max(l)) + u.max(d).max(l), hx(l) + l)));
        }
        if rgt > 0.0 {
            fill(Rect::from_min_max(pos2(vx(u.max(d).max(rgt)), hx(rgt)), pos2(r.max.x, hx(rgt) + rgt)));
        }
        if u > 0.0 {
            fill(Rect::from_min_max(pos2(vx(u), r.min.y), pos2(vx(u) + u, hx(l.max(rgt).max(u)) + l.max(rgt).max(u))));
        }
        if d > 0.0 {
            fill(Rect::from_min_max(pos2(vx(d), hx(l.max(rgt).max(d))), pos2(vx(d) + d, r.max.y)));
        }
    };
    let (n, h) = (light, heavy);
    match code {
        0x2500 => seg(n, n, 0.0, 0.0),
        0x2501 => seg(h, h, 0.0, 0.0),
        0x2502 => seg(0.0, 0.0, n, n),
        0x2503 => seg(0.0, 0.0, h, h),
        0x250c => seg(0.0, n, 0.0, n),
        0x250f => seg(0.0, h, 0.0, h),
        0x2510 => seg(n, 0.0, 0.0, n),
        0x2513 => seg(h, 0.0, 0.0, h),
        0x2514 => seg(0.0, n, n, 0.0),
        0x2517 => seg(0.0, h, h, 0.0),
        0x2518 => seg(n, 0.0, n, 0.0),
        0x251b => seg(h, 0.0, h, 0.0),
        0x251c => seg(0.0, n, n, n),
        0x2523 => seg(0.0, h, h, h),
        0x2524 => seg(n, 0.0, n, n),
        0x252b => seg(h, 0.0, h, h),
        0x252c => seg(n, n, 0.0, n),
        0x2533 => seg(h, h, 0.0, h),
        0x2534 => seg(n, n, n, 0.0),
        0x253b => seg(h, h, h, 0.0),
        0x253c => seg(n, n, n, n),
        0x254b => seg(h, h, h, h),
        0x2574 => seg(n, 0.0, 0.0, 0.0),
        0x2575 => seg(0.0, 0.0, n, 0.0),
        0x2576 => seg(0.0, n, 0.0, 0.0),
        0x2577 => seg(0.0, 0.0, 0.0, n),
        // Rounded corners: straight arms meeting a quarter circle.
        0x256d..=0x2570 => {
            // Which way the horizontal and vertical arms go.
            let (hx, vy) = match code {
                0x256d => (1.0, 1.0),
                0x256e => (-1.0, 1.0),
                0x256f => (-1.0, -1.0),
                _ => (1.0, -1.0),
            };
            let c = pos2(snap(r.center().x - n / 2.0) + n / 2.0, snap(r.center().y - n / 2.0) + n / 2.0);
            let radius = (r.width() / 2.0).min(r.height() / 2.0);
            let arc_center = pos2(c.x + hx * radius, c.y + vy * radius);
            let mut pts = Vec::new();
            // Vertical arm from the cell edge to the arc.
            pts.push(pos2(c.x, if vy > 0.0 { r.max.y } else { r.min.y }));
            for k in 0..=12 {
                let t = k as f32 / 12.0 * std::f32::consts::FRAC_PI_2;
                pts.push(pos2(arc_center.x - hx * radius * t.cos(), arc_center.y - vy * radius * t.sin()));
            }
            pts.push(pos2(if hx > 0.0 { r.max.x } else { r.min.x }, c.y));
            p.add(egui::Shape::line(pts, Stroke::new(n, color)));
        }
        // Double lines. Each arm is two lines; where arms meet, a line
        // stops short (inner) or runs on to the corner (outer) depending on
        // whether another arm is on its side.
        0x2550 | 0x2551 | 0x2554 | 0x2557 | 0x255a | 0x255d | 0x2560 | 0x2563 | 0x2566 | 0x2569 | 0x256c => {
            // left, right, up, down
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
            let c = pos2(snap(r.center().x - n / 2.0), snap(r.center().y - n / 2.0));
            for side in [-1.0f32, 1.0] {
                // Horizontal lines at c.y ± g.
                let y = c.y + side * g;
                let blocked = if side < 0.0 { au } else { ad };
                let inner = if blocked { g } else { -g };
                if ar {
                    fill(Rect::from_min_max(pos2(c.x + inner, y), pos2(r.max.x, y + n)));
                }
                if al {
                    fill(Rect::from_min_max(pos2(r.min.x, y), pos2(c.x - inner + n, y + n)));
                }
                // Vertical lines at c.x ± g.
                let x = c.x + side * g;
                let blocked = if side < 0.0 { al } else { ar };
                let inner = if blocked { g } else { -g };
                if ad {
                    fill(Rect::from_min_max(pos2(x, c.y + inner), pos2(x + n, r.max.y)));
                }
                if au {
                    fill(Rect::from_min_max(pos2(x, r.min.y), pos2(x + n, c.y - inner + n)));
                }
            }
        }
        // Blocks.
        0x2580 => fill(Rect::from_min_max(r.min, pos2(r.max.x, snap(r.center().y)))),
        0x2581..=0x2588 => {
            let eighths = (code - 0x2580) as f32;
            fill(Rect::from_min_max(pos2(r.min.x, snap(r.max.y - r.height() * eighths / 8.0)), r.max));
        }
        0x2589..=0x258f => {
            let eighths = (0x2590 - code) as f32;
            fill(Rect::from_min_max(r.min, pos2(snap(r.min.x + r.width() * eighths / 8.0), r.max.y)));
        }
        0x2590 => fill(Rect::from_min_max(pos2(snap(r.center().x), r.min.y), r.max)),
        0x2591..=0x2593 => {
            let a = [64, 128, 192][(code - 0x2591) as usize];
            fill_alpha(p, r, color, a);
        }
        0x2594 => fill(Rect::from_min_max(r.min, pos2(r.max.x, snap(r.min.y + r.height() / 8.0)))),
        0x2595 => fill(Rect::from_min_max(pos2(snap(r.max.x - r.width() / 8.0), r.min.y), r.max)),
        0x2596..=0x259f => {
            // Quadrants: bit order upper-left, upper-right, lower-left, lower-right.
            let q = [0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111][(code - 0x2596) as usize];
            let (mx, my) = (snap(r.center().x), snap(r.center().y));
            let quads = [
                Rect::from_min_max(r.min, pos2(mx, my)),
                Rect::from_min_max(pos2(mx, r.min.y), pos2(r.max.x, my)),
                Rect::from_min_max(pos2(r.min.x, my), pos2(mx, r.max.y)),
                Rect::from_min_max(pos2(mx, my), r.max),
            ];
            for (i, quad) in quads.iter().enumerate() {
                if q & (0b1000 >> i) != 0 {
                    fill(*quad);
                }
            }
        }
        // Powerline separators.
        0xe0b0 | 0xe0b2 => {
            let pts = if code == 0xe0b0 {
                vec![r.left_top(), pos2(r.max.x, r.center().y), r.left_bottom()]
            } else {
                vec![r.right_top(), r.right_bottom(), pos2(r.min.x, r.center().y)]
            };
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        0xe0b1 | 0xe0b3 => {
            let pts = if code == 0xe0b1 {
                [r.left_top(), pos2(r.max.x - px, r.center().y), r.left_bottom()]
            } else {
                [r.right_top(), pos2(r.min.x + px, r.center().y), r.right_bottom()]
            };
            p.add(egui::Shape::line(pts.to_vec(), Stroke::new(light, color)));
        }
        _ => {
            // Anything else in the range: the font's glyph, centred.
            let galley = p.layout_no_wrap(c.to_string(), FontId::monospace(r.height() / 1.12 * 0.9), color);
            p.galley(pos2(r.min.x, r.min.y), galley, color);
        }
    }
    let _ = (cx, cy);
}

fn fill_alpha(p: &Shapes, r: Rect, color: Color32, a: u8) {
    p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a));
}

// ---- images and links -------------------------------------------------------

/// Paint the image slices for tagged cells, one quad per run of cells.
fn draw_images(
    ui: &Ui,
    painter: &egui::Painter,
    s: &Session,
    cells: &[(i32, usize, u32, usize)],
    textures: &mut HashMap<u32, (u64, egui::TextureHandle)>,
    at: impl Fn(i32, usize) -> Pos2,
    cell: egui::Vec2,
) {
    if cells.is_empty() {
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
        let fresh = textures.get(&p.image).is_some_and(|(g, _)| *g == img.generation);
        if !fresh {
            let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
            let tex = ui.ctx().load_texture(format!("term-img-{}", p.image), color, egui::TextureOptions::LINEAR);
            textures.insert(p.image, (img.generation, tex));
        }
        let Some((_, tex)) = textures.get(&p.image) else { continue };
        let (w, h) = (img.width as f32, img.height as f32);
        let [sx, sy, sw, sh] = p.src.map(|v| v as f32);
        let c0 = col.saturating_sub(p.start_col) as f32;
        let c1 = (last_col + 1).saturating_sub(p.start_col) as f32;
        let u0 = (sx + sw * c0 / p.cols as f32) / w;
        let u1 = (sx + sw * (c1 / p.cols as f32).min(1.0)) / w;
        let v0 = (sy + sh * row as f32 / p.rows as f32) / h;
        let v1 = (sy + sh * (row + 1) as f32 / p.rows as f32) / h;
        // Keep the aspect ratio: the image is scaled to fit its cell box
        // (cells are rarely an exact multiple of its pixel size).
        let tl = at(line, col);
        let rect = Rect::from_min_size(tl, vec2(cell.x * (c1 - c0), cell.y));
        painter.image(tex.id(), rect, Rect::from_min_max(pos2(u0, v0), pos2(u1, v1)), Color32::WHITE);
    }
    // Forget textures of images that are gone.
    textures.retain(|id, _| store.images.contains_key(id));
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
        let cells: Vec<Point> = points
            .into_iter()
            .filter(|p| term.grid()[p.line][p.column].hyperlink().is_some_and(|o| o.id() == id))
            .collect();
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

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(m: egui::Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

/// The bytes for a key, or None when it's left to the text event.
pub fn key_bytes(key: Key, m: egui::Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let mp = modifier_param(m);
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
    let tilde = |n: u8| -> Vec<u8> {
        if mp > 1 { format!("\x1b[{n};{mp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let ss3 = |letter: char| -> Vec<u8> {
        if mp > 1 { format!("\x1b[1;{mp}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() }
    };
    let esc = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    Some(match key {
        Key::ArrowUp => csi('A'),
        Key::ArrowDown => csi('B'),
        // Option+←/→ alone: a word back or forward, as macOS Terminal
        // sends it (readline, zsh and fish all know ESC b / ESC f; bash
        // leaves xterm's CSI 1;3 D unbound). A program that asked for the
        // kitty keyboard protocol (Claude Code, …) gets the protocol's
        // CSI 1;3 D and reads it as Alt+← itself.
        Key::ArrowRight if m.alt && !m.ctrl && !m.shift && !kitty => b"\x1bf".to_vec(),
        Key::ArrowLeft if m.alt && !m.ctrl && !m.shift && !kitty => b"\x1bb".to_vec(),
        Key::ArrowRight => csi('C'),
        Key::ArrowLeft => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Enter => esc(vec![b'\r']),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc(vec![b'\t']),
        Key::Backspace if m.ctrl => esc(vec![0x08]),
        Key::Backspace => esc(vec![0x7f]),
        Key::Escape => esc(vec![0x1b]),
        _ if m.ctrl => esc(vec![ctrl_byte(key)?]),
        _ => return None,
    })
}

/// Ctrl+key as a C0 control character.
fn ctrl_byte(key: Key) -> Option<u8> {
    let name = key.name();
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_lowercase() & 0x1f);
        }
    }
    Some(match key {
        Key::Space | Key::Num2 => 0,
        Key::OpenBracket | Key::Num3 => 0x1b,
        Key::Backslash | Key::Num4 => 0x1c,
        Key::CloseBracket | Key::Num5 => 0x1d,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Num7 => 0x1f,
        Key::Slash => 0x1f,
        Key::Num8 | Key::Questionmark => 0x7f,
        _ => return None,
    })
}

/// The kitty keyboard protocol's code for a key: its Unicode codepoint
/// (unshifted) or a functional-key number. None for the arrows (egui's
/// symbols for them, ⏴ and friends, aren't what they type), which keep
/// their legacy `CSI 1;mods D` form under the protocol too.
fn kitty_code(key: Key) -> Option<u32> {
    Some(match key {
        Key::Escape => 27,
        Key::Enter => 13,
        Key::Tab => 9,
        Key::Backspace => 127,
        Key::Space => 32,
        Key::Insert => 2,
        Key::Delete => 3,
        _ => {
            let sym = key.symbol_or_name();
            let mut chars = sym.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            match c {
                '−' => '-' as u32,
                c if c.is_ascii() => c.to_ascii_lowercase() as u32,
                _ => return None,
            }
        }
    })
}

/// `CSI code ; mods [: event] u`, the kitty protocol's key report.
fn csi_u(code: u32, m: egui::Modifiers, event: u8) -> Vec<u8> {
    let mp = modifier_param(m);
    match (mp, event) {
        (1, 1) => format!("\x1b[{code}u"),
        (_, 1) => format!("\x1b[{code};{mp}u"),
        _ => format!("\x1b[{code};{mp}:{event}u"),
    }
    .into_bytes()
}

/// Encode a key under the kitty keyboard protocol, or None for the
/// legacy encoding. `event` is 1 press, 2 repeat, 3 release.
fn kitty_key(key: Key, m: egui::Modifiers, mode: TermMode, event: u8) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    if event == 3 && !events {
        return None;
    }
    let event = if events { event } else { 1 };
    let code = kitty_code(key);
    let functional = matches!(key, Key::Escape | Key::Enter | Key::Tab | Key::Backspace);
    let text_key = code.is_some() && !functional && !matches!(key, Key::Insert | Key::Delete);
    if let Some(code) = code {
        if all {
            return Some(if matches!(key, Key::Insert | Key::Delete) { csi_tilde(code as u8, m, event) } else { csi_u(code, m, event) });
        }
        if functional {
            // Esc is always disambiguated; Enter, Tab and Backspace only
            // with modifiers (plain ones keep their legacy bytes).
            let plain = !m.ctrl && !m.alt && !m.shift;
            return (key == Key::Escape || !plain || event != 1).then(|| csi_u(code, m, event));
        }
        if text_key && (m.ctrl || m.alt) {
            return Some(csi_u(code, m, event));
        }
        if text_key {
            // Plain typing stays text (releases are reported when asked).
            return (event == 3).then(|| csi_u(code, m, event));
        }
    }
    // Arrows, F-keys and friends keep their legacy form (with the event
    // type when reporting them).
    if event != 1 {
        return legacy_with_event(key, m, event);
    }
    None
}

fn csi_tilde(n: u8, m: egui::Modifiers, event: u8) -> Vec<u8> {
    let mp = modifier_param(m);
    if mp == 1 && event == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{mp}:{event}~") }.into_bytes()
}

fn legacy_with_event(key: Key, m: egui::Modifiers, event: u8) -> Option<Vec<u8>> {
    let mp = modifier_param(m);
    let letter = |l: char| Some(format!("\x1b[1;{mp}:{event}{l}").into_bytes());
    match key {
        Key::ArrowUp => letter('A'),
        Key::ArrowDown => letter('B'),
        Key::ArrowRight => letter('C'),
        Key::ArrowLeft => letter('D'),
        Key::Home => letter('H'),
        Key::End => letter('F'),
        Key::PageUp => Some(csi_tilde(5, m, event)),
        Key::PageDown => Some(csi_tilde(6, m, event)),
        _ => None,
    }
}

/// A menu shortcut's chord, the app's rather than the shell's: one of
/// Terminal's in the keyboard profile (⌘key, which arrives as
/// Ctrl+Shift+key, whether or not the item is enabled just now; ⇧= arrives
/// as Plus).
fn is_shortcut(key: Key, m: egui::Modifiers) -> bool {
    m.command && olo_kit::keyboard::is_own_shortcut(crate::APP_ID, key, m, true)
}

fn keyboard(ui: &Ui, s: &Session, out: &mut Output) {
    let events = ui.input(|i| i.events.clone());
    if events.is_empty() {
        return;
    }
    let mode = *s.term.lock().mode();
    let kitty = mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL);
    let mut wrote = false;
    // A key we already reported as an escape sequence also produces a
    // text event; skip that.
    let mut skip_text = false;
    for e in events {
        match e {
            egui::Event::Key { key, pressed, repeat, modifiers, .. } => {
                // ⌘ with a menu shortcut's key is the app's (⌘C arrives
                // as Ctrl+Shift+C); any other goes to the shell as
                // Ctrl+key, so ⌘R searches history like Ctrl+R. Physical
                // Ctrl isn't `command` (the kit's `key_modifiers`), so
                // Ctrl+Shift+- is the shell's Ctrl+_ (undo), not Smaller.
                if is_shortcut(key, modifiers) && pressed {
                    continue;
                }
                if pressed && modifiers.shift && !mode.contains(TermMode::ALT_SCREEN) {
                    // Shift+PageUp/Down scroll the history, as everywhere.
                    let page = s.size.rows as i32 - 1;
                    let scroll = match key {
                        Key::PageUp => Some(Scroll::Delta(page)),
                        Key::PageDown => Some(Scroll::Delta(-page)),
                        Key::Home => Some(Scroll::Top),
                        Key::End => Some(Scroll::Bottom),
                        _ => None,
                    };
                    if let Some(sc) = scroll {
                        s.term.lock().scroll_display(sc);
                        continue;
                    }
                }
                if kitty {
                    let event = if !pressed { 3 } else if repeat { 2 } else { 1 };
                    if let Some(bytes) = kitty_key(key, modifiers, mode, event) {
                        s.write(bytes);
                        wrote |= pressed;
                        skip_text = pressed;
                        continue;
                    }
                }
                if !pressed {
                    continue;
                }
                skip_text = false;
                if let Some(bytes) = key_bytes(key, modifiers, mode) {
                    s.write(bytes);
                    wrote = true;
                }
            }
            egui::Event::Text(text) => {
                if std::mem::take(&mut skip_text) {
                    continue;
                }
                let alt = ui.input(|i| i.modifiers.alt);
                let mut b = Vec::new();
                if alt {
                    b.push(0x1b);
                }
                b.extend_from_slice(text.as_bytes());
                s.write(b);
                wrote = true;
            }
            egui::Event::Copy => out.copy = s.term.lock().selection_to_string(),
            egui::Event::Paste(text) => {
                paste(s, &text, mode);
                wrote = true;
            }
            _ => {}
        }
    }
    if wrote {
        s.typed();
    }
}

/// Send pasted text, bracketed when the program asked for that (so a
/// pasted command doesn't run until the user presses Return).
pub fn paste(s: &Session, text: &str, mode: TermMode) {
    if text.is_empty() {
        return;
    }
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A paste can't end the bracket early.
        let clean = text.replace("\x1b[201~", "");
        s.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
    } else {
        s.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
    }
}

// ---- mouse ------------------------------------------------------------------

fn cell_at(pos: Pos2, rect: Rect, m: &Metrics, size: Size, offset: usize) -> (Point, Side) {
    let x = (pos.x - rect.min.x - PAD.x).max(0.0);
    let y = (pos.y - rect.min.y - PAD.y).max(0.0);
    let col = ((x / m.cell.x) as usize).min(size.cols - 1);
    let row = ((y / m.cell.y) as usize).min(size.rows - 1);
    let side = if x - col as f32 * m.cell.x > m.cell.x / 2.0 { Side::Right } else { Side::Left };
    (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
}

/// SGR mouse report (1006).
fn report(s: &Session, button: u8, point: Point, offset: usize, pressed: bool, m: egui::Modifiers) {
    let mods = 4 * m.shift as u8 + 8 * m.alt as u8 + 16 * m.ctrl as u8;
    let line = point.line.0 + offset as i32 + 1;
    let col = point.column.0 + 1;
    let fin = if pressed { 'M' } else { 'm' };
    s.write(format!("\x1b[<{};{col};{line}{fin}", button + mods).into_bytes());
}

fn pointer(ui: &Ui, s: &mut Session, m: &Metrics, resp: &egui::Response, rect: Rect, mouse: &mut Mouse, out: &mut Output) {
    let (mode, offset) = {
        let t = s.term.lock();
        (*t.mode(), t.grid().display_offset())
    };
    let mods = ui.input(|i| i.modifiers);
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;
    let size = s.size;

    // Wheel: history, or the program's own scrolling.
    let wheel = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } if resp.hovered() || resp.contains_pointer() => Some(match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Page => delta.y * size.rows as f32,
                    egui::MouseWheelUnit::Point => delta.y / m.cell.y,
                }),
                _ => None,
            })
            .sum::<f32>()
    });
    if wheel != 0.0 {
        mouse.scroll += wheel;
        let lines = mouse.scroll.trunc() as i32;
        mouse.scroll -= lines as f32;
        if lines != 0 {
            if reporting {
                if let Some(pos) = resp.hover_pos() {
                    let (pt, _) = cell_at(pos, rect, m, size, offset);
                    for _ in 0..lines.unsigned_abs() {
                        report(s, if lines > 0 { 64 } else { 65 }, pt, offset, true, mods);
                    }
                }
            } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                let key = if lines > 0 { "A" } else { "B" };
                let seq = if mode.contains(TermMode::APP_CURSOR) { format!("\x1bO{key}") } else { format!("\x1b[{key}") };
                s.write(seq.repeat(lines.unsigned_abs() as usize).into_bytes());
            } else {
                s.term.lock().scroll_display(Scroll::Delta(lines * 3));
            }
        }
    }

    let Some(pos) = resp.interact_pointer_pos().or(resp.hover_pos()) else { return };
    let (point, side) = cell_at(pos, rect, m, size, offset);

    // ⌘-click on a link opens it, even in a program that takes the mouse.
    let pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) && resp.contains_pointer();
    if pressed
        && mods.command
        && let Some(target) = mouse.hover.as_ref().and_then(|h| h.target.clone())
    {
        out.open = Some(target);
        mouse.last_cell = Some(point);
        return;
    }

    if reporting {
        let pressed = ui.input(|i| {
            [(egui::PointerButton::Primary, 0u8), (egui::PointerButton::Middle, 1), (egui::PointerButton::Secondary, 2)]
                .into_iter()
                .find(|(b, _)| i.pointer.button_pressed(*b))
                .map(|(_, n)| n)
        });
        let released = ui.input(|i| {
            [(egui::PointerButton::Primary, 0u8), (egui::PointerButton::Middle, 1), (egui::PointerButton::Secondary, 2)]
                .into_iter()
                .find(|(b, _)| i.pointer.button_released(*b))
                .map(|(_, n)| n)
        });
        if let Some(b) = pressed
            && resp.contains_pointer()
        {
            mouse.reporting = Some(b);
            report(s, b, point, offset, true, mods);
        }
        if mouse.last_cell != Some(point) {
            if let Some(b) = mouse.reporting
                && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
            {
                report(s, 32 + b, point, offset, true, mods);
            } else if mouse.reporting.is_none() && mode.contains(TermMode::MOUSE_MOTION) && resp.contains_pointer() {
                report(s, 35, point, offset, true, mods);
            }
        }
        if let Some(b) = released
            && mouse.reporting.is_some()
        {
            mouse.reporting = None;
            report(s, b, point, offset, false, mods);
        }
        mouse.last_cell = Some(point);
        return;
    }
    mouse.last_cell = Some(point);

    // Selection: drag for characters, double-click words, triple-click
    // lines; Shift extends.
    if pressed {
        let now = std::time::Instant::now();
        let count = match mouse.last_click {
            Some((t, p, n)) if now.duration_since(t).as_millis() < 400 && p == point => n % 3 + 1,
            _ => 1,
        };
        mouse.last_click = Some((now, point, count));
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
            t.selection = Some(Selection::new(ty, point, side));
            if count > 1 {
                // Words and lines select on the click itself.
                if let Some(sel) = t.selection.as_mut() {
                    sel.update(point, side);
                }
            }
        }
        mouse.selecting = true;
    }
    if mouse.selecting && ui.input(|i| i.pointer.primary_down()) {
        let mut t = s.term.lock();
        if let Some(sel) = t.selection.as_mut() {
            sel.update(point, side);
        }
        // Drag past the edge to scroll, faster the further out.
        let past = if pos.y < rect.min.y { pos.y - rect.min.y } else { (pos.y - rect.max.y).max(0.0) };
        if past == 0.0 {
            mouse.edge = 0.0;
        }
        mouse.edge += olo_kit::widgets::ScrollView::edge_scroll(ui.ctx(), egui::vec2(0.0, past)).y / m.cell.y;
        let lines = mouse.edge.trunc() as i32;
        mouse.edge -= lines as f32;
        if lines != 0 {
            t.scroll_display(Scroll::Delta(-lines));
        }
    }
    if mouse.selecting && ui.input(|i| i.pointer.primary_released()) {
        mouse.selecting = false;
        let mut t = s.term.lock();
        // A plain click (no drag) clears the selection.
        if t.selection.as_ref().is_some_and(|sel| sel.is_empty()) {
            t.selection = None;
        }
    }
    if resp.clicked_by(egui::PointerButton::Middle) {
        out.paste = true;
    }
    if resp.secondary_clicked() {
        out.context_menu = Some(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_arrows_move_by_word() {
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        assert_eq!(key_bytes(Key::ArrowLeft, alt, TermMode::empty()).unwrap(), b"\x1bb");
        assert_eq!(key_bytes(Key::ArrowRight, alt, TermMode::empty()).unwrap(), b"\x1bf");
        // With others held, xterm's modified arrows.
        let both = egui::Modifiers { alt: true, shift: true, ..Default::default() };
        assert_eq!(key_bytes(Key::ArrowLeft, both, TermMode::empty()).unwrap(), b"\x1b[1;4D");
        assert_eq!(key_bytes(Key::ArrowLeft, egui::Modifiers::NONE, TermMode::empty()).unwrap(), b"\x1b[D");
        // Under the kitty keyboard protocol (Claude Code turns it on), the
        // protocol's own form, which it reads as Meta+← (a word back), as
        // the keyboard sends it: the protocol's encoding first.
        let sent = |key, m, mode| kitty_key(key, m, mode, 1).or_else(|| key_bytes(key, m, mode)).unwrap();
        for kitty in [TermMode::DISAMBIGUATE_ESC_CODES, TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC] {
            assert_eq!(sent(Key::ArrowLeft, alt, kitty), b"\x1b[1;3D");
            assert_eq!(sent(Key::ArrowRight, alt, kitty), b"\x1b[1;3C");
            let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
            assert_eq!(sent(Key::ArrowLeft, ctrl, kitty), b"\x1b[1;5D");
            // Text keys are still the protocol's CSI u.
            assert_eq!(sent(Key::B, alt, kitty), b"\x1b[98;3u");
            assert_eq!(sent(Key::Minus, ctrl, kitty), b"\x1b[45;5u");
        }
        assert_eq!(sent(Key::ArrowLeft, egui::Modifiers::NONE, TermMode::REPORT_ALL_KEYS_AS_ESC), b"\x1b[D");
    }

    #[test]
    fn ctrl_underscore_is_the_shells_and_command_minus_is_smaller() {
        // ⌘- arrives as Ctrl+Shift+- with `command`; a physical
        // Ctrl+Shift+- (Ctrl+_, readline's undo) without it.
        let cmd = egui::Modifiers { ctrl: true, shift: true, command: true, ..Default::default() };
        let ctrl = egui::Modifiers { ctrl: true, shift: true, ..Default::default() };
        assert!(is_shortcut(Key::Minus, cmd));
        assert!(!is_shortcut(Key::Minus, ctrl));
        assert_eq!(key_bytes(Key::Minus, ctrl, TermMode::empty()).unwrap(), [0x1f]);
        // Ctrl+^ and Ctrl+@ too (Ctrl+Shift+6, Ctrl+Shift+2).
        assert_eq!(key_bytes(Key::Num6, ctrl, TermMode::empty()).unwrap(), [0x1e]);
        assert_eq!(key_bytes(Key::Num2, ctrl, TermMode::empty()).unwrap(), [0]);
        // ⌘R isn't a menu's: the shell's Ctrl+R.
        assert!(!is_shortcut(Key::R, cmd));
    }
}
