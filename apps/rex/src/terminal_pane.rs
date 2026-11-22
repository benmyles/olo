//! 120 Hz GPU terminal pane renderer over `olo_rex_vt::GhosttyTerminal` + `RenderState`.
//!
//! Performance & visual architecture (matching Olo's strict fast-and-light + pixel-perfect rules):
//! - Two-phase `RenderState::begin_update`/`end_update` keeps terminal lock hold tiny.
//! - `RowCache` keyed by `RowKey` + Ghostty's stable `RowId([u64; 2])` + `dirty` flag skips
//!   unchanged rows completely, painting cached `GlyphRun`s straight from the atlas inside
//!   a single `Painter::paint_layer`.
//! - Fast-path printable ASCII glyph lookup avoids HarfBuzz shaping on normal monospace runs.
//! - Crisp device-pixel vector drawing for box-drawing (U+2500..259F), Powerline (E0B0..E0B3),
//!   and all 5 underline styles (`Single`, `Double`, `Curly`, `Dotted`, `Dashed`), strikethrough,
//!   and overline.
//! - Kitty graphics `RenderImage` texture cache (RGBA→BGRA) across all 3 Z-layers
//!   (`BelowBackground`, `BelowText`, `AboveText`) with animation tick support.
//! - Sub-row `Elastic` smooth scrolling reporting `g.note_scroll([0.0, seen, 0.0, max])`.
//! - Search match highlights, Copy Mode cursor & visual overlay, Shine Animation on Copy,
//!   and optional GPU Cursor Trail (0 CPU / 0 frames when Off or idle).

use std::{
    cell::{Cell as StdCell, RefCell},
    sync::Arc,
};

use olo_kit::{Bounds, ContentMask, Font, FontId, FontStyle, FontWeight, GlyphId, GlyphRun, Hsla, Painter, PathBuilder, Pixels, Point, RenderImage, TextRun, point, px, size, style};
use olo_rex_vt::{CellWide, CursorInfo, CursorStyle as VtCursorStyle, GhosttyTerminal, ImageLayer, ImagePlacement, RenderRow, Rgb, RowId, TerminalTheme, UnderlineStyle, ViewportMatch};

use crate::{
    copy_mode::CopyMode,
    links::DetectedLink,
    settings::{self, Settings},
    themes::{self, hsla},
};

/// Cell and font metrics snapped to device pixels for the active display scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub font_family: Option<String>,
    pub size: f32,
    pub line_height: f32,
    pub char_spacing: f32,
    pub cell: (f32, f32),
    pub advance: f32,
    pub baseline: f32,
    pub row: f32,
    pub scale: f32,
}

impl Metrics {
    pub fn from_settings(scale: f32, settings: &Settings) -> Self {
        // (Olo's fixed-width face, its own line height and spacing: the size is the one choice.)
        Self::new(scale, None, settings.font_size.clamp(settings::TEXT_SIZES.0, settings::TEXT_SIZES.1), 1.0, 0.0)
    }

    pub fn new(scale: f32, font_family: Option<String>, size: f32, line_height: f32, char_spacing: f32) -> Self {
        let ts = olo_kit::text_system();
        let base_font = resolve_mono_font(font_family.as_deref(), FontWeight::NORMAL, false);
        let id = ts.resolve_font(&base_font);
        let fs = px(size);
        let advance = ts.advance(id, fs, 'M').map_or(size * 0.6, |a| f32::from(a.width));
        let ascent = f32::from(ts.ascent(id, fs));
        let descent = f32::from(ts.descent(id, fs)).abs();
        let raw_row = (ascent + descent).max(size);
        let row = raw_row * line_height;
        let snap = |v: f32| (v * scale).ceil().max(1.0) / scale;
        let cell = (snap((advance + char_spacing).max(4.0)), snap(row));
        let baseline = (((cell.1 - raw_row) / 2.0 + ascent) * scale).round() / scale;
        Self { font_family, size, line_height, char_spacing, cell, advance, baseline, row: raw_row, scale }
    }

    pub fn device_cell(&self) -> (u32, u32) {
        ((self.cell.0 * self.scale).round().max(1.0) as u32, (self.cell.1 * self.scale).round().max(1.0) as u32)
    }

    pub fn grid_size(&self, w: f32, h: f32, pad: (f32, f32)) -> (u16, u16) {
        let cols = (((w - 2.0 * pad.0) / self.cell.0).floor() as i32).clamp(2, 500) as u16;
        let rows = (((h - 2.0 * pad.1) / self.cell.1).floor() as i32).clamp(1, 300) as u16;
        (cols, rows)
    }

    pub fn cell_at(&self, rect: Bounds<Pixels>, pad: (f32, f32), pos: Point<Pixels>, shift: f32) -> (u16, u16) {
        let ox = f32::from(rect.left()) + pad.0;
        let oy = f32::from(rect.top()) + pad.1 + shift;
        let col = ((f32::from(pos.x) - ox) / self.cell.0).floor().max(0.0) as u16;
        let row = ((f32::from(pos.y) - oy) / self.cell.1).floor().max(0.0) as u16;
        (col, row)
    }
}

fn resolve_mono_font(family: Option<&str>, weight: FontWeight, italic: bool) -> Font {
    let mut f = style::mono_font(weight);
    if let Some(name) = family.filter(|s| !s.trim().is_empty()) {
        f.family = name.trim().to_owned().into();
    }
    if italic {
        f.style = FontStyle::Italic;
    }
    f
}

/// Sub-row elastic scrollback state.
#[derive(Default)]
struct SmoothScroll {
    elastic: olo_kit::Elastic,
    /// Pixels scrolled up from the live bottom (`0.0` = bottom).
    pos: f32,
    /// Last whole-row display offset applied to Ghostty.
    rows: u64,
}

impl SmoothScroll {
    fn sync(&mut self, display_from_bottom: u64, history: u64, view_h: f32, ch: f32) -> (Point<Pixels>, f32) {
        if display_from_bottom != self.rows {
            self.pos = display_from_bottom as f32 * ch;
            self.rows = display_from_bottom;
        }
        let max = history as f32 * ch;
        self.pos = self.pos.clamp(0.0, max);
        self.elastic.set_extent(size(px(0.0), px(max)), size(px(1.0), px(view_h.max(1.0))));
        (point(px(0.0), px(self.pos - max)), max)
    }

    pub fn scroll_by(&mut self, delta_px: f32, history: u64, view_h: f32, ch: f32, term: &mut GhosttyTerminal) {
        let max = history as f32 * ch;
        self.pos = (self.pos + delta_px).clamp(0.0, max);
        let target_from_bottom = (self.pos / ch).floor().max(0.0) as u64;
        if target_from_bottom != self.rows {
            let target_offset = history.saturating_sub(target_from_bottom);
            let _ = term.scroll_to_row(target_offset as usize);
            self.rows = target_from_bottom;
        }
        self.elastic.set_extent(size(px(0.0), px(max)), size(px(1.0), px(view_h.max(1.0))));
    }
}

/// Per-pane visual caches kept between frames on the UI thread (not shared across threads).
#[derive(Default)]
pub struct PaneVisualState {
    rows: RefCell<RowCache>,

    pub bounds: StdCell<Bounds<Pixels>>,
    pub shift: StdCell<f32>,
    pub drag_anchor: StdCell<Option<(u16, u16)>>,
    smooth: RefCell<SmoothScroll>,
}

impl PaneVisualState {
    pub fn scroll_smooth(&self, delta_px: f32, ch: f32, term: &mut GhosttyTerminal) {
        let sb = term.scrollbar();
        let history = sb.history_rows();
        let view_h = sb.len as f32 * ch;
        let mut sm = self.smooth.borrow_mut();
        sm.sync(sb.display_offset_from_bottom(), history, view_h, ch);
        sm.scroll_by(delta_px, history, view_h, ch, term);
    }
}

#[derive(Default)]
struct RowCache {
    key: Option<RowKey>,
    owner: u64,
    revision: u64,
    by_id: olo_rex_vt::RowMap<CachedRow>,
    order: Vec<Option<RowId>>,
    ascii: Vec<(FontStyleKey, FontId, Box<[Option<Option<GlyphId>>; 95]>)>,
}

#[derive(Clone, PartialEq)]
struct RowKey {
    metrics: Metrics,
    default_fg: Rgb,
    default_bg: Rgb,
    selection_bg: Rgb,
    selection_fg: Option<Rgb>,
    min_contrast: f32,
    bold_is_bright: bool,
    palette_ansi: [Rgb; 16],
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FontStyleKey {
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct GlyphStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
}

struct CachedRow {
    revision: u64,
    bgs: Vec<(f32, f32, Hsla)>,
    glyphs: Vec<Glyph>,
    emoji: bool,
    run: RefCell<GlyphRun>,
    shapes: Vec<Shape>,
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

#[derive(Clone)]
enum Shape {
    Rect(f32, f32, f32, f32, Hsla),
    Line(Vec<(f32, f32)>, f32, Hsla),
    Poly(Vec<(f32, f32)>, Hsla),
}

struct Run {
    text: String,
    style: GlyphStyle,
    starts: Vec<(usize, usize)>,
    next: usize,
}

/// All inputs required to paint one Rex terminal pane.
pub struct PanePaintArgs<'a> {
    pub term: &'a mut GhosttyTerminal,
    pub frame: &'a crate::frame::Frame,
    pub theme: &'a TerminalTheme,
    pub metrics: &'a Metrics,
    pub focused: bool,
    pub window_active: bool,
    pub copy_mode: Option<&'a CopyMode>,
    pub search_matches: &'a [ViewportMatch],
    pub hover_link: Option<&'a DetectedLink>,
    pub preedit: Option<&'a str>,
}

/// Paint one terminal pane into `rect` and return cursor bounds (in window pixels) for IME positioning.
pub fn paint_pane(g: &mut Painter, rect: Bounds<Pixels>, args: PanePaintArgs<'_>, visual: &PaneVisualState) -> Option<Bounds<Pixels>> {
    let PanePaintArgs { term, frame, theme, metrics: m, focused, window_active, copy_mode, search_matches, hover_link, preedit } = args;

    visual.bounds.set(rect);
    olo_kit::review::own_grid(g, rect);

    let pad = settings::PADDING;
    let (cw, ch) = m.cell;
    let line_px = (1.0 / m.scale).max(0.5);

    let Some((colors, cursor)) = &frame.header else {
        g.paint_quad(olo_kit::fill(rect, hsla(theme.background)));
        return None;
    };
    let rows = frame.order.len();

    // What the program in the pane says its colours are, the padding
    // around the text included (the palette's own where it says none).
    let default_bg = colors.background;
    let default_fg = colors.foreground;
    let bg_hsla = hsla(default_bg);
    g.paint_quad(olo_kit::fill(rect, bg_hsla));

    // Sub-row elastic scrollback offset.
    let sb = term.scrollbar();
    let display_from_bottom = sb.display_offset_from_bottom();
    let history = sb.history_rows();
    let (slide, max_scroll, seen_scroll, moving) = {
        let mut sm = visual.smooth.borrow_mut();
        let (at, max) = sm.sync(display_from_bottom, history, rows as f32 * ch, ch);
        let (shown, moving) = sm.elastic.offset(at);
        let slide = sm.pos - display_from_bottom as f32 * ch + f32::from(shown.y - at.y);
        ((slide * m.scale).round() / m.scale, max, -f32::from(shown.y), moving)
    };
    g.note_scroll([0.0, seen_scroll, 0.0, max_scroll]);
    if moving {
        g.paint_again_within(0.0);
    }
    visual.shift.set(slide);

    let origin = (f32::from(rect.left()) + pad.0, f32::from(rect.top()) + pad.1 + slide);

    // Selection colors.
    let selection_bg = theme.selection_bg.unwrap_or(Rgb::new(56, 114, 212));
    let selection_fg = theme.selection_fg;
    let mut ansi16 = [Rgb::new(0, 0, 0); 16];
    for i in 0..16 {
        ansi16[i] = colors.palette[i];
    }

    let key = RowKey { metrics: m.clone(), default_fg, default_bg, selection_bg, selection_fg, min_contrast: settings::MINIMUM_CONTRAST, bold_is_bright: false, palette_ansi: ansi16 };

    // Update RowCache only for rows that changed (or full dirty / key change).
    {
        let mut cache = visual.rows.borrow_mut();
        let rebuild = cache.owner != frame.owner || cache.key.as_ref() != Some(&key);
        if rebuild || cache.revision != frame.revision {
            if rebuild {
                cache.key = Some(key);
                cache.owner = frame.owner;
                cache.by_id.clear();
                cache.ascii.clear();
            }
            cache.order.clear();
            for id in &frame.order {
                let source = &frame.rows[id];
                let (stamp, r) = (source.revision, &source.row);
                cache.order.push(Some(*id));
                if !cache.by_id.get(id).is_some_and(|cached| cached.revision == stamp) {
                    let mut built = build_cached_row(g, &mut cache.ascii, r, m, default_fg, default_bg, selection_bg, selection_fg, &ansi16, settings::MINIMUM_CONTRAST, false);
                    built.revision = stamp;
                    cache.by_id.insert(*id, built);
                }
            }
            cache.by_id.retain(|id, _| frame.rows.contains_key(id));
            cache.revision = frame.revision;
        }
    }

    // Paint inside clip mask of the pane rect.
    g.with_content_mask(Some(ContentMask { bounds: rect }), |g| {
        // 1. Kitty graphics BelowBackground.
        paint_kitty_layer(g, frame, ImageLayer::BelowBackground, origin, (cw, ch), m.scale);

        // 2. All cached rows in one `paint_layer` (backgrounds first, then shapes + glyphs).
        {
            let cache = visual.rows.borrow();
            g.paint_layer(rect, |g| {
                for (r_idx, maybe_id) in cache.order.iter().enumerate() {
                    let Some(id) = maybe_id else { continue };
                    let Some(row) = cache.by_id.get(id) else { continue };
                    let y = origin.1 + r_idx as f32 * ch;
                    for &(x, w, color) in &row.bgs {
                        g.paint_quad(olo_kit::fill(Bounds::new(point(px(origin.0 + x), px(y)), size(px(w), px(ch))), color));
                    }
                }
                // Kitty graphics BelowText (above cell backgrounds, under glyphs).
                paint_kitty_layer(g, frame, ImageLayer::BelowText, origin, (cw, ch), m.scale);

                for (r_idx, maybe_id) in cache.order.iter().enumerate() {
                    let Some(id) = maybe_id else { continue };
                    let Some(row) = cache.by_id.get(id) else { continue };
                    let y = origin.1 + r_idx as f32 * ch;
                    paint_shapes(g, &row.shapes, (origin.0, y));
                    let base = point(px(origin.0), px(y + m.baseline));
                    g.paint_glyph_run(&mut row.run.borrow_mut(), base, 0, |glyph| {
                        for it in row.glyphs.iter().filter(|it| !it.emoji) {
                            glyph(point(px(it.x), px(0.0)), it.font, it.id, px(it.size), it.color);
                        }
                    });
                    if row.emoji {
                        for it in row.glyphs.iter().filter(|it| it.emoji) {
                            let _ = g.paint_emoji(point(base.x + px(it.x), base.y), it.font, it.id, px(it.size));
                        }
                    }
                }
            });
        }

        // 3. Kitty graphics AboveText.
        paint_kitty_layer(g, frame, ImageLayer::AboveText, origin, (cw, ch), m.scale);

        // Schedule Kitty animation tick if any animated images are active.
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        if let Some(delay_ms) = term.kitty_animation_tick(now_ms) {
            g.paint_again_within(delay_ms as f64 / 1000.0);
        }

        // 4. Search match highlights.
        if !search_matches.is_empty() {
            let match_bg = style::c([255, 210, 64, 105]);
            let selected_bg = style::c([255, 149, 0, 170]);
            for sm in search_matches {
                let col = if sm.selected { selected_bg } else { match_bg };
                for row_y in sm.start_row..=sm.end_row {
                    let c0 = if row_y == sm.start_row { sm.start_col } else { 0 };
                    let c1 = if row_y == sm.end_row { sm.end_col } else { term.size().0.saturating_sub(1) };
                    let x = origin.0 + c0 as f32 * cw;
                    let y = origin.1 + row_y as f32 * ch;
                    let w = (c1.saturating_sub(c0) + 1) as f32 * cw;
                    g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y)), size(px(w), px(ch))), col));
                }
            }
        }

        // 5. Hovered link underline.
        let cursor_rgb = colors.cursor.or(theme.cursor).unwrap_or(default_fg);
        let cursor_color = hsla(cursor_rgb);
        if let Some(link) = hover_link {
            let y = origin.1 + link.row as f32 * ch + m.baseline + (m.row * 0.12).max(1.0);
            let x = origin.0 + link.start_col as f32 * cw;
            let w = (link.end_col.saturating_sub(link.start_col) + 1) as f32 * cw;
            g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y)), size(px(w), px((2.0 * line_px).max(1.0)))), cursor_color));
        }

        // 7. IME Preedit composition text at cursor.
        let mut preedit_advance = 0.0f32;
        if let Some(text) = preedit.filter(|t| !t.is_empty())
            && let Some((cx, cy)) = cursor.viewport_pos
        {
            let x = origin.0 + cx as f32 * cw;
            let y = origin.1 + cy as f32 * ch;
            let font = resolve_mono_font(m.font_family.as_deref(), FontWeight::NORMAL, false);
            let run = TextRun { len: text.len(), font, color: hsla(default_fg), background_color: None, underline: None, strikethrough: None, size: None };
            let shaped = g.text_system().shape_line(text.to_owned().into(), px(m.size), &[run], Some(px(cw)));
            let w = f32::from(shaped.width).max(cw);
            g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y)), size(px(w), px(ch))), bg_hsla));
            let h = shaped.ascent + shaped.descent;
            let _ = shaped.paint(point(px(x), px(y + m.baseline) - shaped.ascent), h, g);
            g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y + ch - 2.0 * line_px)), size(px(w), px((2.0 * line_px).max(1.0)))), hsla(default_fg)));
            preedit_advance = w;
        }

        // 8. Cursor (Copy Mode cursor takes precedence when active; else live terminal cursor).
        if let Some(cm) = copy_mode {
            let x = origin.0 + cm.col as f32 * cw;
            let y = origin.1 + cm.row as f32 * ch;
            let cm_color = style::c([255, 196, 46, 220]);
            g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y)), size(px(cw), px(ch))), cm_color));
        } else {
            paint_cursor(g, &cursor, origin, (cw, ch), preedit_advance, cursor_color, line_px, focused && window_active);
        }
    });

    // Compute IME cursor rect.
    cursor.viewport_pos.map(|(cx, cy)| Bounds::new(point(px(origin.0 + cx as f32 * cw), px(origin.1 + cy as f32 * ch)), size(px(cw), px(ch))))
}

#[allow(clippy::too_many_arguments)]
fn paint_cursor(g: &mut Painter, cursor: &CursorInfo, origin: (f32, f32), (cw, ch): (f32, f32), preedit_advance: f32, cursor_color: Hsla, line_px: f32, focused: bool) {
    if !cursor.visible {
        return;
    }
    let Some((col, row)) = cursor.viewport_pos else {
        return;
    };
    let x = origin.0 + col as f32 * cw + preedit_advance;
    let y = origin.1 + row as f32 * ch;
    let w = if cursor.wide_tail { cw * 2.0 } else { cw };

    // (Steady, as Terminal's: a blinking cursor is two frames a second
    // for as long as nothing happens. It blinked where the program asked,
    // and could leave a trail.)

    // In a pane without the keyboard: an outline, as Terminal's, so the
    // pane shows where its cursor is and that it lives. (It was hidden:
    // a shell waiting at its prompt looked hung.)
    if !focused {
        let t = line_px.max(0.5);
        g.paint_quad(olo_kit::outline(Bounds::new(point(px(x + t / 2.0), px(y + t / 2.0)), size(px(w - t), px(ch - t))), cursor_color, olo_kit::BorderStyle::Solid).border_widths(px(2.0 * t)));
        return;
    }

    let shape = cursor.style;

    let r = Bounds::new(point(px(x), px(y)), size(px(w), px(ch)));
    if shape == VtCursorStyle::BlockHollow {
        let t = line_px.max(0.5);
        g.paint_quad(
            olo_kit::outline(Bounds::new(point(px(x + t * 0.5), px(y + t * 0.5)), size(px(w - t), px(ch - t))), cursor_color.opacity(0.65), olo_kit::BorderStyle::Solid)
                .border_widths(px((2.0 * t).max(1.0))),
        );
        return;
    }

    match shape {
        VtCursorStyle::Bar => {
            let bw = (2.0 * line_px).max(1.5);
            g.paint_quad(olo_kit::fill(Bounds::new(r.origin, size(px(bw), px(ch))), cursor_color));
        }
        VtCursorStyle::Underline => {
            let uh = (2.5 * line_px).max(2.0);
            g.paint_quad(olo_kit::fill(Bounds::new(point(px(x), px(y + ch - uh)), size(px(w), px(uh))), cursor_color));
        }
        VtCursorStyle::Block | VtCursorStyle::BlockHollow => {
            // Translucent cursor tint when password input lock icon isn't hiding it,
            // crisp solid block otherwise.
            let fill = if cursor.password_input { style::c([235, 168, 52, 210]) } else { cursor_color.opacity(0.82) };
            g.paint_quad(olo_kit::fill(r, fill));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_cached_row(
    g: &Painter,
    ascii_table: &mut Vec<(FontStyleKey, FontId, Box<[Option<Option<GlyphId>>; 95]>)>,
    row: &RenderRow,
    m: &Metrics,
    default_fg: Rgb,
    default_bg: Rgb,
    selection_bg: Rgb,
    selection_fg: Option<Rgb>,
    ansi16: &[Rgb; 16],
    min_contrast: f32,
    bold_is_bright: bool,
) -> CachedRow {
    let (cw, ch) = m.cell;
    let line_px = (1.0 / m.scale).max(0.5);
    let mut bgs: Vec<(f32, f32, Hsla)> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut shapes: Vec<Shape> = Vec::new();
    let mut emoji_clusters: Vec<(usize, String, GlyphStyle, usize)> = Vec::new();
    let mut last_bg: Option<(usize, Rgb)> = None;
    let cols = row.cells.len();
    let mut used = vec![false; cols + 1];
    let mut legible_last: Option<(Rgb, Rgb, Rgb)> = None;
    let mut last_ink = None;

    for (col, cell) in row.cells.iter().enumerate() {
        if matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead) {
            continue;
        }
        let wide = cell.wide == CellWide::Wide;
        let span = if wide { 2 } else { 1 };

        let mut fg = cell.fg.unwrap_or(default_fg);
        let mut bg = cell.bg.unwrap_or(default_bg);

        if bold_is_bright
            && cell.style.bold
            && let Some(idx) = cell.style.fg_palette_index
            && idx < 8
        {
            fg = ansi16[(idx + 8) as usize];
        }
        if cell.style.faint {
            fg = themes::mix(fg, bg, 0.55);
        }
        if cell.style.inverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = row.selection.is_some_and(|(a, b)| (a as usize) <= col && col <= (b as usize));
        if selected {
            bg = selection_bg;
            if let Some(sfg) = selection_fg {
                fg = sfg;
            }
        }

        let primary_char = char::from_u32(cell.codepoint).unwrap_or(' ');
        if min_contrast > 1.0 && !themes::is_graphic_char(primary_char) {
            fg = match legible_last {
                Some((lf, lb, out)) if lf == fg && lb == bg => out,
                _ => {
                    let out = themes::legible(fg, bg, min_contrast);
                    legible_last = Some((fg, bg, out));
                    out
                }
            };
        }

        if bg != default_bg {
            match (bgs.last_mut(), last_bg) {
                (Some(prev), Some((end_col, prev_rgb))) if end_col == col && prev_rgb == bg => {
                    prev.1 += span as f32 * cw;
                }
                _ => bgs.push((col as f32 * cw, span as f32 * cw, hsla(bg))),
            }
            last_bg = Some((col + span, bg));
        }

        if cell.style.invisible {
            continue;
        }

        let ink = match last_ink {
            Some((rgb, ink)) if rgb == fg => ink,
            _ => {
                let ink = hsla(fg);
                last_ink = Some((fg, ink));
                ink
            }
        };
        let st = GlyphStyle { fg: ink, bold: cell.style.bold, italic: cell.style.italic };
        let x = col as f32 * cw;
        let w = cw * span as f32;

        // Underlines (all 5 styles!).
        if cell.style.underline != UnderlineStyle::None {
            let uc = cell.style.underline_color.map(hsla).unwrap_or(st.fg);
            let uy = m.baseline + (m.row * 0.12).max(1.0);
            let t = (m.size / 12.0).round().max(1.0) * line_px;
            match cell.style.underline {
                UnderlineStyle::None => {}
                UnderlineStyle::Single => shapes.push(Shape::Rect(x, uy, w, t, uc)),
                UnderlineStyle::Double => {
                    shapes.push(Shape::Rect(x, uy, w, t, uc));
                    shapes.push(Shape::Rect(x, uy + 2.0 * t, w, t, uc));
                }
                UnderlineStyle::Curly => {
                    let amp = (t * 1.35).max(1.0);
                    let steps = 8usize;
                    let mut pts = Vec::with_capacity(steps + 1);
                    for i in 0..=steps {
                        let frac = i as f32 / steps as f32;
                        let px_ = x + w * frac;
                        let py_ = uy + (frac * std::f32::consts::TAU).sin() * amp;
                        pts.push((px_, py_));
                    }
                    shapes.push(Shape::Line(pts, t, uc));
                }
                UnderlineStyle::Dotted => {
                    let dot = (t * 1.4).max(1.0);
                    let mut dx = x;
                    while dx < x + w {
                        shapes.push(Shape::Rect(dx, uy, dot.min(x + w - dx), t, uc));
                        dx += dot * 2.0;
                    }
                }
                UnderlineStyle::Dashed => {
                    let dash = (w * 0.36).max(2.0);
                    let gap = (w * 0.16).max(1.0);
                    let mut dx = x;
                    while dx < x + w {
                        shapes.push(Shape::Rect(dx, uy, dash.min(x + w - dx), t, uc));
                        dx += dash + gap;
                    }
                }
            }
        }

        if cell.style.strikethrough {
            let sy = m.baseline - m.row * 0.24;
            shapes.push(Shape::Rect(x, sy, w, line_px, st.fg));
        }
        if cell.style.overline {
            shapes.push(Shape::Rect(x, line_px, w, line_px, st.fg));
        }

        if cell.codepoint == 0 || (primary_char == ' ' && cell.grapheme.is_empty()) {
            continue;
        }
        used[col] = true;

        if cell.grapheme.is_empty() && is_box_char(primary_char) {
            draw_box(&mut shapes, (x, 0.0, cw, ch), primary_char, st.fg, m.scale);
            continue;
        }

        let cluster = if cell.grapheme.is_empty() { primary_char.to_string() } else { cell.grapheme.clone() };

        if wants_color_emoji(&cluster, wide) {
            emoji_clusters.push((col, cluster, st, span));
            continue;
        }

        match runs.last_mut() {
            Some(r) if r.style == st && r.next == col => {
                r.starts.push((r.text.len(), col));
                r.text.push_str(&cluster);
                r.next = col + span;
            }
            _ => runs.push(Run { text: cluster, style: st, starts: vec![(0, col)], next: col + span }),
        }
    }

    let mut glyphs = Vec::new();
    for Run { text, style: st, starts, .. } in &runs {
        if text.bytes().all(|b| (0x21..0x7f).contains(&b))
            && let Some((font, ids)) = ascii_glyphs(g, ascii_table, m.font_family.as_deref(), *st, text)
        {
            for (&(_, col), id) in starts.iter().zip(ids) {
                glyphs.push(Glyph { x: col as f32 * cw, font, id, size: m.size, color: st.fg, emoji: false });
            }
            continue;
        }
        let font = resolve_mono_font(m.font_family.as_deref(), if st.bold { FontWeight::BOLD } else { FontWeight::NORMAL }, st.italic);
        let run = TextRun { len: text.len(), font, color: st.fg, background_color: None, underline: None, strikethrough: None, size: None };
        let layout = g.text_system().layout_line_now(text, px(m.size), &[run]);
        for shaped in &layout.runs {
            let mut cluster_base: Option<(usize, f32)> = None;
            for gl in &shaped.glyphs {
                let k = starts.partition_point(|&(byte, _)| byte <= gl.index).saturating_sub(1);
                let (byte, col) = starts[k];
                let gx = f32::from(gl.position.x);
                let base = match cluster_base {
                    Some((b, x0)) if b == byte => x0,
                    _ => {
                        cluster_base = Some((byte, gx));
                        gx
                    }
                };
                let wide_cell = row.cells.get(col).is_some_and(|c| c.wide == CellWide::Wide);
                let adv = if wide_cell { 2.0 * cw } else { cw };
                let next_pos = shaped.glyphs.iter().find(|n| n.index > gl.index).map_or(layout.width, |n| n.position.x);
                let glyph_w = (f32::from(next_pos) - gx).max(0.0);
                let centre = if (glyph_w - m.advance).abs() > 0.5 { ((adv - glyph_w) / 2.0 * m.scale).round() / m.scale } else { 0.0 };
                glyphs.push(Glyph { x: col as f32 * cw + (gx - base) + centre, font: shaped.font_id, id: gl.id, size: f32::from(shaped.font_size), color: st.fg, emoji: gl.is_emoji });
            }
        }
    }

    for (col, text, st, span) in emoji_clusters {
        let eff_span = if span == 1 && col + 1 < cols && !used[col + 1] { 2 } else { span };
        let font = resolve_mono_font(m.font_family.as_deref(), if st.bold { FontWeight::BOLD } else { FontWeight::NORMAL }, st.italic);
        let run = TextRun { len: text.len(), font, color: st.fg, background_color: None, underline: None, strikethrough: None, size: None };
        let layout = g.text_system().layout_line_now(&text, px(m.size), &[run]);
        let w = f32::from(layout.width);
        let x0 = col as f32 * cw + ((eff_span.min(2) as f32 * cw - w) / 2.0 * m.scale).round() / m.scale;
        for shaped in &layout.runs {
            for gl in &shaped.glyphs {
                glyphs.push(Glyph { x: x0 + f32::from(gl.position.x), font: shaped.font_id, id: gl.id, size: f32::from(shaped.font_size), color: st.fg, emoji: gl.is_emoji });
            }
        }
    }

    let emoji = glyphs.iter().any(|gl| gl.emoji);
    CachedRow { revision: 0, bgs, glyphs, emoji, run: RefCell::default(), shapes }
}

fn ascii_glyphs(g: &Painter, table: &mut Vec<(FontStyleKey, FontId, Box<[Option<Option<GlyphId>>; 95]>)>, family: Option<&str>, st: GlyphStyle, text: &str) -> Option<(FontId, Vec<GlyphId>)> {
    let ts = g.text_system();
    let fkey = FontStyleKey { bold: st.bold, italic: st.italic };
    let idx = match table.iter().position(|(k, _, _)| *k == fkey) {
        Some(i) => i,
        None => {
            let font = resolve_mono_font(family, if st.bold { FontWeight::BOLD } else { FontWeight::NORMAL }, st.italic);
            table.push((fkey, ts.resolve_font(&font), Box::new([None; 95])));
            table.len() - 1
        }
    };
    let (_, font, slots) = &mut table[idx];
    let mut ids = Vec::with_capacity(text.len());
    for b in text.bytes() {
        let slot = &mut slots[usize::from(b - 0x20)];
        let id = *slot.get_or_insert_with(|| ts.glyph_for_char(*font, char::from(b)));
        ids.push(id?);
    }
    Some((*font, ids))
}

fn image_rect(p: &ImagePlacement, origin: (f32, f32), cell: (f32, f32), scale: f32) -> Bounds<Pixels> {
    let x = origin.0 + p.viewport_col as f32 * cell.0 + p.cell_offset_x as f32 / scale;
    let y = origin.1 + p.viewport_row as f32 * cell.1 + p.cell_offset_y as f32 / scale;
    Bounds::new(point(px(x), px(y)), size(px(p.pixel_width as f32 / scale), px(p.pixel_height as f32 / scale)))
}

// Position the full texture so exactly the source rectangle is inside the clip.
// Nearest sampling on enlargement keeps a neighbour outside the crop from bleeding in.
fn image_texture_bounds(p: &ImagePlacement, dest: Bounds<Pixels>, w: f32, h: f32) -> Option<Bounds<Pixels>> {
    if p.source_width == 0 || p.source_height == 0 {
        return None;
    }
    let sx = f32::from(dest.size.width) / p.source_width as f32;
    let sy = f32::from(dest.size.height) / p.source_height as f32;
    Some(Bounds::new(point(dest.left() - px(p.source_x as f32 * sx), dest.top() - px(p.source_y as f32 * sy)), size(px(w * sx), px(h * sy))))
}
fn paint_kitty_layer(g: &mut Painter, frame: &crate::frame::Frame, layer: ImageLayer, origin: (f32, f32), cell: (f32, f32), scale: f32) {
    for p in frame.placements.iter().filter(|p| p.layer == layer) {
        let Some(Some(img)) = frame.textures.get(&(p.image_id, p.image_generation)) else {
            continue;
        };
        let b = image_rect(p, origin, cell, scale);
        let wh = img.size(0);
        let (w, h) = (u32::from(wh.width) as f32, u32::from(wh.height) as f32);
        let Some(bounds) = image_texture_bounds(p, b, w, h) else {
            continue;
        };
        g.with_content_mask(Some(ContentMask { bounds: b }), |g| {
            if p.pixel_width >= p.source_width || p.pixel_height >= p.source_height {
                let _ = g.paint_image_nearest(bounds, img.clone());
            } else {
                let _ = g.paint_image(bounds, olo_kit::Corners::default(), img.clone(), 0, false);
            }
        });
    }
}

/// Convert straight RGBA8 pixels to Olo's BGRA `RenderImage`.
pub fn rgba_to_render_image(rgba: &[u8], width: u32, height: u32) -> Option<Arc<RenderImage>> {
    if width == 0 || height == 0 || rgba.len() != (width as usize).checked_mul(height as usize)?.checked_mul(4)? {
        return None;
    }
    let mut bgra = Vec::with_capacity(rgba.len());
    for px_ in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[px_[2], px_[1], px_[0], px_[3]]);
    }
    let buf = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])))
}

fn wants_color_emoji(cluster: &str, wide: bool) -> bool {
    let Some(first) = cluster.chars().next() else { return false };
    (wide && !first.is_ascii()) || cluster.contains('\u{FE0F}') || ('\u{1F1E6}'..='\u{1F1FF}').contains(&first)
}

fn is_box_char(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c) || ('\u{e0b0}'..='\u{e0b3}').contains(&c)
}

fn paint_shapes(g: &mut Painter, shapes: &[Shape], (ox, oy): (f32, f32)) {
    for s in shapes {
        match s {
            Shape::Rect(x, y, w, h, c) => g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x), px(oy + y)), size(px(*w), px(*h))), *c)),
            Shape::Line(pts, width, c) => {
                let mut b = PathBuilder::stroke(px(*width));
                for (k, &(x, y)) in pts.iter().enumerate() {
                    let pt = point(px(ox + x), px(oy + y));
                    if k == 0 { b.move_to(pt) } else { b.line_to(pt) }
                }
                if let Ok(path) = b.build() {
                    g.paint_path(path, *c);
                }
            }
            Shape::Poly(pts, c) => {
                let mut b = PathBuilder::fill();
                for (k, &(x, y)) in pts.iter().enumerate() {
                    let pt = point(px(ox + x), px(oy + y));
                    if k == 0 { b.move_to(pt) } else { b.line_to(pt) }
                }
                b.close();
                if let Ok(path) = b.build() {
                    g.paint_path(path, *c);
                }
            }
        }
    }
}

fn draw_box(out: &mut Vec<Shape>, (rx, ry, rw, rh): (f32, f32, f32, f32), c: char, color: Hsla, scale: f32) {
    let one = 1.0 / scale;
    let snap = |v: f32| (v * scale).round() / scale;
    let light = (rw / 8.0 * scale).round().max(1.0) / scale;
    let heavy = light * 2.0;
    let (cx, cy) = (rx + rw / 2.0, ry + rh / 2.0);
    let (x1, y1) = (rx + rw, ry + rh);
    let mut fill = |x0: f32, y0: f32, x1_: f32, y1_: f32| out.push(Shape::Rect(x0, y0, (x1_ - x0).max(0.0), (y1_ - y0).max(0.0), color));
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
    let code = c as u32;
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
        0x256d..=0x2570 => {
            let (hx, vy) = match code {
                0x256d => (1.0, 1.0),
                0x256e => (-1.0, 1.0),
                0x256f => (-1.0, -1.0),
                _ => (1.0, -1.0),
            };
            let c_ = (snap(cx - n / 2.0) + n / 2.0, snap(cy - n / 2.0) + n / 2.0);
            let radius = (rw / 2.0).min(rh / 2.0);
            let arc = (c_.0 + hx * radius, c_.1 + vy * radius);
            let mut pts = vec![(c_.0, if vy > 0.0 { y1 } else { ry })];
            for k in 0..=12 {
                let t = k as f32 / 12.0 * std::f32::consts::FRAC_PI_2;
                pts.push((arc.0 - hx * radius * t.cos(), arc.1 - vy * radius * t.sin()));
            }
            pts.push((if hx > 0.0 { x1 } else { rx }, c_.1));
            out.push(Shape::Line(pts, n, color));
        }
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
            let gap = snap(light * 1.5).max(light * 1.5);
            let c_ = (snap(cx - n / 2.0), snap(cy - n / 2.0));
            for side in [-1.0f32, 1.0] {
                let y = c_.1 + side * gap;
                let inner = if if side < 0.0 { au } else { ad } { gap } else { -gap };
                if ar {
                    fill(c_.0 + inner, y, x1, y + n);
                }
                if al {
                    fill(rx, y, c_.0 - inner + n, y + n);
                }
                let x = c_.0 + side * gap;
                let inner = if if side < 0.0 { al } else { ar } { gap } else { -gap };
                if ad {
                    fill(x, c_.1 + inner, x + n, y1);
                }
                if au {
                    fill(x, ry, x + n, c_.1 - inner + n);
                }
            }
        }
        0x2580 => fill(rx, ry, x1, snap(cy)),
        0x2581..=0x2588 => {
            let e = (code - 0x2580) as f32;
            fill(rx, snap(y1 - rh * e / 8.0), x1, y1);
        }
        0x2589..=0x258f => {
            let e = (0x2590 - code) as f32;
            fill(rx, ry, snap(rx + rw * e / 8.0), y1);
        }
        0x2590 => fill(snap(cx), ry, x1, y1),
        0x2591..=0x2593 => {
            let a = [0.25, 0.5, 0.75][(code - 0x2591) as usize];
            out.push(Shape::Rect(rx, ry, rw, rh, color.opacity(a)));
        }
        0x2594 => fill(rx, ry, x1, snap(ry + rh / 8.0)),
        0x2595 => fill(snap(x1 - rw / 8.0), ry, x1, y1),
        0x2596..=0x259f => {
            let q = [0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111][(code - 0x2596) as usize];
            let (mx, my) = (snap(cx), snap(cy));
            let quads = [(rx, ry, mx, my), (mx, ry, x1, my), (rx, my, mx, y1), (mx, my, x1, y1)];
            for (i, (a, b, c2, d)) in quads.into_iter().enumerate() {
                if q & (0b1000 >> i) != 0 {
                    fill(a, b, c2, d);
                }
            }
        }
        0xe0b0 => out.push(Shape::Poly(vec![(rx, ry), (x1, cy), (rx, y1)], color)),
        0xe0b2 => out.push(Shape::Poly(vec![(x1, ry), (x1, y1), (rx, cy)], color)),
        0xe0b1 => out.push(Shape::Line(vec![(rx, ry), (x1 - one, cy), (rx, y1)], light, color)),
        0xe0b3 => out.push(Shape::Line(vec![(x1, ry), (rx + one, cy), (x1, y1)], light, color)),
        _ => seg(&mut fill, n, n, n, n),
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn source_crop_pixel_extents_and_fractional_offsets() {
        let p = ImagePlacement {
            image_id: 1,
            image_generation: 1,
            placement_id: 1,
            layer: ImageLayer::AboveText,
            z: 1,
            viewport_col: 2,
            viewport_row: 1,
            grid_cols: 3,
            grid_rows: 2,
            cell_offset_x: 3,
            cell_offset_y: 6,
            pixel_width: 12,
            pixel_height: 9,
            source_x: 2,
            source_y: 4,
            source_width: 2,
            source_height: 2,
        };

        let r = image_rect(&p, (10., 20.), (6., 12.), 1.5);
        assert_eq!(r, Bounds::new(point(px(24.), px(36.)), size(px(8.), px(6.))));
        assert_eq!(image_texture_bounds(&p, r, 8., 8.), Some(Bounds::new(point(px(16.), px(24.)), size(px(32.), px(24.)))));
        assert!(rgba_to_render_image(&[0; 3], 1, 1).is_none());
        assert!(rgba_to_render_image(&[0; 5], 1, 1).is_none());
        assert!(rgba_to_render_image(&[], u32::MAX, u32::MAX).is_none());
        assert_eq!(rgba_to_render_image(&[1, 2, 3, 255], 1, 1).unwrap().as_bytes(0).unwrap(), &[3, 2, 1, 255]);
    }
}
