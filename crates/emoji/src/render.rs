//! Drawing colour glyphs (the `render` feature).

use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    color::{Brush, ColorGlyphCollection, ColorPainter, ColorStop, CompositeMode, Extend, Transform as FontTransform},
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::types::BoundingBox,
};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode, SweepGradient, Transform,
};

/// A colour font, loaded once and kept for the life of the process.
pub struct ColorFont {
    data: &'static [u8],
    index: u32,
    shaper: harfrust::ShaperData,
}

/// A rendered glyph: premultiplied RGBA, row by row.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ColorFont {
    /// The font in `data` (face `index`), if it has colour glyphs.
    pub fn new(data: Vec<u8>, index: u32) -> Option<ColorFont> {
        ColorFont::from_static(Box::leak(data.into_boxed_slice()), index)
    }

    /// The same for data that lives for the process (a mapped file).
    pub fn from_static(data: &'static [u8], index: u32) -> Option<ColorFont> {
        let font = FontRef::from_index(data, index).ok()?;
        if !has_colr(&font) {
            return None;
        }
        let shaper = harfrust::ShaperData::new(&harfrust::FontRef::from_index(data, index).ok()?);
        Some(ColorFont { data, index, shaper })
    }

    fn font(&self) -> FontRef<'static> {
        FontRef::from_index(self.data, self.index).expect("checked in new")
    }

    /// The colour glyph for a whole cluster ("👍🏽", "👩‍💻", "🇳🇴", "1️⃣"),
    /// or None if the font draws it as more than one glyph or not in colour.
    pub fn glyph(&self, cluster: &str) -> Option<u32> {
        let font = harfrust::FontRef::from_index(self.data, self.index).ok()?;
        let shaper = self.shaper.shaper(&font).build();
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(cluster);
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
        // Variation selectors left over are drawn as nothing.
        let glyphs: Vec<u32> = shaped.glyph_infos().iter().map(|g| g.glyph_id).filter(|&g| g != 0 || cluster.is_empty()).collect();
        let colors = ColorGlyphCollection::new(&self.font());
        let mut coloured = glyphs.iter().copied().filter(|&g| colors.get(GlyphId::new(g)).is_some());
        let first = coloured.next()?;
        let rest = glyphs.iter().filter(|&&g| g != first && !self.blank(g)).count();
        (rest == 0).then_some(first)
    }

    /// A glyph that draws nothing (variation selectors, ZWJ).
    fn blank(&self, glyph: u32) -> bool {
        let font = self.font();
        let mut bounds = Bounds::default();
        match font.outline_glyphs().get(GlyphId::new(glyph)) {
            Some(o) => {
                let _ = o.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut bounds);
                !bounds.any
            }
            None => true,
        }
    }

    /// Width over height of `glyph`'s box, as `render` draws it.
    pub fn aspect(&self, glyph: u32) -> f32 {
        let font = self.font();
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        let span = metrics.ascent - metrics.descent;
        let advance = font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(GlyphId::new(glyph)).unwrap_or(span);
        if span > 0.0 { advance / span } else { 1.0 }
    }

    /// `glyph` drawn `height` pixels tall (the font's ascent to descent),
    /// its advance wide. `fg` is the text colour, for glyphs that use it.
    pub fn render(&self, glyph: u32, height: u32, fg: [u8; 4]) -> Option<Image> {
        let font = self.font();
        let colors = ColorGlyphCollection::new(&font);
        let color = colors.get(GlyphId::new(glyph))?;
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        let span = metrics.ascent - metrics.descent;
        if span <= 0.0 || height == 0 {
            return None;
        }
        let scale = height as f32 / span;
        let advance = font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(GlyphId::new(glyph)).unwrap_or(span);
        let width = (advance * scale).round().max(1.0) as u32;
        let palette: Vec<Color> = font.color_palettes().get(0).map(|p| p.colors().iter().map(|c| Color::from_rgba8(c.red, c.green, c.blue, c.alpha)).collect()).unwrap_or_default();
        let mut painter = Painter {
            font: font.clone(),
            palette,
            fg: Color::from_rgba8(fg[0], fg[1], fg[2], fg[3]),
            layers: vec![Pixmap::new(width, height)?],
            transforms: vec![Transform::from_row(scale, 0.0, 0.0, -scale, 0.0, metrics.ascent * scale)],
            clips: Vec::new(),
        };
        color.paint(LocationRef::default(), &mut painter).ok()?;
        let pixmap = painter.layers.swap_remove(0);
        Some(Image { width, height, rgba: pixmap.take() })
    }
}

fn has_colr(font: &FontRef) -> bool {
    use skrifa::raw::TableProvider;
    font.colr().is_ok()
}

#[derive(Default)]
struct Bounds {
    any: bool,
}

impl OutlinePen for Bounds {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {
        self.any = true;
    }
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.any = true;
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.any = true;
    }
    fn close(&mut self) {}
}

struct PathPen(PathBuilder);

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(cx0, cy0, x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// skrifa's paint callbacks on tiny-skia: a stack of layers (pixmaps)
/// merged with their composite mode, of transforms (font units to pixels
/// at the bottom) and of clip masks.
struct Painter<'a> {
    font: FontRef<'a>,
    palette: Vec<Color>,
    fg: Color,
    layers: Vec<Pixmap>,
    transforms: Vec<Transform>,
    clips: Vec<Mask>,
}

impl Painter<'_> {
    fn transform(&self) -> Transform {
        *self.transforms.last().expect("base transform")
    }

    fn size(&self) -> (u32, u32) {
        let p = &self.layers[0];
        (p.width(), p.height())
    }

    fn color(&self, index: u16, alpha: f32) -> Color {
        let mut c = if index == 0xFFFF { self.fg } else { self.palette.get(index as usize).copied().unwrap_or(Color::BLACK) };
        c.apply_opacity(alpha);
        c
    }

    fn stops(&self, stops: &[ColorStop]) -> Vec<GradientStop> {
        stops.iter().map(|s| GradientStop::new(s.offset, self.color(s.palette_index, s.alpha))).collect()
    }

    fn push_clip(&mut self, path: Option<Path>) {
        let (w, h) = self.size();
        let t = self.transform();
        let mask = match (self.clips.last(), path) {
            (Some(top), Some(path)) => {
                let mut m = top.clone();
                m.intersect_path(&path, FillRule::Winding, true, t);
                m
            }
            (None, Some(path)) => {
                let mut m = Mask::new(w, h).expect("non-empty");
                m.fill_path(&path, FillRule::Winding, true, t);
                m
            }
            // Nothing to draw: clip everything.
            (_, None) => Mask::new(w, h).expect("non-empty"),
        };
        self.clips.push(mask);
    }

    fn shader(&self, brush: Brush<'_>) -> Option<Shader<'static>> {
        let t = self.transform();
        let solid_or = |stops: &[ColorStop]| match stops {
            [] => None,
            [only] => Some(Shader::SolidColor(self.color(only.palette_index, only.alpha))),
            _ => Some(Shader::SolidColor(Color::TRANSPARENT)),
        };
        match brush {
            Brush::Solid { palette_index, alpha } => Some(Shader::SolidColor(self.color(palette_index, alpha))),
            Brush::LinearGradient { p0, p1, color_stops, extend } => {
                if color_stops.len() < 2 {
                    return solid_or(color_stops);
                }
                LinearGradient::new(Point::from_xy(p0.x, p0.y), Point::from_xy(p1.x, p1.y), self.stops(color_stops), spread(extend), t)
            }
            Brush::RadialGradient { c0, r0, c1, r1, color_stops, extend } => {
                if color_stops.len() < 2 {
                    return solid_or(color_stops);
                }
                let (c0, r0, c1, r1, stops) = clamp_radii((c0.x, c0.y), r0, (c1.x, c1.y), r1, color_stops);
                let stops: Vec<GradientStop> = stops.iter().map(|&(o, c)| GradientStop::new(o, c.resolve(self))).collect();
                RadialGradient::new(Point::from_xy(c0.0, c0.1), r0, Point::from_xy(c1.0, c1.1), r1, stops, spread(extend), t)
            }
            Brush::SweepGradient { c0, start_angle, end_angle, color_stops, extend } => {
                if color_stops.len() < 2 {
                    return solid_or(color_stops);
                }
                SweepGradient::new(Point::from_xy(c0.x, c0.y), start_angle, end_angle, self.stops(color_stops), spread(extend), t)
            }
        }
    }
}

/// A stop's colour, by palette entry or already interpolated.
#[derive(Clone, Copy)]
enum StopColor {
    Palette(u16, f32),
    Mixed(Color),
}

impl StopColor {
    fn resolve(self, p: &Painter) -> Color {
        match self {
            StopColor::Palette(i, a) => p.color(i, a),
            StopColor::Mixed(c) => c,
        }
    }
}

type Circle = ((f32, f32), f32);
/// Two circles and the stops between them.
type Radial = ((f32, f32), f32, (f32, f32), f32, Vec<(f32, StopColor)>);

/// skrifa normalises stops to 0–1, which can leave a negative radius at
/// one end; the colour line is cut where the radius reaches zero.
fn clamp_radii(c0: (f32, f32), r0: f32, c1: (f32, f32), r1: f32, stops: &[ColorStop]) -> Radial {
    let mut out: Vec<(f32, StopColor)> = stops.iter().map(|s| (s.offset, StopColor::Palette(s.palette_index, s.alpha))).collect();
    if r0 >= 0.0 && r1 >= 0.0 {
        return (c0, r0, c1, r1, out);
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    // Where the radius is zero along the line from circle 0 to circle 1.
    let t = r0 / (r0 - r1);
    let at = (lerp(c0.0, c1.0, t), lerp(c0.1, c1.1, t));
    let (keep_from, (a, b)): (bool, (Circle, Circle)) = if r0 < 0.0 { (true, ((at, 0.0), (c1, r1))) } else { (false, ((c0, r0), (at, 0.0))) };
    // Stops are resolved later; keep palette stops and remap offsets.
    out =
        if keep_from { out.into_iter().filter(|&(o, _)| o >= t).map(|(o, c)| ((o - t) / (1.0 - t), c)).collect() } else { out.into_iter().filter(|&(o, _)| o <= t).map(|(o, c)| (o / t, c)).collect() };
    if out.is_empty() {
        out.push((0.0, StopColor::Mixed(Color::TRANSPARENT)));
    }
    (a.0, a.1, b.0, b.1, out)
}

fn spread(e: Extend) -> SpreadMode {
    match e {
        Extend::Repeat => SpreadMode::Repeat,
        Extend::Reflect => SpreadMode::Reflect,
        _ => SpreadMode::Pad,
    }
}

fn blend(mode: CompositeMode) -> BlendMode {
    use CompositeMode as C;
    match mode {
        C::Clear => BlendMode::Clear,
        C::Src => BlendMode::Source,
        C::Dest => BlendMode::Destination,
        C::DestOver => BlendMode::DestinationOver,
        C::SrcIn => BlendMode::SourceIn,
        C::DestIn => BlendMode::DestinationIn,
        C::SrcOut => BlendMode::SourceOut,
        C::DestOut => BlendMode::DestinationOut,
        C::SrcAtop => BlendMode::SourceAtop,
        C::DestAtop => BlendMode::DestinationAtop,
        C::Xor => BlendMode::Xor,
        C::Plus => BlendMode::Plus,
        C::Screen => BlendMode::Screen,
        C::Overlay => BlendMode::Overlay,
        C::Darken => BlendMode::Darken,
        C::Lighten => BlendMode::Lighten,
        C::ColorDodge => BlendMode::ColorDodge,
        C::ColorBurn => BlendMode::ColorBurn,
        C::HardLight => BlendMode::HardLight,
        C::SoftLight => BlendMode::SoftLight,
        C::Difference => BlendMode::Difference,
        C::Exclusion => BlendMode::Exclusion,
        C::Multiply => BlendMode::Multiply,
        C::HslHue => BlendMode::Hue,
        C::HslSaturation => BlendMode::Saturation,
        C::HslColor => BlendMode::Color,
        C::HslLuminosity => BlendMode::Luminosity,
        _ => BlendMode::SourceOver,
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, t: FontTransform) {
        let next = self.transform().pre_concat(Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy));
        self.transforms.push(next);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        let mut pen = PathPen(PathBuilder::new());
        if let Some(outline) = self.font.outline_glyphs().get(glyph) {
            let _ = outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
        }
        self.push_clip(pen.0.finish());
    }

    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        let rect = Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max);
        self.push_clip(rect.map(PathBuilder::from_rect));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let Some(shader) = self.shader(brush) else { return };
        let (w, h) = self.size();
        let paint = Paint { shader, anti_alias: true, ..Default::default() };
        let mask = self.clips.last().cloned();
        let layer = self.layers.last_mut().expect("base layer");
        if let Some(r) = Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
            layer.fill_rect(r, &paint, Transform::identity(), mask.as_ref());
        }
    }

    fn push_layer(&mut self, _mode: CompositeMode) {
        let (w, h) = self.size();
        self.layers.push(Pixmap::new(w, h).expect("non-empty"));
    }

    fn pop_layer_with_mode(&mut self, mode: CompositeMode) {
        if self.layers.len() < 2 {
            return;
        }
        let top = self.layers.pop().expect("checked");
        let below = self.layers.last_mut().expect("checked");
        let paint = PixmapPaint { blend_mode: blend(mode), ..Default::default() };
        below.draw_pixmap(0, 0, top.as_ref(), &paint, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTO: &str = "/usr/share/fonts/google-noto-color-emoji-fonts/Noto-COLRv1.ttf";

    fn noto() -> Option<ColorFont> {
        ColorFont::new(std::fs::read(NOTO).ok()?, 0)
    }

    fn save(img: &Image, name: &str) {
        let Ok(dir) = std::env::var("EMOJI_OUT") else { return };
        let f = std::fs::File::create(format!("{dir}/{name}.png")).unwrap();
        let mut enc = png::Encoder::new(f, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // Unpremultiply for PNG.
        let mut px = img.rgba.clone();
        for p in px.chunks_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }

    #[test]
    fn radii_clamped_at_zero() {
        let stops = [ColorStop { offset: 0.0, palette_index: 0, alpha: 1.0 }, ColorStop { offset: 0.5, palette_index: 1, alpha: 1.0 }, ColorStop { offset: 1.0, palette_index: 2, alpha: 1.0 }];
        let (c0, r0, _, r1, s) = clamp_radii((0.0, 0.0), -10.0, (10.0, 0.0), 10.0, &stops);
        assert_eq!((c0, r0, r1), ((5.0, 0.0), 0.0, 10.0));
        let offsets: Vec<f32> = s.iter().map(|s| s.0).collect();
        assert_eq!(offsets, [0.0, 1.0]);
    }

    #[test]
    fn single_and_sequences() {
        let Some(font) = noto() else { return };
        for (name, text) in [("grin", "😀"), ("thumbs", "👍🏽"), ("technologist", "👩‍💻"), ("flag", "🇳🇴"), ("keycap", "1️⃣"), ("heart", "❤️"), ("rainbow", "🌈"), ("fire", "🔥")]
        {
            let g = font.glyph(text).unwrap_or_else(|| panic!("{name}: one colour glyph"));
            let img = font.render(g, 64, [0, 0, 0, 255]).expect("rendered");
            assert!(img.width > 40 && img.width < 120, "{name}: {}", img.width);
            let opaque = img.rgba.chunks(4).filter(|p| p[3] > 200).count();
            assert!(opaque > (img.width * img.height / 5) as usize, "{name} is mostly painted ({opaque})");
            save(&img, name);
        }
        // Plain letters aren't colour glyphs.
        assert!(font.glyph("a").is_none());
        // A ZWJ sequence's parts are distinct from the whole.
        assert_ne!(font.glyph("👩‍💻"), font.glyph("👩"));
    }
}

// ---- finding emoji in text -------------------------------------------------
