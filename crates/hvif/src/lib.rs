//! The Haiku Vector Icon Format: a compact vector format of styles
//! (colours and gradients), paths and shapes, drawn in a 64 × 64 space.
//!
//! [`Icon::parse`] reads the "ncif" container the way Haiku's
//! FlatIconImporter does; [`Icon::render`] draws it at any size the way
//! Haiku's IconRenderer does: shapes in order, non-zero fill, colours
//! blended in a 2.2 gamma space, shapes hidden outside their level-of-
//! detail range. [`cursor`] renders Haiku's system cursors.
//!
//! The format and the rendering rules follow Haiku's libicon (MIT
//! licensed, Copyright 2006-2007 Haiku, Stephan Aßmus).

pub mod cursor;

use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Path, PathBuilder, Pixmap, Stroke, Transform};

/// Why an icon could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an HVIF ("ncif") file.
    Magic,
    /// The data ends in the middle of something.
    Truncated,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Magic => f.write_str("not a Haiku vector icon"),
            Error::Truncated => f.write_str("truncated Haiku vector icon"),
        }
    }
}

impl std::error::Error for Error {}

/// A colour as stored: straight (not premultiplied) sRGB with alpha.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientKind {
    Linear,
    Radial,
    Diamond,
    Conic,
    Xy,
    SqrtXy,
}

#[derive(Debug, Clone)]
pub struct Gradient {
    pub kind: GradientKind,
    /// From gradient space to shape space.
    pub transform: Transform,
    /// (offset 0..=1, colour), in file order.
    pub stops: Vec<(f32, Rgba)>,
}

#[derive(Debug, Clone)]
pub enum Style {
    Solid(Rgba),
    Gradient(Gradient),
}

/// One point of a path: the vertex and its incoming and outgoing
/// control points (equal to the vertex on straight segments).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub p: (f32, f32),
    pub cin: (f32, f32),
    pub cout: (f32, f32),
}

#[derive(Debug, Clone)]
pub struct IconPath {
    pub points: Vec<Vertex>,
    pub closed: bool,
}

#[derive(Debug, Clone)]
pub enum Transformer {
    Affine(Transform),
    /// Grows (positive width) or shrinks the outline by half the width.
    Contour {
        width: f32,
        join: u8,
        miter: f32,
    },
    /// Haiku's perspective transformer; read but drawn without it.
    Perspective([f32; 9]),
    Stroke {
        width: f32,
        join: u8,
        cap: u8,
        miter: f32,
    },
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub style: usize,
    pub paths: Vec<usize>,
    pub transform: Transform,
    pub hinting: bool,
    pub min_scale: f32,
    pub max_scale: f32,
    pub transformers: Vec<Transformer>,
}

impl Shape {
    /// Haiku's PathSourceShape::Visible.
    pub fn visible(&self, scale: f32) -> bool {
        scale >= self.min_scale && (scale <= self.max_scale || self.max_scale >= 4.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Icon {
    pub styles: Vec<Style>,
    pub paths: Vec<IconPath>,
    pub shapes: Vec<Shape>,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, Error> {
        let b = *self.data.get(self.at).ok_or(Error::Truncated)?;
        self.at += 1;
        Ok(b)
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self.data.get(self.at..self.at + N).ok_or(Error::Truncated)?;
        self.at += N;
        Ok(s.try_into().expect("length checked"))
    }

    fn skip_tag(&mut self) -> Result<(), Error> {
        let len = u16::from_le_bytes(self.bytes()?) as usize;
        if self.at + len > self.data.len() {
            return Err(Error::Truncated);
        }
        self.at += len;
        Ok(())
    }

    fn coord(&mut self) -> Result<f32, Error> {
        let b = self.u8()?;
        if b & 128 != 0 {
            let lo = self.u8()?;
            Ok((((b as u16 & 127) << 8) | lo as u16) as f32 / 102.0 - 128.0)
        } else {
            Ok(b as f32 - 32.0)
        }
    }

    fn point(&mut self) -> Result<(f32, f32), Error> {
        Ok((self.coord()?, self.coord()?))
    }

    fn float24(&mut self) -> Result<f32, Error> {
        let [a, b, c] = self.bytes()?;
        let v = (a as u32) << 16 | (b as u32) << 8 | c as u32;
        if v == 0 {
            return Ok(0.0);
        }
        let sign = (v & 0x80_0000) >> 23;
        let exponent = ((v & 0x7e_0000) >> 17) as i32 - 32;
        let mantissa = (v & 0x1_ffff) << 6;
        Ok(f32::from_bits(sign << 31 | ((exponent + 127) as u32) << 23 | mantissa))
    }

    fn matrix(&mut self) -> Result<Transform, Error> {
        let mut m = [0.0; 6];
        for v in &mut m {
            *v = self.float24()?;
        }
        // agg's order: sx, shy, shx, sy, tx, ty.
        Ok(Transform::from_row(m[0], m[1], m[2], m[3], m[4], m[5]))
    }

    fn color(&mut self, alpha: bool, gray: bool) -> Result<Rgba, Error> {
        Ok(match (alpha, gray) {
            (true, false) => self.bytes()?,
            (false, false) => {
                let [r, g, b] = self.bytes()?;
                [r, g, b, 255]
            }
            (true, true) => {
                let [v, a] = self.bytes()?;
                [v, v, v, a]
            }
            (false, true) => {
                let v = self.u8()?;
                [v, v, v, 255]
            }
        })
    }
}

impl Icon {
    pub fn parse(data: &[u8]) -> Result<Icon, Error> {
        if !data.starts_with(b"ncif") {
            return Err(Error::Magic);
        }
        let mut r = Reader { data, at: 4 };
        let mut icon = Icon::default();

        for _ in 0..r.u8()? {
            let style = match r.u8()? {
                1 => Style::Solid(r.color(true, false)?),
                3 => Style::Solid(r.color(false, false)?),
                4 => Style::Solid(r.color(true, true)?),
                5 => Style::Solid(r.color(false, true)?),
                2 => {
                    let kind = match r.u8()? {
                        1 => GradientKind::Radial,
                        2 => GradientKind::Diamond,
                        3 => GradientKind::Conic,
                        4 => GradientKind::Xy,
                        5 => GradientKind::SqrtXy,
                        _ => GradientKind::Linear,
                    };
                    let flags = r.u8()?;
                    let count = r.u8()?;
                    let transform = if flags & 1 << 1 != 0 { r.matrix()? } else { Transform::identity() };
                    let (alpha, gray) = (flags & 1 << 2 == 0, flags & 1 << 4 != 0);
                    let mut stops = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let offset = r.u8()? as f32 / 255.0;
                        stops.push((offset, r.color(alpha, gray)?));
                    }
                    Style::Gradient(Gradient { kind, transform, stops })
                }
                _ => {
                    r.skip_tag()?;
                    continue;
                }
            };
            icon.styles.push(style);
        }

        for _ in 0..r.u8()? {
            let flags = r.u8()?;
            let count = r.u8()? as usize;
            let mut points = Vec::with_capacity(count);
            let line = |p| Vertex { p, cin: p, cout: p };
            if flags & 1 << 3 != 0 {
                for _ in 0..count {
                    points.push(line(r.point()?));
                }
            } else if flags & 1 << 2 != 0 {
                let commands: Vec<u8> = (0..count.div_ceil(4)).map(|_| r.u8()).collect::<Result<_, _>>()?;
                let mut last = (0.0, 0.0);
                for i in 0..count {
                    let v = match commands[i / 4] >> ((i % 4) * 2) & 3 {
                        0 => line((r.coord()?, last.1)),
                        1 => line((last.0, r.coord()?)),
                        2 => line(r.point()?),
                        _ => Vertex { p: r.point()?, cin: r.point()?, cout: r.point()? },
                    };
                    last = v.p;
                    points.push(v);
                }
            } else {
                for _ in 0..count {
                    points.push(Vertex { p: r.point()?, cin: r.point()?, cout: r.point()? });
                }
            }
            icon.paths.push(IconPath { points, closed: flags & 1 << 1 != 0 });
        }

        for _ in 0..r.u8()? {
            if r.u8()? != 10 {
                r.skip_tag()?;
                continue;
            }
            let style = r.u8()? as usize;
            let paths = (0..r.u8()?).map(|_| r.u8().map(usize::from)).collect::<Result<_, _>>()?;
            let flags = r.u8()?;
            let mut shape = Shape { style, paths, transform: Transform::identity(), hinting: flags & 1 << 2 != 0, min_scale: 0.0, max_scale: 4.0, transformers: Vec::new() };
            if flags & 1 << 1 != 0 {
                shape.transform = r.matrix()?;
            } else if flags & 1 << 5 != 0 {
                let (x, y) = r.point()?;
                shape.transform = Transform::from_translate(x, y);
            }
            if flags & 1 << 3 != 0 {
                shape.min_scale = r.u8()? as f32 / 63.75;
                shape.max_scale = r.u8()? as f32 / 63.75;
            }
            if flags & 1 << 4 != 0 {
                for _ in 0..r.u8()? {
                    let t = match r.u8()? {
                        20 => {
                            let mut m = [0.0; 6];
                            for v in &mut m {
                                *v = f32::from_le_bytes(r.bytes()?);
                            }
                            Transformer::Affine(Transform::from_row(m[0], m[1], m[2], m[3], m[4], m[5]))
                        }
                        21 => {
                            let [w, join, miter] = r.bytes()?;
                            Transformer::Contour { width: w as f32 - 128.0, join, miter: miter as f32 }
                        }
                        22 => {
                            let mut m = [0.0; 9];
                            for v in &mut m {
                                *v = r.float24()?;
                            }
                            Transformer::Perspective(m)
                        }
                        23 => {
                            let [w, options, miter] = r.bytes()?;
                            Transformer::Stroke { width: w as f32 - 128.0, join: options & 15, cap: options >> 4, miter: miter as f32 }
                        }
                        _ => {
                            r.skip_tag()?;
                            continue;
                        }
                    };
                    shape.transformers.push(t);
                }
            }
            icon.shapes.push(shape);
        }
        Ok(icon)
    }

    /// Apply `t` (in icon space) after every shape's own transform.
    pub fn transform(&mut self, t: Transform) {
        for shape in &mut self.shapes {
            shape.transform = shape.transform.post_concat(t);
        }
    }

    /// Draw the icon into a new `size` × `size` pixmap (premultiplied
    /// RGBA, as tiny-skia keeps it).
    pub fn render(&self, size: u32) -> Pixmap {
        let mut pixmap = Pixmap::new(size.max(1), size.max(1)).expect("non-zero size");
        self.render_into(&mut pixmap, size as f32 / 64.0);
        pixmap
    }

    /// Draw the icon over a pixmap's contents, icon space scaled by
    /// `scale` (64 × 64 at 1.0).
    pub fn render_into(&self, pixmap: &mut Pixmap, scale: f32) {
        let (w, h) = (pixmap.width(), pixmap.height());
        // Linear-light premultiplied colour, so edges blend as Haiku's do.
        let mut buf: Vec<[f32; 4]> = pixmap
            .pixels()
            .iter()
            .map(|p| {
                let a = p.alpha() as f32 / 255.0;
                let lin = |c: u8| if a > 0.0 { to_linear(c as f32 / 255.0 / a) * a } else { 0.0 };
                [lin(p.red()), lin(p.green()), lin(p.blue()), a]
            })
            .collect();
        let global = Transform::from_scale(scale, scale);

        for shape in &self.shapes {
            if !shape.visible(scale) {
                continue;
            }
            let Some(style) = self.styles.get(shape.style) else { continue };
            let Some(coverage) = self.coverage(shape, global, w, h) else { continue };
            let transform = shape.transform.post_concat(global);
            let colour = Colour::new(style, transform);
            for (i, c) in coverage.data().iter().enumerate() {
                if *c == 0 {
                    continue;
                }
                let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
                let [r, g, b, a] = colour.at(x, y);
                let a = a * *c as f32 / 255.0;
                let d = &mut buf[i];
                let k = 1.0 - a;
                *d = [r * a + d[0] * k, g * a + d[1] * k, b * a + d[2] * k, a + d[3] * k];
            }
        }

        for (p, d) in pixmap.pixels_mut().iter_mut().zip(buf) {
            let a = d[3].clamp(0.0, 1.0);
            let srgb = |c: f32| if a > 0.0 { (from_linear((c / a).clamp(0.0, 1.0)) * a * 255.0).round() as u8 } else { 0 };
            let a8 = (a * 255.0).round() as u8;
            *p = tiny_skia::PremultipliedColorU8::from_rgba(srgb(d[0]).min(a8), srgb(d[1]).min(a8), srgb(d[2]).min(a8), a8).expect("premultiplied");
        }
    }

    /// The shape's paths, through its transformers, as pixel coverage.
    fn coverage(&self, shape: &Shape, global: Transform, w: u32, h: u32) -> Option<Mask> {
        let mut pb = PathBuilder::new();
        for path in shape.paths.iter().filter_map(|&i| self.paths.get(i)) {
            add_path(&mut pb, path);
        }
        let mut geometry = Geometry::Fill(pb.finish()?);
        let res = global.get_scale().0.max(1.0);
        for t in &shape.transformers {
            geometry = match (geometry, t) {
                (g, Transformer::Affine(m)) => g.transform(*m)?,
                (g, Transformer::Perspective(_)) => g,
                (Geometry::Fill(p), Transformer::Contour { width, join, miter }) => {
                    let outline = p.stroke(&stroke(width.abs(), *join, 0, *miter), res)?;
                    Geometry::Contour { base: p, outline, grow: *width > 0.0 }
                }
                (Geometry::Fill(p), Transformer::Stroke { width, join, cap, miter }) => Geometry::Fill(p.stroke(&stroke(width.abs(), *join, *cap, *miter), res)?),
                // Haiku chains these too; icons don't.
                (g, _) => g,
            };
        }
        let transform = shape.transform.post_concat(global);
        let place = |p: Path| -> Option<Path> {
            let p = p.transform(transform)?;
            if shape.hinting { hint(&p) } else { Some(p) }
        };
        let fill = |p: &Path| {
            let mut m = Mask::new(w, h)?;
            m.fill_path(p, FillRule::Winding, true, Transform::identity());
            Some(m)
        };
        match geometry {
            Geometry::Fill(p) => fill(&place(p)?),
            Geometry::Contour { base, outline, grow } => {
                let mut base = fill(&place(base)?)?;
                let outline = fill(&place(outline)?)?;
                for (b, o) in base.data_mut().iter_mut().zip(outline.data()) {
                    let (bv, ov) = (*b as u32, *o as u32);
                    *b = if grow { (bv + ov - bv * ov / 255).min(255) } else { bv * (255 - ov) / 255 } as u8;
                }
                Some(base)
            }
        }
    }
}

enum Geometry {
    Fill(Path),
    /// A contour transformer's result: the fill grown or shrunk by its
    /// outline (a stroke as wide as the contour).
    Contour {
        base: Path,
        outline: Path,
        grow: bool,
    },
}

impl Geometry {
    fn transform(self, m: Transform) -> Option<Geometry> {
        Some(match self {
            Geometry::Fill(p) => Geometry::Fill(p.transform(m)?),
            Geometry::Contour { base, outline, grow } => Geometry::Contour { base: base.transform(m)?, outline: outline.transform(m)?, grow },
        })
    }
}

fn stroke(width: f32, join: u8, cap: u8, miter: f32) -> Stroke {
    Stroke {
        width,
        miter_limit: miter.max(1.0),
        // agg: miter, miter revert, round, bevel, miter round.
        line_join: match join {
            2 => LineJoin::Round,
            3 => LineJoin::Bevel,
            4 => LineJoin::MiterClip,
            _ => LineJoin::Miter,
        },
        // agg: butt, square, round.
        line_cap: match cap {
            1 => LineCap::Square,
            2 => LineCap::Round,
            _ => LineCap::Butt,
        },
        dash: None,
    }
}

/// One HVIF path as cubic segments from each vertex's outgoing control
/// point to the next one's incoming control point.
fn add_path(pb: &mut PathBuilder, path: &IconPath) {
    let Some(first) = path.points.first() else { return };
    pb.move_to(first.p.0, first.p.1);
    let segment = |pb: &mut PathBuilder, a: &Vertex, b: &Vertex| {
        if a.cout == a.p && b.cin == b.p {
            pb.line_to(b.p.0, b.p.1);
        } else {
            pb.cubic_to(a.cout.0, a.cout.1, b.cin.0, b.cin.1, b.p.0, b.p.1);
        }
    };
    for pair in path.points.windows(2) {
        segment(pb, &pair[0], &pair[1]);
    }
    if path.closed {
        if path.points.len() > 1 {
            segment(pb, path.points.last().expect("non-empty"), first);
        }
        pb.close();
    }
}

/// Haiku's HintingTransformer: every vertex onto a whole pixel.
fn hint(path: &Path) -> Option<Path> {
    let r = |p: tiny_skia::Point| ((p.x + 0.5).floor(), (p.y + 0.5).floor());
    let mut pb = PathBuilder::new();
    for seg in path.segments() {
        match seg {
            tiny_skia::PathSegment::MoveTo(p) => {
                let (x, y) = r(p);
                pb.move_to(x, y)
            }
            tiny_skia::PathSegment::LineTo(p) => {
                let (x, y) = r(p);
                pb.line_to(x, y)
            }
            tiny_skia::PathSegment::QuadTo(a, p) => {
                let ((ax, ay), (x, y)) = (r(a), r(p));
                pb.quad_to(ax, ay, x, y)
            }
            tiny_skia::PathSegment::CubicTo(a, b, p) => {
                let ((ax, ay), (bx, by), (x, y)) = (r(a), r(b), r(p));
                pb.cubic_to(ax, ay, bx, by, x, y)
            }
            tiny_skia::PathSegment::Close => pb.close(),
        }
    }
    pb.finish()
}

const GAMMA: f32 = 2.2;

fn to_linear(c: f32) -> f32 {
    c.powf(GAMMA)
}

fn from_linear(c: f32) -> f32 {
    c.powf(1.0 / GAMMA)
}

/// A style ready to sample at device pixels.
enum Colour {
    Solid([f32; 4]),
    Gradient { kind: GradientKind, inverse: Transform, table: Box<[[f32; 4]; 256]> },
}

impl Colour {
    fn new(style: &Style, shape: Transform) -> Colour {
        let lin = |c: Rgba| [to_linear(c[0] as f32 / 255.0), to_linear(c[1] as f32 / 255.0), to_linear(c[2] as f32 / 255.0), c[3] as f32 / 255.0];
        match style {
            Style::Solid(c) => Colour::Solid(lin(*c)),
            Style::Gradient(g) => {
                let inverse = g.transform.post_concat(shape).invert().unwrap_or_default();
                let table = gradient_table(&g.stops).map(lin);
                Colour::Gradient { kind: g.kind, inverse, table: Box::new(table) }
            }
        }
    }

    /// Linear colour, straight alpha.
    fn at(&self, x: f32, y: f32) -> [f32; 4] {
        match self {
            Colour::Solid(c) => *c,
            Colour::Gradient { kind, inverse, table } => {
                let mut p = [tiny_skia::Point::from_xy(x, y)];
                inverse.map_points(&mut p);
                let (gx, gy) = (p[0].x, p[0].y);
                // agg's gradient functions, over their ranges.
                let (d, start, end) = match kind {
                    GradientKind::Linear => (gx, -64.0, 64.0),
                    GradientKind::Radial => ((gx * gx + gy * gy).sqrt(), 0.0, 64.0),
                    GradientKind::Diamond => (gx.abs().max(gy.abs()), 0.0, 64.0),
                    GradientKind::Conic => (gy.atan2(gx).abs() * 64.0 / std::f32::consts::PI, 0.0, 64.0),
                    GradientKind::Xy => (gx.abs() * gy.abs() / 64.0, 0.0, 64.0),
                    GradientKind::SqrtXy => ((gx.abs() * gy.abs()).sqrt(), 0.0, 64.0),
                };
                let i = ((d - start) * 256.0 / (end - start)).floor().clamp(0.0, 255.0) as usize;
                table[i]
            }
        }
    }
}

/// Haiku's Gradient::MakeGradient: 256 colours interpolated (in sRGB)
/// between the stops, the ends held flat.
fn gradient_table(stops: &[(f32, Rgba)]) -> [Rgba; 256] {
    let mut table = [[0u8; 4]; 256];
    let mut sorted: Vec<(f32, Rgba)> = stops.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(&(_, first)) = sorted.first() else { return table };
    let last = sorted.last().expect("non-empty").1;
    let index = |o: f32| ((256.0 * o + 0.5).floor() as i32).clamp(0, 256) as usize;
    table.fill(first);
    for pair in sorted.windows(2) {
        let ((o0, c0), (o1, c1)) = (pair[0], pair[1]);
        let (i0, i1) = (index(o0), index(o1));
        for (i, slot) in table.iter_mut().enumerate().take(i1).skip(i0) {
            let t = (i - i0) as f32 / (i1 - i0) as f32;
            *slot = std::array::from_fn(|k| (c0[k] as f32 + (c1[k] as f32 - c0[k] as f32) * t).round() as u8);
        }
    }
    let end = index(sorted.last().expect("non-empty").0);
    for slot in table.iter_mut().skip(end) {
        *slot = last;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_coords_and_floats() {
        let mut r = Reader { data: &[32, 0x80 | 0x33, 0x00, 0x3f, 0x80, 0x00], at: 0 };
        assert_eq!(r.coord().unwrap(), 0.0);
        assert!((r.coord().unwrap() - (0x3300 as f32 / 102.0 - 128.0)).abs() < 1e-4);
        // 1.0: exponent 0 (biased 32), mantissa 0.
        let mut r = Reader { data: &[0x40, 0x00, 0x00], at: 0 };
        assert_eq!(r.float24().unwrap(), 1.0);
        assert_eq!(Icon::parse(b"nope").unwrap_err(), Error::Magic);
        assert_eq!(Icon::parse(b"ncif\x01\x01").unwrap_err(), Error::Truncated);
    }

    #[test]
    fn fills_a_square() {
        // One opaque red style, one closed square path (16,16)-(48,48),
        // one shape.
        let mut data = b"ncif".to_vec();
        data.extend([1, 3, 255, 0, 0]);
        data.extend([1, 1 << 1 | 1 << 3, 4, 48, 48, 80, 48, 80, 80, 48, 80]);
        data.extend([1, 10, 0, 1, 0, 0]);
        let icon = Icon::parse(&data).unwrap();
        let pix = icon.render(64);
        let at = |x, y| pix.pixel(x, y).unwrap();
        assert_eq!((at(32, 32).red(), at(32, 32).alpha()), (255, 255));
        assert_eq!(at(8, 8).alpha(), 0);
        assert_eq!(at(16, 16).alpha(), 255);
        assert_eq!(at(48, 48).alpha(), 0);
    }

    #[test]
    fn gradient_table_holds_the_ends() {
        let t = gradient_table(&[(0.25, [0, 0, 0, 255]), (0.75, [255, 255, 255, 255])]);
        assert_eq!(t[0], [0, 0, 0, 255]);
        assert_eq!(t[255], [255, 255, 255, 255]);
        assert!(t[128][0] > 120 && t[128][0] < 135);
    }
}
