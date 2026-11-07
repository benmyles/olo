use super::*;
use crate::css::style::{TextOrientation, WritingMode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fallbacks {
    pub ex_from_outline: bool,
    pub cap_from_outline: bool,
    pub ex_half_em: bool,
    pub cap_ascent: bool,
    pub ch_estimate: bool,
    pub ic_em: bool,
    pub synthesized_vertical_advance: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct ResolvedMetrics {
    pub style: FontMetrics,
    pub first_available: FaceId,
    pub zero_face: Option<FaceId>,
    pub ideograph_face: Option<FaceId>,
    pub ascent: f64,
    pub descent: f64,
    pub line_gap: f64,
    pub fallbacks: Fallbacks,
}
impl ResolvedMetrics {
    /// The style metrics for a device scale: Chrome rounds a face's ascent,
    /// descent and line gap to whole device pixels (Blink's SimpleFontData
    /// at the scaled size), so `line-height: normal` is that sum; the same
    /// here, or no page's lines land where Chrome's do. The other metrics
    /// stay fractional, as Chrome's do.
    pub fn style_at(&self, scale: f64) -> FontMetrics {
        let scale = if scale.is_finite() && scale > 0. { scale } else { 1. };
        let round = |v: f64| (v * scale + 0.5).floor() / scale;
        FontMetrics { normal_line_height: (round(self.ascent) + round(-self.descent) + round(self.line_gap)).max(0.), ..self.style }
    }
}
// Preserve fractional HVAR/VVAR deltas instead of ttf-parser's public u16
// convenience advances, which round before scaling to CSS pixels.
fn advance(face: &ttf_parser::Face<'_>, glyph: ttf_parser::GlyphId, vertical: bool, limit: usize) -> Option<f64> {
    let tables = face.tables();
    let base = if vertical { tables.vmtx?.advance(glyph)? } else { tables.hmtx?.advance(glyph)? } as f64;
    let (has_table, delta) = if vertical {
        (tables.vvar.is_some(), tables.vvar.and_then(|t| t.advance_offset(glyph, face.variation_coordinates())))
    } else {
        (tables.hvar.is_some(), tables.hvar.and_then(|t| t.advance_offset(glyph, face.variation_coordinates())))
    };
    if has_table {
        let delta = delta.unwrap_or(0.);
        return Some((base + f64::from(delta)).max(0.));
    }
    if face.is_variable() && tables.gvar.is_some() {
        bounded_simple_outline(face, glyph, limit)?;
        if let Some(points) = face.glyph_phantom_points(glyph) {
            return Some((base + f64::from(if vertical { points.bottom.y } else { points.right.x })).max(0.));
        }
    }
    Some(base)
}
fn delta(face: &ttf_parser::Face<'_>, tag: &[u8; 4]) -> f64 {
    face.tables().mvar.and_then(|t| t.metric_offset(ttf_parser::Tag::from_bytes(tag), face.variation_coordinates())).map_or(0., f64::from)
}
// Follow the table-selection policy of ttf-parser's hhea/OS2 accessors, while
// retaining fractional MVAR deltas through CSS-pixel scaling.
fn horizontal(face: &ttf_parser::Face<'_>) -> (f64, f64, f64) {
    let t = face.tables();
    let h = t.hhea;
    if let Some(o) = t.os2
        && o.use_typographic_metrics()
    {
        return (f64::from(o.typographic_ascender()) + delta(face, b"hasc"), f64::from(o.typographic_descender()) + delta(face, b"hdsc"), f64::from(o.typographic_line_gap()) + delta(face, b"hlgp"));
    }
    let (mut a, mut d, mut g) = (f64::from(h.ascender), f64::from(h.descender), f64::from(h.line_gap));
    if let Some(o) = t.os2 {
        if a == 0. {
            a = if o.typographic_ascender() != 0 { f64::from(o.typographic_ascender()) + delta(face, b"hasc") } else { f64::from(o.windows_ascender()) + delta(face, b"hcla") };
        }
        if d == 0. {
            d = if o.typographic_descender() != 0 { f64::from(o.typographic_descender()) + delta(face, b"hdsc") } else { f64::from(o.windows_descender()) + delta(face, b"hcld") };
        }
        if h.ascender == 0 || h.descender == 0 {
            g = if o.typographic_ascender() != 0 || o.typographic_descender() != 0 { f64::from(o.typographic_line_gap()) + delta(face, b"hlgp") } else { 0. };
        }
    }
    (a, d, g)
}
// Never enter a recursive composite/CFF outline interpreter merely to infer
// CSS metrics. A small composite can reference an exponential graph. Restrict
// this optional heuristic to simple TrueType glyphs and bounded gvar data.
fn bounded_simple_outline(face: &ttf_parser::Face<'_>, glyph: ttf_parser::GlyphId, limit: usize) -> Option<()> {
    let table = |tag| face.raw_face().table(ttf_parser::Tag::from_bytes(tag));
    let head = table(b"head")?;
    let loca = table(b"loca")?;
    let glyf = table(b"glyf")?;
    let u16_at = |bytes: &[u8], at: usize| Some(u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?));
    let offset = |index: usize| match u16_at(head, 50)? {
        0 => Some(usize::from(u16_at(loca, index.checked_mul(2)?)?) * 2),
        1 => {
            let at = index.checked_mul(4)?;
            usize::try_from(u32::from_be_bytes(loca.get(at..at.checked_add(4)?)?.try_into().ok()?)).ok()
        }
        _ => None,
    };
    let bytes = glyf.get(offset(usize::from(glyph.0))?..offset(usize::from(glyph.0) + 1)?)?;
    let variable_bytes = table(b"gvar").map_or(0, <[u8]>::len);
    if bytes.len().saturating_add(variable_bytes) > limit || bytes.len() < 10 {
        return None;
    }
    let contours = i16::from_be_bytes(bytes.get(..2)?.try_into().ok()?);
    if contours < 0 {
        return None;
    }
    // Fixed point cap bounds decoding even for highly compressed glyph data.
    if contours > 0 && (usize::from(u16_at(bytes, 8 + contours as usize * 2)?) + 1) > 4096 {
        return None;
    }
    Some(())
}
fn outline_height(face: &ttf_parser::Face<'_>, glyph: char, limit: usize) -> Option<f64> {
    let glyph = face.glyph_index(glyph)?;
    bounded_simple_outline(face, glyph, limit)?;
    let bbox = face.glyph_bounding_box(glyph)?;
    // Flat-topped x/H heuristics measured from the baseline, not descenders.
    (bbox.y_max > 0).then_some(f64::from(bbox.y_max))
}
impl Scope {
    pub(super) fn resolve_metrics(&self, request: &FontRequest, cancelled: Option<&AtomicBool>) -> Result<ResolvedMetrics> {
        let selected = self.first_available(request, cancelled)?;
        check(cancelled)?;
        let face = selected.parsed()?;
        let size = request.size.max(0.);
        let unit = size / f64::from(face.units_per_em());
        let (a, d, g) = horizontal(&face);
        let (ascent, descent, line_gap) = (a * unit, d * unit, g * unit);
        let mut fallback = Fallbacks::default();
        let x = face.tables().os2.and_then(|t| t.x_height()).map(|v| f64::from(v) + delta(&face, b"xhgt")).filter(|v| *v > 0.);
        let x = x.or_else(|| {
            let height = outline_height(&face, 'x', self.set.catalog.limits.outline_bytes);
            fallback.ex_from_outline = height.is_some();
            height
        });
        check(cancelled)?;
        let ex = x.map_or_else(
            || {
                fallback.ex_half_em = true;
                size * 0.5
            },
            |v| v * unit,
        );
        let cap = face.tables().os2.and_then(|t| t.capital_height()).map(|v| f64::from(v) + delta(&face, b"cpht")).filter(|v| *v > 0.);
        let cap = cap.or_else(|| {
            let height = outline_height(&face, 'H', self.set.catalog.limits.outline_bytes);
            fallback.cap_from_outline = height.is_some();
            height
        });
        let cap = cap.map_or_else(
            || {
                fallback.cap_ascent = true;
                ascent.max(0.)
            },
            |v| v * unit,
        );
        check(cancelled)?;
        let zero = self.cmap_selection(request, '0', cancelled)?;
        let water = self.cmap_selection(request, '水', cancelled)?;
        let vertical = matches!(request.axes.writing_mode, WritingMode::VerticalRl | WritingMode::VerticalLr);
        let zero_vertical = vertical && request.axes.text_orientation == TextOrientation::Upright;
        let water_vertical = vertical && request.axes.text_orientation != TextOrientation::Sideways;
        let mut reference = |selection: Option<&Selection>, c: char, vertical: bool, default: f64| -> Result<Option<f64>> {
            let Some(selection) = selection else { return Ok(None) };
            check(cancelled)?;
            let other;
            let glyph_face = if selection.face == selected.face {
                &face
            } else {
                other = selection.parsed()?;
                &other
            };
            let Some(glyph) = glyph_face.glyph_index(c).filter(|g| g.0 != 0) else { return Ok(None) };
            if vertical && glyph_face.tables().vmtx.is_none() {
                fallback.synthesized_vertical_advance = true;
                return Ok(Some(default));
            }
            Ok(advance(glyph_face, glyph, vertical, self.set.catalog.limits.outline_bytes).map(|v| v * size / f64::from(glyph_face.units_per_em())))
        };
        let ch = reference(zero.as_ref(), '0', zero_vertical, size)?;
        let ic = reference(water.as_ref(), '水', water_vertical, size)?;
        let ch = ch.unwrap_or_else(|| {
            fallback.ch_estimate = true;
            size * if zero_vertical { 1. } else { 0.5 }
        });
        let ic = ic.unwrap_or_else(|| {
            fallback.ic_em = true;
            size
        });
        let line_height = (ascent - descent + line_gap).max(0.);
        if ![ex, cap, ch, ic, line_height, ascent, descent, line_gap].into_iter().all(f64::is_finite) {
            return Err(Error::Invalid);
        }
        check(cancelled)?;
        Ok(ResolvedMetrics {
            style: FontMetrics { id: selected.face.value(), ex, cap, ch, ic, normal_line_height: line_height },
            first_available: selected.face,
            zero_face: zero.as_ref().map(|s| s.face),
            ideograph_face: water.as_ref().map(|s| s.face),
            ascent,
            descent,
            line_gap,
            fallbacks: fallback,
        })
    }
}
