//! SVG `<text>`: shaped with the engine's fonts when the layout is prepared
//! (the viewport loader has no shaper), each glyph's outline appended to
//! one path in the viewport's user space.
use crate::{
    css::style::Style,
    dom::NodeId,
    font::Scope,
    layout::{Error, Result},
    text::shaping,
};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    Start,
    Middle,
    End,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Baseline {
    Alphabetic,
    Central,
    Middle,
    Hanging,
    TextTop,
    TextBottom,
}
/// A text element as loaded: its content and placement, shaped later.
#[derive(PartialEq)]
pub struct Source {
    pub node: NodeId,
    pub text: Arc<str>,
    /// The start point, after dx/dy; a percentage is of the viewport,
    /// applied when the geometry is resolved.
    pub x: f32,
    pub y: f32,
    pub x_percent: bool,
    pub y_percent: bool,
    pub anchor: Anchor,
    pub baseline: Baseline,
    pub transform: Option<tiny_skia::Transform>,
}
struct Outline {
    path: tiny_skia::PathBuilder,
    transform: tiny_skia::Transform,
    segments: usize,
    limit: usize,
}
impl Outline {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let mut p = tiny_skia::Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        (p.x, p.y)
    }
    fn next(&mut self) -> bool {
        self.segments += 1;
        self.segments <= self.limit
    }
}
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.next() {
            let (x, y) = self.point(x, y);
            self.path.move_to(x, y);
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if self.next() {
            let (x, y) = self.point(x, y);
            self.path.line_to(x, y);
        }
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if self.next() {
            let (x1, y1) = self.point(x1, y1);
            let (x, y) = self.point(x, y);
            self.path.quad_to(x1, y1, x, y);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if self.next() {
            let (x1, y1) = self.point(x1, y1);
            let (x2, y2) = self.point(x2, y2);
            let (x, y) = self.point(x, y);
            self.path.cubic_to(x1, y1, x2, y2, x, y);
        }
    }
    fn close(&mut self) {
        if self.next() {
            self.path.close();
        }
    }
}
/// The text's glyph outlines as one path in the text's own space (the
/// percentage offsets and the element's transform are applied when the
/// geometry is resolved), or None when it has no ink.
pub(crate) fn shape(engine: &mut shaping::Engine, scope: &Scope, style: &Style, source: &Source, segments_limit: usize, cancel: Option<&AtomicBool>) -> Result<Option<tiny_skia::Path>> {
    if source.text.trim().is_empty() {
        return Ok(None);
    }
    let mut job = shaping::ParagraphJob::for_layout_style(engine, scope, style, source.text.clone(), Default::default(), cancel)?;
    while !job.step(128)?.finished {}
    let paragraph = job.finish()?;
    // Advance first, for the anchor.
    let mut width = 0f64;
    for run in paragraph.runs.iter() {
        let unit = run.size / f64::from(run.font.units_per_em()).max(1.);
        width += run.glyphs.iter().map(|g| f64::from(g.x_advance)).sum::<f64>() * unit;
    }
    let start = match source.anchor {
        Anchor::Start => 0.,
        Anchor::Middle => -width / 2.,
        Anchor::End => -width,
    };
    let mut builder = Outline { path: tiny_skia::PathBuilder::new(), transform: tiny_skia::Transform::identity(), segments: 0, limit: segments_limit };
    let mut pen = if source.x_percent { start } else { f64::from(source.x) + start };
    for run in paragraph.runs.iter() {
        let unit = run.size / f64::from(run.font.units_per_em()).max(1.);
        let face = run.font.selection().parsed().map_err(|_| Error::Invalid)?;
        let (ascent, descent) = (f64::from(face.ascender()), f64::from(face.descender()));
        let x_height = face.x_height().map_or(ascent / 2., f64::from);
        // The alphabetic baseline for the asked-for baseline at `y`.
        let baseline = if source.y_percent { 0. } else { f64::from(source.y) }
            + match source.baseline {
                Baseline::Alphabetic => 0.,
                Baseline::Central => (ascent + descent) / 2. * unit,
                Baseline::Middle => x_height / 2. * unit,
                Baseline::Hanging => ascent * 0.8 * unit,
                Baseline::TextTop => ascent * unit,
                Baseline::TextBottom => descent * unit,
            };
        for g in run.glyphs.iter() {
            if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            let x = pen + f64::from(g.x_offset) * unit;
            let y = baseline - f64::from(g.y_offset) * unit;
            builder.transform = tiny_skia::Transform::from_row(unit as f32, 0., 0., -unit as f32, x as f32, y as f32);
            if let Ok(id) = u16::try_from(g.id) {
                face.outline_glyph(ttf_parser::GlyphId(id), &mut builder);
            }
            if builder.segments > builder.limit {
                return Err(Error::Limit);
            }
            pen += f64::from(g.x_advance) * unit;
        }
    }
    Ok(builder.path.finish())
}
/// The shaped path placed: percentage offsets against the viewport, then
/// the element's transform.
pub(crate) fn place(source: &Source, path: &tiny_skia::Path, viewport: [f64; 2]) -> Option<tiny_skia::Path> {
    let x = if source.x_percent { f64::from(source.x) / 100. * viewport[0] } else { 0. };
    let y = if source.y_percent { f64::from(source.y) / 100. * viewport[1] } else { 0. };
    let mut t = tiny_skia::Transform::from_translate(x as f32, y as f32);
    if let Some(own) = source.transform {
        t = t.post_concat(own);
    }
    if t.is_identity() { Some(path.clone()) } else { path.clone().transform(t) }
}
