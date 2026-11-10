use crate::{
    Bounds, Half, Hsla, LineLayout, Painter, Pixels, Point, Result, SharedString, StrikethroughStyle, TextAlign, UnderlineStyle, WrapBoundary, WrappedLineLayout, black, fill, point, px, size,
};
use derive_more::{Deref, DerefMut};
use smallvec::SmallVec;
use std::sync::Arc;

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
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
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
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
}

impl WrappedLine {
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
    let mut h = collections::FxHasher::default();
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
