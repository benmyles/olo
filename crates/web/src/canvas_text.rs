//! Text for a canvas (HTML §4.12.5.1.4): its `font` (the CSS `font`
//! shorthand, parsed by the engine's own property parser) as a font
//! request, and a string shaped with the page's fonts into glyph outlines
//! and the metrics `measureText` gives. The canvas fills or strokes the
//! outlines with its own transform, style and alignment.
use crate::{
    css::{
        Syntax, math,
        properties::{self as p, Keyword as K, Property as P},
        style::{FontRequest, FontSlant},
    },
    font::Scope,
    text::shaping::{BaseDirection, Engine, ParagraphJob, ParagraphLimits},
};
use std::sync::{Arc, atomic::AtomicBool};

/// A canvas's default font size, and what `em` is against in a font
/// string (a canvas out of the document: Chrome's 10px).
const DEFAULT_SIZE: f64 = 10.;

struct Units;
impl math::Context for Units {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        use math::RelativeUnit as U;
        match unit {
            U::Em | U::Percentage => Ok(DEFAULT_SIZE / if unit == U::Percentage { 100. } else { 1. }),
            U::Rem => Ok(16.),
            U::Ex => Ok(DEFAULT_SIZE / 2.),
            U::Ch => Ok(DEFAULT_SIZE / 2.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}

/// The font a canvas `font` string asks for, or None when it doesn't
/// parse as a CSS `font` (the attribute then keeps its old value).
pub fn font_request(font: &str) -> Option<FontRequest> {
    if font.len() > 1024 {
        return None;
    }
    let syntax = Syntax::parse(Arc::from(font), Default::default(), None).ok()?;
    let p::Declaration::Known { values, .. } = p::Declaration::parse("font", syntax.components(), false, p::Limits::default(), None).ok()? else {
        return None;
    };
    let mut request = FontRequest { size: DEFAULT_SIZE, weight: 400., width: 100., slant: FontSlant::Normal, families: p::Families::generic(p::GenericFamily::SansSerif), axes: Default::default() };
    for a in &values {
        let number = |v: &p::Value| match v {
            p::Value::Numeric(n) => n.resolve(&Units, &mut math::Scratch::default(), Default::default(), None).ok(),
            _ => None,
        };
        match (a.property, &a.value) {
            // CSS-wide keywords and var() aren't fonts a canvas takes.
            (_, p::Value::Wide(_) | p::Value::Pending(_)) => return None,
            (P::FontFamily, p::Value::Families(f)) => request.families = f.clone(),
            (P::FontSize, p::Value::Keyword(k)) => {
                request.size = match k {
                    K::XxSmall => 9.,
                    K::XSmall => 10.,
                    K::Small => 13.,
                    K::Medium => 16.,
                    K::Large => 18.,
                    K::XLarge => 24.,
                    K::XxLarge => 32.,
                    K::XxxLarge => 48.,
                    K::Larger => DEFAULT_SIZE * 1.2,
                    K::Smaller => DEFAULT_SIZE / 1.2,
                    _ => DEFAULT_SIZE,
                }
            }
            (P::FontSize, v) => request.size = number(v).filter(|s| s.is_finite() && *s >= 0.)?,
            (P::FontWeight, p::Value::Keyword(k)) => {
                request.weight = match k {
                    K::Bold | K::Bolder => 700.,
                    K::Lighter => 100.,
                    _ => 400.,
                }
            }
            (P::FontWeight, v) => request.weight = number(v).unwrap_or(400.).clamp(1., 1000.),
            (P::FontStyle, p::Value::Keyword(K::Italic)) => request.slant = FontSlant::Italic,
            (P::FontStyle, p::Value::Keyword(K::Oblique)) => request.slant = FontSlant::Oblique(14.),
            (P::FontWidth, v) => request.width = number(v).unwrap_or(100.),
            _ => {}
        }
    }
    Some(request)
}

/// A string shaped for a canvas: its glyphs' outlines on one path (the
/// pen at the origin on the alphabetic baseline, y down), its advance,
/// the primary font's ascent and descent (positive, CSS px) and the ink's
/// bounds about the pen (left and right of it, above and below the
/// baseline, as measureText's actualBoundingBox*).
pub struct Shaped {
    pub path: Option<tiny_skia::Path>,
    pub advance: f64,
    pub ascent: f64,
    pub descent: f64,
    pub ink: [f64; 4],
}

struct Outline {
    path: tiny_skia::PathBuilder,
    transform: tiny_skia::Transform,
    segments: usize,
}
impl Outline {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let mut p = tiny_skia::Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        (p.x, p.y)
    }
}
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segments += 1;
        let (x, y) = self.point(x, y);
        self.path.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segments += 1;
        let (x, y) = self.point(x, y);
        self.path.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.segments += 1;
        let (x1, y1) = self.point(x1, y1);
        let (x, y) = self.point(x, y);
        self.path.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.segments += 1;
        let (x1, y1) = self.point(x1, y1);
        let (x2, y2) = self.point(x2, y2);
        let (x, y) = self.point(x, y);
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.segments += 1;
        self.path.close();
    }
}

/// How many outline segments one string may make (a canvas string is a
/// line; a page drawing megabytes of text in one call gets none).
const SEGMENTS: usize = 1 << 20;

/// Shape `text` (its newlines and tabs as spaces, as the canvas spec
/// replaces them) in `request` with the page's fonts.
pub fn shape(engine: &mut Engine, scope: &Scope, request: &FontRequest, text: &str, cancel: Option<&AtomicBool>) -> Option<Shaped> {
    let text: String = text.chars().map(|c| if matches!(c, '\t' | '\n' | '\x0c' | '\r') { ' ' } else { c }).collect();
    let mut shaped = Shaped { path: None, advance: 0., ascent: request.size * 0.8, descent: request.size * 0.2, ink: [0.; 4] };
    if text.is_empty() {
        return Some(shaped);
    }
    let mut job = ParagraphJob::new(engine, scope, request.clone(), Arc::from(text), BaseDirection::Ltr, ParagraphLimits::default(), cancel).ok()?;
    while !job.step(256).ok()?.finished {}
    let paragraph = job.finish().ok()?;
    let mut builder = Outline { path: tiny_skia::PathBuilder::new(), transform: tiny_skia::Transform::identity(), segments: 0 };
    let mut pen = 0f64;
    let mut first = true;
    for run in paragraph.runs.iter() {
        let unit = run.size / f64::from(run.font.units_per_em()).max(1.);
        let face = run.font.selection().parsed().ok()?;
        if first {
            // The primary font's: what fontBoundingBox* and the baselines
            // are measured against.
            shaped.ascent = f64::from(face.ascender()) * unit;
            shaped.descent = -f64::from(face.descender()) * unit;
            first = false;
        }
        for g in run.glyphs.iter() {
            let x = pen + f64::from(g.x_offset) * unit;
            let y = -f64::from(g.y_offset) * unit;
            builder.transform = tiny_skia::Transform::from_row(unit as f32, 0., 0., -unit as f32, x as f32, y as f32);
            if let Ok(id) = u16::try_from(g.id) {
                face.outline_glyph(ttf_parser::GlyphId(id), &mut builder);
            }
            if builder.segments > SEGMENTS {
                return None;
            }
            pen += f64::from(g.x_advance) * unit;
        }
    }
    shaped.advance = pen;
    shaped.path = builder.path.finish();
    if let Some(b) = shaped.path.as_ref().map(tiny_skia::Path::bounds) {
        shaped.ink = [-f64::from(b.left()), f64::from(b.right()), -f64::from(b.top()), f64::from(b.bottom())];
    }
    Some(shaped)
}
