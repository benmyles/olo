use crate::{
    Bounds, Half, Hsla, LineLayout, Painter, Pixels, Point, Result, SharedString, StrikethroughStyle, TextAlign, UnderlineStyle, WrapBoundary, WrappedLineLayout, black, fill, point, px, size,
};
use derive_more::{Deref, DerefMut};
use smallvec::SmallVec;
use std::sync::Arc;

/// A line's decoration runs: one kept in the line itself, more on the
/// heap. Nearly every line has one (a label, a cell, a menu's item), and
/// a shaped line is handed about by value: with room for 32 in it, as
/// GPUI had, a line was 3 KB and every label painted copied that twice
/// (a list resized live: 1.3 MB of it a frame).
pub(crate) type DecorationRuns = SmallVec<[DecorationRun; 1]>;

/// Set the text decoration for a run of text.
#[derive(Debug, Clone)]
pub struct DecorationRun {
    /// The length of the run in utf-8 bytes.
    pub len: u32,

    /// The color for this run
    pub color: Hsla,

    /// The background color for this run
    pub background_color: Option<Hsla>,

    /// The underline style for this run
    pub underline: Option<UnderlineStyle>,

    /// The strikethrough style for this run
    pub strikethrough: Option<StrikethroughStyle>,
}

/// A line of text that has been shaped and decorated.
#[derive(Clone, Default, Debug, Deref, DerefMut)]
pub struct ShapedLine {
    #[deref]
    #[deref_mut]
    pub(crate) layout: Arc<LineLayout>,
    /// The text that was shaped for this line.
    pub text: SharedString,
    pub(crate) decoration_runs: DecorationRuns,
}

impl ShapedLine {
    /// The length of the line in utf-8 bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.layout.len
    }

    /// Override the len, useful if you're rendering text a
    /// as text b (e.g. rendering invisibles).
    pub fn with_len(mut self, len: usize) -> Self {
        let layout = self.layout.as_ref();
        self.layout =
            Arc::new(LineLayout { font_size: layout.font_size, width: layout.width, ascent: layout.ascent, descent: layout.descent, runs: layout.runs.clone(), len, painted: Default::default() });
        self
    }

    /// Olo: the line with `spacing` more between each character and the
    /// next (letter-spacing, as CSS's; none after the last), its kerning
    /// kept.
    pub fn tracked(&self, spacing: Pixels) -> Self {
        let l = self.layout.as_ref();
        let mut n = 0.0;
        let runs: Vec<_> = l
            .runs
            .iter()
            .map(|run| {
                let mut run = run.clone();
                for glyph in &mut run.glyphs {
                    glyph.position.x += spacing * n;
                    n += 1.0;
                }
                run
            })
            .collect();
        let width = l.width + spacing * (n - 1.0f32).max(0.0);
        let layout = Arc::new(LineLayout { font_size: l.font_size, width, ascent: l.ascent, descent: l.descent, runs, len: l.len, painted: Default::default() });
        Self { layout, text: self.text.clone(), decoration_runs: self.decoration_runs.clone() }
    }

    /// Paint the line of text to the p.
    pub fn paint(&self, origin: Point<Pixels>, line_height: Pixels, p: &mut Painter) -> Result<()> {
        paint_line(origin, &self.layout, "", line_height, TextAlign::default(), None, &self.decoration_runs, &[], p)?;

        Ok(())
    }

    /// Paint the background of the line to the p.
    pub fn paint_background(&self, origin: Point<Pixels>, line_height: Pixels, p: &mut Painter) -> Result<()> {
        paint_line_background(origin, &self.layout, "", line_height, TextAlign::default(), None, &self.decoration_runs, &[], p)?;

        Ok(())
    }
}

/// A line of text that has been shaped, decorated, and wrapped by the text layout system.
#[derive(Clone, Default, Debug, Deref, DerefMut)]
pub struct WrappedLine {
    #[deref]
    #[deref_mut]
    pub(crate) layout: Arc<WrappedLineLayout>,
    /// The text that was shaped for this line.
    pub text: SharedString,
    pub(crate) decoration_runs: DecorationRuns,
}

/// Where a paragraph's last line begins instead, so it isn't one word
/// alone (`WrappedLine::keep_last_words_together`): the byte of the
/// last word of the line over it. `text` wrapped to `room` has its last
/// line from `last_at` and the one over it from `over` (its byte and
/// its x); `width` is the text's on one line, `x_of` a byte's x. None
/// where the last line isn't one word begun after a space, the line
/// over it is one word too, the two words don't fit the last line, or
/// the line they'd leave would be the shorter.
fn last_words_break(text: &str, over: (usize, Pixels), last_at: usize, width: Pixels, room: Pixels, x_of: impl Fn(usize) -> Option<Pixels>) -> Option<usize> {
    let last_line = text.get(last_at..)?.trim_end();
    if last_line.is_empty() || last_line.contains(' ') || !text[..last_at].ends_with(' ') {
        return None;
    }
    let (over_at, over_x) = over;
    let space_at = over_at + text.get(over_at..last_at)?.trim_end().rfind(' ')?;
    let (space_x, word_x) = (x_of(space_at)?, x_of(space_at + 1)?);
    let below = width - word_x;
    (below <= room && space_x - over_x >= below).then_some(space_at + 1)
}

impl WrappedLine {
    /// Olo: a paragraph's last line is never one word alone where the
    /// word before can come down to it. Wrapping fills each line before
    /// the next begins, which leaves "…in Deskbar's / tray." in one
    /// language or another whatever the words; a paragraph set to read
    /// (a note, an alert's message) takes its last two words together
    /// instead, where both fit the last line and the line they leave
    /// stays the longer (so a short line over a long one never comes of
    /// it). The number of lines is the same. For words apart by spaces:
    /// a script that breaks anywhere has no such word. Not for text
    /// being typed, where a word would jump as the next is begun, nor
    /// for text cut to so many lines: the kit's `Para` asks for it.
    pub fn keep_last_words_together(&mut self) {
        let layout = &self.layout;
        let (Some(room), Some(&last)) = (layout.wrap_width, layout.wrap_boundaries.last()) else { return };
        let glyph = |b: WrapBoundary| &layout.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix];
        let text: &str = self.text.as_ref();
        let last_at = glyph(last).index;
        let over = layout.wrap_boundaries.len().checked_sub(2).map(|i| layout.wrap_boundaries[i]);
        let over = over.map_or((0, px(0.)), |b| (glyph(b).index, glyph(b).position.x));
        let glyphs = || layout.unwrapped_layout.runs.iter().enumerate().flat_map(|(run_ix, run)| run.glyphs.iter().enumerate().map(move |(glyph_ix, g)| (WrapBoundary { run_ix, glyph_ix }, g)));
        let x_of = |at: usize| glyphs().find(|(_, g)| g.index == at).map(|(_, g)| g.position.x);
        let Some(word_at) = last_words_break(text, over, last_at, layout.unwrapped_layout.width, room, x_of) else { return };
        let Some((word, _)) = glyphs().find(|(_, g)| g.index == word_at) else { return };
        let mut wrap_boundaries = layout.wrap_boundaries.clone();
        *wrap_boundaries.last_mut().expect("a boundary") = word;
        self.layout = Arc::new(WrappedLineLayout { unwrapped_layout: layout.unwrapped_layout.clone(), wrap_boundaries, wrap_width: layout.wrap_width });
    }

    /// The length of the underlying, unwrapped layout, in utf-8 bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.layout.len()
    }

    /// Paint this line of text to the p.
    pub fn paint(&self, origin: Point<Pixels>, line_height: Pixels, align: TextAlign, bounds: Option<Bounds<Pixels>>, p: &mut Painter) -> Result<()> {
        let align_width = match bounds {
            Some(bounds) => Some(bounds.size.width),
            None => self.layout.wrap_width,
        };

        paint_line(origin, &self.layout.unwrapped_layout, &self.text, line_height, align, align_width, &self.decoration_runs, &self.wrap_boundaries, p)?;

        Ok(())
    }

    /// Olo: where bytes `range` of the line show, a rectangle for each row
    /// they're on (the line's top-left at `origin`), placed as `paint`
    /// places its rows: a link's, for the pointer and agents.
    pub fn range_rects(&self, origin: Point<Pixels>, line_height: Pixels, align: TextAlign, align_width: Option<Pixels>, range: std::ops::Range<usize>) -> SmallVec<[Bounds<Pixels>; 2]> {
        let layout = &self.layout.unwrapped_layout;
        let glyph_at = |b: &WrapBoundary| &layout.runs[b.run_ix].glyphs[b.glyph_ix];
        let mut rects = SmallVec::new();
        if range.is_empty() {
            return rects;
        }
        let (mut row_start, mut row_x) = (0, px(0.));
        for row in 0..=self.wrap_boundaries.len() {
            let next = self.wrap_boundaries.get(row);
            let row_end = next.map_or(layout.len, |b| glyph_at(b).index);
            let (a, b) = (range.start.max(row_start), range.end.min(row_end));
            if a < b {
                let x = aligned_origin_x(origin, align_width.unwrap_or(layout.width), row_x, &align, layout, next.as_ref(), &self.text);
                let (x0, x1) = (layout.x_for_index(a) - row_x, layout.x_for_index(b) - row_x);
                rects.push(Bounds::new(point(x + x0, origin.y + line_height * row as f32), size(x1 - x0, line_height)));
            }
            if row_end >= range.end {
                break;
            }
            if let Some(b) = next {
                (row_start, row_x) = (glyph_at(b).index, glyph_at(b).position.x);
            }
        }
        rects
    }

    /// Paint the background of line of text to the p.
    pub fn paint_background(&self, origin: Point<Pixels>, line_height: Pixels, align: TextAlign, bounds: Option<Bounds<Pixels>>, p: &mut Painter) -> Result<()> {
        let align_width = match bounds {
            Some(bounds) => Some(bounds.size.width),
            None => self.layout.wrap_width,
        };

        paint_line_background(origin, &self.layout.unwrapped_layout, &self.text, line_height, align, align_width, &self.decoration_runs, &self.wrap_boundaries, p)?;

        Ok(())
    }
}

fn paint_line(
    origin: Point<Pixels>,
    layout: &LineLayout,
    text: &str,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    decoration_runs: &[DecorationRun],
    wrap_boundaries: &[WrapBoundary],
    p: &mut Painter,
) -> Result<()> {
    let line_bounds = Bounds::new(origin, size(layout.width, line_height * (wrap_boundaries.len() as f32 + 1.)));
    // Olo: a line of plain glyphs (no underline, strikethrough or emoji:
    // nearly every line) is painted from what it found last time, moved
    // (`LineLayout::painted`): a window of text redrawn for a hover was
    // an atlas lookup and a scene insert a glyph, 1.6 µs each (a full
    // screen of Notifications' history 15–25 ms a frame).
    let plain = decoration_runs.iter().all(|r| r.underline.is_none() && r.strikethrough.is_none()) && layout.runs.iter().all(|r| r.glyphs.iter().all(|g| !g.is_emoji));
    if plain {
        let stamp = line_stamp(line_height, align, align_width, decoration_runs, wrap_boundaries);
        let mut painted = layout.painted.0.lock();
        p.paint_layer(line_bounds, |p| {
            p.paint_glyph_run(&mut painted, origin, stamp, |emit| plain_glyphs(origin, layout, text, line_height, align, align_width, decoration_runs, wrap_boundaries, emit))
        });
        return Ok(());
    }
    p.paint_layer(line_bounds, |p| {
        let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
        let baseline_offset = point(px(0.), padding_top + layout.ascent);
        let mut decoration_runs = decoration_runs.iter();
        let mut wraps = wrap_boundaries.iter().peekable();
        let mut run_end = 0;
        let mut color = black();
        let mut current_underline: Option<(Point<Pixels>, UnderlineStyle)> = None;
        let mut current_strikethrough: Option<(Point<Pixels>, StrikethroughStyle)> = None;
        let text_system = p.text_system().clone();
        let mut glyph_origin = point(aligned_origin_x(origin, align_width.unwrap_or(layout.width), px(0.0), &align, layout, wraps.peek(), text), origin.y);
        let mut prev_glyph_position = Point::default();
        let mut max_glyph_size = size(px(0.), px(0.));
        let mut first_glyph_x = origin.x;
        for (run_ix, run) in layout.runs.iter().enumerate() {
            max_glyph_size = text_system.bounding_box(run.font_id, run.font_size).size;

            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                glyph_origin.x += glyph.position.x - prev_glyph_position.x;
                if glyph_ix == 0 && run_ix == 0 {
                    first_glyph_x = glyph_origin.x;
                }

                if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    wraps.next();
                    if let Some((underline_origin, underline_style)) = current_underline.as_mut() {
                        if glyph_origin.x == underline_origin.x {
                            underline_origin.x -= max_glyph_size.width.half();
                        };
                        p.paint_underline(*underline_origin, glyph_origin.x - underline_origin.x, underline_style);
                        underline_origin.x = origin.x;
                        underline_origin.y += line_height;
                    }
                    if let Some((strikethrough_origin, strikethrough_style)) = current_strikethrough.as_mut() {
                        if glyph_origin.x == strikethrough_origin.x {
                            strikethrough_origin.x -= max_glyph_size.width.half();
                        };
                        p.paint_strikethrough(*strikethrough_origin, glyph_origin.x - strikethrough_origin.x, strikethrough_style);
                        strikethrough_origin.x = origin.x;
                        strikethrough_origin.y += line_height;
                    }

                    glyph_origin.x = aligned_origin_x(origin, align_width.unwrap_or(layout.width), glyph.position.x, &align, layout, wraps.peek(), text);
                    glyph_origin.y += line_height;
                }
                prev_glyph_position = glyph.position;

                let mut finished_underline: Option<(Point<Pixels>, UnderlineStyle)> = None;
                let mut finished_strikethrough: Option<(Point<Pixels>, StrikethroughStyle)> = None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();

                    // ignore style runs that apply to a partial glyph
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        if let Some((_, underline_style)) = &mut current_underline
                            && style_run.underline.as_ref() != Some(underline_style)
                        {
                            finished_underline = current_underline.take();
                        }
                        if let Some(run_underline) = style_run.underline.as_ref() {
                            current_underline.get_or_insert((
                                point(glyph_origin.x, glyph_origin.y + baseline_offset.y + (layout.descent * 0.618)),
                                UnderlineStyle { color: Some(run_underline.color.unwrap_or(style_run.color)), thickness: run_underline.thickness, wavy: run_underline.wavy },
                            ));
                        }
                        if let Some((_, strikethrough_style)) = &mut current_strikethrough
                            && style_run.strikethrough.as_ref() != Some(strikethrough_style)
                        {
                            finished_strikethrough = current_strikethrough.take();
                        }
                        if let Some(run_strikethrough) = style_run.strikethrough.as_ref() {
                            current_strikethrough.get_or_insert((
                                point(glyph_origin.x, glyph_origin.y + (((layout.ascent * 0.5) + baseline_offset.y) * 0.5)),
                                StrikethroughStyle { color: Some(run_strikethrough.color.unwrap_or(style_run.color)), thickness: run_strikethrough.thickness },
                            ));
                        }

                        run_end += style_run.len as usize;
                        color = style_run.color;
                    } else {
                        run_end = layout.len;
                        finished_underline = current_underline.take();
                        finished_strikethrough = current_strikethrough.take();
                    }
                }

                if let Some((mut underline_origin, underline_style)) = finished_underline {
                    if underline_origin.x == glyph_origin.x {
                        underline_origin.x -= max_glyph_size.width.half();
                    };
                    p.paint_underline(underline_origin, glyph_origin.x - underline_origin.x, &underline_style);
                }

                if let Some((mut strikethrough_origin, strikethrough_style)) = finished_strikethrough {
                    if strikethrough_origin.x == glyph_origin.x {
                        strikethrough_origin.x -= max_glyph_size.width.half();
                    };
                    p.paint_strikethrough(strikethrough_origin, glyph_origin.x - strikethrough_origin.x, &strikethrough_style);
                }

                let max_glyph_bounds = Bounds { origin: glyph_origin, size: max_glyph_size };

                let content_mask = p.content_mask();
                if max_glyph_bounds.intersects(&content_mask.bounds) {
                    if glyph.is_emoji {
                        p.paint_emoji(glyph_origin + baseline_offset, run.font_id, glyph.id, run.font_size)?;
                    } else {
                        p.paint_glyph(glyph_origin + baseline_offset, run.font_id, glyph.id, run.font_size, color)?;
                    }
                } else {
                    p.note_culled();
                }
            }
        }

        let mut last_line_end_x = first_glyph_x + layout.width;
        if let Some(boundary) = wrap_boundaries.last() {
            let run = &layout.runs[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            last_line_end_x -= glyph.position.x;
        }

        if let Some((mut underline_start, underline_style)) = current_underline.take() {
            if last_line_end_x == underline_start.x {
                underline_start.x -= max_glyph_size.width.half()
            };
            p.paint_underline(underline_start, last_line_end_x - underline_start.x, &underline_style);
        }

        if let Some((mut strikethrough_start, strikethrough_style)) = current_strikethrough.take() {
            if last_line_end_x == strikethrough_start.x {
                strikethrough_start.x -= max_glyph_size.width.half()
            };
            p.paint_strikethrough(strikethrough_start, last_line_end_x - strikethrough_start.x, &strikethrough_style);
        }

        Ok(())
    })
}

fn paint_line_background(
    origin: Point<Pixels>,
    layout: &LineLayout,
    text: &str,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    decoration_runs: &[DecorationRun],
    wrap_boundaries: &[WrapBoundary],
    p: &mut Painter,
) -> Result<()> {
    // (Nearly every line has none: then nothing to look for, glyph by
    // glyph, as every line painted did, a list's rows on each step of a
    // live resize among them.)
    if decoration_runs.iter().all(|run| run.background_color.is_none()) {
        return Ok(());
    }
    let line_bounds = Bounds::new(origin, size(layout.width, line_height * (wrap_boundaries.len() as f32 + 1.)));
    p.paint_layer(line_bounds, |p| {
        let mut decoration_runs = decoration_runs.iter();
        let mut wraps = wrap_boundaries.iter().peekable();
        let mut run_end = 0;
        let mut current_background: Option<(Point<Pixels>, Hsla)> = None;
        let text_system = p.text_system().clone();
        let mut glyph_origin = point(aligned_origin_x(origin, align_width.unwrap_or(layout.width), px(0.0), &align, layout, wraps.peek(), text), origin.y);
        let mut prev_glyph_position = Point::default();
        let mut max_glyph_size = size(px(0.), px(0.));
        for (run_ix, run) in layout.runs.iter().enumerate() {
            max_glyph_size = text_system.bounding_box(run.font_id, run.font_size).size;

            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                glyph_origin.x += glyph.position.x - prev_glyph_position.x;

                if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    wraps.next();
                    if let Some((background_origin, background_color)) = current_background.as_mut() {
                        if glyph_origin.x == background_origin.x {
                            background_origin.x -= max_glyph_size.width.half()
                        }
                        p.paint_quad(fill(Bounds { origin: *background_origin, size: size(glyph_origin.x - background_origin.x, line_height) }, *background_color));
                        background_origin.x = origin.x;
                        background_origin.y += line_height;
                    }

                    glyph_origin.x = aligned_origin_x(origin, align_width.unwrap_or(layout.width), glyph.position.x, &align, layout, wraps.peek(), text);
                    glyph_origin.y += line_height;
                }
                prev_glyph_position = glyph.position;

                let mut finished_background: Option<(Point<Pixels>, Hsla)> = None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();

                    // ignore style runs that apply to a partial glyph
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        if let Some((_, background_color)) = &mut current_background
                            && style_run.background_color.as_ref() != Some(background_color)
                        {
                            finished_background = current_background.take();
                        }
                        if let Some(run_background) = style_run.background_color {
                            current_background.get_or_insert((point(glyph_origin.x, glyph_origin.y), run_background));
                        }
                        run_end += style_run.len as usize;
                    } else {
                        run_end = layout.len;
                        finished_background = current_background.take();
                    }
                }

                if let Some((mut background_origin, background_color)) = finished_background {
                    let mut width = glyph_origin.x - background_origin.x;
                    if background_origin.x == glyph_origin.x {
                        background_origin.x -= max_glyph_size.width.half();
                    };
                    p.paint_quad(fill(Bounds { origin: background_origin, size: size(width, line_height) }, background_color));
                }
            }
        }

        let mut last_line_end_x = origin.x + layout.width;
        if let Some(boundary) = wrap_boundaries.last() {
            let run = &layout.runs[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            last_line_end_x -= glyph.position.x;
        }

        if let Some((mut background_origin, background_color)) = current_background.take() {
            if last_line_end_x == background_origin.x {
                background_origin.x -= max_glyph_size.width.half()
            };
            p.paint_quad(fill(Bounds { origin: background_origin, size: size(last_line_end_x - background_origin.x, line_height) }, background_color));
        }

        Ok(())
    })
}

/// What a plain line's glyph positions and colours depend on besides
/// its layout and where it starts: its key in `LineLayout::painted`.
fn line_stamp(line_height: Pixels, align: TextAlign, align_width: Option<Pixels>, decoration_runs: &[DecorationRun], wrap_boundaries: &[WrapBoundary]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    line_height.0.to_bits().hash(&mut h);
    (align as u8).hash(&mut h);
    align_width.map(|w| w.0.to_bits()).hash(&mut h);
    for r in decoration_runs {
        r.len.hash(&mut h);
        [r.color.h, r.color.s, r.color.l, r.color.a].map(f32::to_bits).hash(&mut h);
    }
    for w in wrap_boundaries {
        (w.run_ix, w.glyph_ix).hash(&mut h);
    }
    h.finish()
}

/// A plain line's glyphs (no decorations, no emoji), each with where it
/// sits from `origin` (its baseline), its face, size and colour: as
/// `paint_line` places and colours them.
#[allow(clippy::too_many_arguments)]
fn plain_glyphs(
    origin: Point<Pixels>,
    layout: &LineLayout,
    text: &str,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    decoration_runs: &[DecorationRun],
    wrap_boundaries: &[WrapBoundary],
    emit: &mut dyn FnMut(Point<Pixels>, crate::FontId, crate::GlyphId, Pixels, Hsla),
) {
    let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
    let baseline_offset = point(px(0.), padding_top + layout.ascent);
    let mut decoration_runs = decoration_runs.iter();
    let mut wraps = wrap_boundaries.iter().peekable();
    let mut run_end = 0;
    let mut color = black();
    let mut glyph_origin = point(aligned_origin_x(origin, align_width.unwrap_or(layout.width), px(0.0), &align, layout, wraps.peek(), text), origin.y);
    let mut prev_glyph_position = Point::default();
    for (run_ix, run) in layout.runs.iter().enumerate() {
        for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
            glyph_origin.x += glyph.position.x - prev_glyph_position.x;
            if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                wraps.next();
                glyph_origin.x = aligned_origin_x(origin, align_width.unwrap_or(layout.width), glyph.position.x, &align, layout, wraps.peek(), text);
                glyph_origin.y += line_height;
            }
            prev_glyph_position = glyph.position;
            if glyph.index >= run_end {
                let mut style_run = decoration_runs.next();
                // (A style run that applies to part of a glyph is passed.)
                while let Some(run) = style_run {
                    if glyph.index < run_end + (run.len as usize) {
                        break;
                    }
                    run_end += run.len as usize;
                    style_run = decoration_runs.next();
                }
                if let Some(style_run) = style_run {
                    run_end += style_run.len as usize;
                    color = style_run.color;
                } else {
                    run_end = layout.len;
                }
            }
            emit(glyph_origin + baseline_offset - origin, run.font_id, glyph.id, run.font_size, color);
        }
    }
}

fn aligned_origin_x(origin: Point<Pixels>, align_width: Pixels, last_glyph_x: Pixels, align: &TextAlign, layout: &LineLayout, wrap_boundary: Option<&&WrapBoundary>, text: &str) -> Pixels {
    if *align == TextAlign::Left {
        return origin.x;
    }
    let end_of_line = match wrap_boundary {
        Some(boundary) => row_end(layout, text, boundary),
        None => layout.width,
    };

    let line_width = end_of_line - last_glyph_x;

    match align {
        TextAlign::Left => origin.x,
        TextAlign::Center => (origin.x * 2.0 + align_width - line_width) / 2.0,
        TextAlign::Right => origin.x + align_width - line_width,
    }
}

/// Olo: where a wrapped row's text ends: the next row's start, less the
/// spaces before it, which hang past the edge (a centred or right-aligned
/// row is placed by its words, not its spaces; upstream counted them).
fn row_end(layout: &LineLayout, text: &str, boundary: &WrapBoundary) -> Pixels {
    let mut end = layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].position.x;
    let (mut run, mut glyph) = (boundary.run_ix, boundary.glyph_ix);
    loop {
        if glyph == 0 {
            if run == 0 {
                break;
            }
            run -= 1;
            glyph = layout.runs[run].glyphs.len();
            continue;
        }
        glyph -= 1;
        let g = &layout.runs[run].glyphs[glyph];
        if !text.get(g.index..).and_then(|t| t.chars().next()).is_some_and(char::is_whitespace) {
            break;
        }
        end = g.position.x;
    }
    end
}

#[cfg(test)]
mod last_words_tests {
    use super::*;

    /// Text in a face of ten points a letter, wrapped as the layout
    /// wraps it (each line as many words as fit): the last line's start,
    /// and where the rule would have it.
    fn wrapped(text: &str, room: f32) -> (Vec<&str>, Option<Vec<&str>>) {
        let x = |at: usize| px(at as f32 * 10.0);
        let mut starts = vec![0];
        let mut line = 0;
        for (at, _) in text.match_indices(' ') {
            let end = text[at + 1..].find(' ').map_or(text.len(), |n| at + 1 + n);
            if (end - line) as f32 * 10.0 > room {
                line = at + 1;
                starts.push(line);
            }
        }
        let lines = |starts: &[usize]| starts.iter().enumerate().map(|(i, s)| text[*s..starts.get(i + 1).copied().unwrap_or(text.len())].trim_end()).collect::<Vec<_>>();
        let n = starts.len();
        let ruled = (n >= 2).then(|| last_words_break(text, (starts[n - 2], x(starts[n - 2])), starts[n - 1], x(text.len()), px(room), |at| Some(x(at)))).flatten().map(|at| {
            let mut s = starts.clone();
            s[n - 1] = at;
            lines(&s)
        });
        (lines(&starts), ruled)
    }

    #[test]
    fn a_last_word_takes_the_one_before_it_down() {
        // "…from Deskbar's / tray." becomes "…from / Deskbar's tray."
        let (was, now) = wrapped("Choose a workspace's own from Deskbar's tray.", 400.0);
        assert_eq!(was, ["Choose a workspace's own from Deskbar's", "tray."]);
        assert_eq!(now.unwrap(), ["Choose a workspace's own from", "Deskbar's tray."]);
        // Three lines: only the last two change.
        let (was, now) = wrapped("one two three four five six seven", 130.0);
        assert_eq!(was, ["one two three", "four five six", "seven"]);
        assert_eq!(now.unwrap(), ["one two three", "four five", "six seven"]);
    }

    #[test]
    fn where_it_would_read_worse_nothing_moves() {
        // The line it would leave would be the shorter.
        let (was, now) = wrapped("Write up the design review", 200.0);
        assert_eq!((was, now), (vec!["Write up the design", "review"], None));
        // The two words don't fit one line.
        assert_eq!(wrapped("a extraordinarily long", 170.0).1, None);
        // The line over it is one word; two words on the last line; one line.
        assert_eq!(wrapped("extraordinary word", 130.0).1, None);
        assert_eq!(wrapped("one two three four five", 130.0), (vec!["one two three", "four five"], None));
        assert_eq!(wrapped("one two", 130.0).1, None);
        // A script that breaks anywhere has no such word.
        assert_eq!(last_words_break("各ウィンドウが残りのスペース", (0, px(0.)), 12, px(140.0), px(60.0), |at| Some(px(at as f32))), None);
    }
}
