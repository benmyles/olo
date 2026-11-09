//! SVG geometry as scripts ask for it (SVG 2 §8.2, §8.3: SVGGraphicsElement
//! and SVGGeometryElement): an element's outline in its own user space
//! from its attributes, as SVG defines each shape's path (where it starts,
//! which way it turns), its length and points along it, its bounding box,
//! and the matrices from its user space to its viewport's and the page's.
use crate::dom::{Document, Namespace, NodeId, NodeType};
use kurbo::{Affine, BezPath, Rect, Shape};
use svgtypes::SimplePathSegment as S;

/// The most path data or points a script's question reads.
const SOURCE_BYTES: usize = 1 << 20;

fn svg_name(doc: &Document, node: NodeId) -> Option<&str> {
    (doc.namespace(node).ok()? == Some(Namespace::Svg)).then(|| doc.name(node).ok().flatten()).flatten()
}
/// The nearest `<svg>` above a node (its viewport element).
fn viewport_of(doc: &Document, node: NodeId) -> Option<NodeId> {
    let mut at = doc.parent(node).ok().flatten();
    while let Some(n) = at {
        if svg_name(doc, n) == Some("svg") {
            return Some(n);
        }
        at = doc.parent(n).ok().flatten();
    }
    None
}
/// The element with an id, in tree order (bounded: a script's question).
fn find_id(doc: &Document, id: &str) -> Option<NodeId> {
    let mut stack = vec![doc.root()];
    let mut seen = 0usize;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > 200_000 {
            return None;
        }
        if doc.attribute(n, "id").ok().flatten() == Some(id) {
            return Some(n);
        }
        let mut children = Vec::new();
        let mut c = doc.first_child(n).ok().flatten();
        while let Some(child) = c {
            children.push(child);
            c = doc.next_sibling(child).ok().flatten();
        }
        stack.extend(children.into_iter().rev());
    }
    None
}
/// What a percentage length is of: the viewport's viewBox, else its
/// width and height when they are numbers, else 300 × 150.
fn percent_basis(doc: &Document, node: NodeId) -> (f64, f64) {
    let Some(svg) = viewport_of(doc, node) else { return (300., 150.) };
    if let Some(vb) = doc.attribute(svg, "viewBox").ok().flatten().and_then(|v| v.parse::<svgtypes::ViewBox>().ok()) {
        return (vb.w, vb.h);
    }
    let dim = |name: &str, default: f64| doc.attribute(svg, name).ok().flatten().and_then(|v| v.trim().strip_suffix("px").unwrap_or(v.trim()).parse::<f64>().ok()).unwrap_or(default);
    (dim("width", 300.), dim("height", 150.))
}
/// A length attribute in user units: a number, px, or a percentage of the
/// viewport's width, height or diagonal.
fn length(doc: &Document, node: NodeId, name: &str, axis: u8) -> Option<f64> {
    let value = doc.attribute(node, name).ok().flatten()?.trim();
    if let Some(p) = value.strip_suffix('%') {
        let p = p.trim().parse::<f64>().ok()? / 100.;
        let (w, h) = percent_basis(doc, node);
        return Some(
            p * match axis {
                0 => w,
                1 => h,
                _ => ((w * w + h * h) / 2.).sqrt(),
            },
        );
    }
    let v = value.strip_suffix("px").unwrap_or(value).trim().parse::<f64>().ok()?;
    v.is_finite().then_some(v)
}
fn len0(doc: &Document, node: NodeId, name: &str, axis: u8) -> f64 {
    length(doc, node, name, axis).unwrap_or(0.)
}
/// Quarter arcs of an ellipse about (cx, cy), from angle `start` (radians,
/// clockwise on screen), appended to `path`.
fn quarters(path: &mut BezPath, cx: f64, cy: f64, rx: f64, ry: f64, start: f64, count: usize) {
    for i in 0..count {
        let arc = kurbo::Arc::new((cx, cy), (rx, ry), start + i as f64 * std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2, 0.);
        arc.to_cubic_beziers(0.01, |p1, p2, p| path.curve_to(p1, p2, p));
    }
}
/// An element's outline in its own user space (before its `transform`),
/// as SVG 2 defines each shape's equivalent path.
pub fn outline(doc: &Document, node: NodeId) -> Option<BezPath> {
    let name = svg_name(doc, node)?;
    let mut path = BezPath::new();
    match name {
        "path" => {
            let d = doc.attribute(node, "d").ok().flatten()?;
            if d.len() > SOURCE_BYTES {
                return None;
            }
            // Up to the first error, as rendering does (SVG 2 §9.5.4).
            for segment in svgtypes::SimplifyingPathParser::from(d) {
                match segment {
                    Ok(S::MoveTo { x, y }) => path.move_to((x, y)),
                    Ok(S::LineTo { x, y }) => path.line_to((x, y)),
                    Ok(S::CurveTo { x1, y1, x2, y2, x, y }) => path.curve_to((x1, y1), (x2, y2), (x, y)),
                    Ok(S::Quadratic { x1, y1, x, y }) => path.quad_to((x1, y1), (x, y)),
                    Ok(S::ClosePath) => path.close_path(),
                    Err(_) => break,
                }
            }
        }
        "rect" => {
            let (x, y) = (len0(doc, node, "x", 0), len0(doc, node, "y", 1));
            let (w, h) = (len0(doc, node, "width", 0), len0(doc, node, "height", 1));
            if !(w > 0. && h > 0.) {
                return Some(path);
            }
            // rx and ry: one takes the other when missing, both at most half the side.
            let (rx, ry) = match (length(doc, node, "rx", 0), length(doc, node, "ry", 1)) {
                (Some(rx), Some(ry)) => (rx, ry),
                (Some(r), None) | (None, Some(r)) => (r, r),
                (None, None) => (0., 0.),
            };
            let (rx, ry) = (rx.clamp(0., w / 2.), ry.clamp(0., h / 2.));
            if rx > 0. && ry > 0. {
                path.move_to((x + rx, y));
                path.line_to((x + w - rx, y));
                quarters(&mut path, x + w - rx, y + ry, rx, ry, -std::f64::consts::FRAC_PI_2, 1);
                path.line_to((x + w, y + h - ry));
                quarters(&mut path, x + w - rx, y + h - ry, rx, ry, 0., 1);
                path.line_to((x + rx, y + h));
                quarters(&mut path, x + rx, y + h - ry, rx, ry, std::f64::consts::FRAC_PI_2, 1);
                path.line_to((x, y + ry));
                quarters(&mut path, x + rx, y + ry, rx, ry, std::f64::consts::PI, 1);
            } else {
                path.move_to((x, y));
                path.line_to((x + w, y));
                path.line_to((x + w, y + h));
                path.line_to((x, y + h));
            }
            path.close_path();
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (len0(doc, node, "cx", 0), len0(doc, node, "cy", 1));
            let (rx, ry) = if name == "circle" {
                let r = len0(doc, node, "r", 2);
                (r, r)
            } else {
                let rx = length(doc, node, "rx", 0);
                let ry = length(doc, node, "ry", 1);
                (rx.or(ry).unwrap_or(0.), ry.or(rx).unwrap_or(0.))
            };
            if !(rx > 0. && ry > 0.) {
                return Some(path);
            }
            // From (cx + rx, cy), clockwise (SVG 2 §10.3, §10.4).
            path.move_to((cx + rx, cy));
            quarters(&mut path, cx, cy, rx, ry, 0., 4);
            path.close_path();
        }
        "line" => {
            path.move_to((len0(doc, node, "x1", 0), len0(doc, node, "y1", 1)));
            path.line_to((len0(doc, node, "x2", 0), len0(doc, node, "y2", 1)));
        }
        "polyline" | "polygon" => {
            let points = doc.attribute(node, "points").ok().flatten().unwrap_or("");
            if points.len() > SOURCE_BYTES {
                return None;
            }
            for (i, (x, y)) in svgtypes::PointsParser::from(points).enumerate() {
                if i == 0 {
                    path.move_to((x, y));
                } else {
                    path.line_to((x, y));
                }
            }
            if name == "polygon" && !path.elements().is_empty() {
                path.close_path();
            }
        }
        _ => return None,
    }
    Some(path)
}
/// getTotalLength: the outline's length, measured as Chrome measures it
/// (Skia's contour measure: each curve cut in halves until its control
/// points are within half a pixel of its chord, the chords summed, in
/// single precision; circles and rounded corners as conics, path data's
/// curves as cubics, as Blink builds them). Chrome's circle of radius 10
/// is 62.427 long, not 2πr, and pages (and fingerprints) read that.
pub fn total_length(doc: &Document, node: NodeId) -> Option<f64> {
    if svg_name(doc, node)? == "path" {
        return Some(f64::from(blink_path(doc, node, None)?.0));
    }
    let contours = skia_contours(doc, node)?;
    Some(contours.iter().map(|c| f64::from(c.length())).sum())
}
/// getPointAtLength: the point `distance` along the outline (clamped to
/// its ends), found as Skia finds it: the piece the distance falls in, its
/// curve parameter interpolated along the piece's chord.
pub fn point_at_length(doc: &Document, node: NodeId, distance: f64) -> Option<(f64, f64)> {
    if svg_name(doc, node)? == "path" {
        let (_, p) = blink_path(doc, node, Some(distance as f32))?;
        return Some((f64::from(p.0), f64::from(p.1)));
    }
    let contours = skia_contours(doc, node)?;
    let mut left = (distance as f32).max(0.);
    let last = contours.iter().rev().find(|c| !c.pieces.is_empty());
    for c in &contours {
        let len = c.length();
        if c.pieces.is_empty() {
            continue;
        }
        if left <= len {
            let p = c.point_at(left);
            return Some((f64::from(p.0), f64::from(p.1)));
        }
        left -= len;
    }
    match last {
        Some(c) => {
            let p = c.point_at(c.length());
            Some((f64::from(p.0), f64::from(p.1)))
        }
        // Nothing to measure: the outline's first point, else the origin.
        None => outline(doc, node)?.elements().first().and_then(|e| e.end_point()).map(|p| (p.x, p.y)).or(Some((0., 0.))),
    }
}

/// A `<path>`'s length, or its point at a distance: path data's curves
/// measured exactly (Chrome's `<path>` lengths are the true arc length to
/// single precision, unlike its other shapes', which are Skia's), the
/// point on the curve at that length, clamped to the path's ends.
fn blink_path(doc: &Document, node: NodeId, desired: Option<f32>) -> Option<(f32, F)> {
    use kurbo::{ParamCurve, ParamCurveArclen};
    const ACCURACY: f64 = 1e-9;
    let path = outline(doc, node)?;
    let mut total = 0f64;
    let mut last = path.elements().first().and_then(|e| e.end_point()).unwrap_or_default();
    let want = desired.map(|d| f64::from(d).max(0.));
    for seg in path.segments() {
        let len = seg.arclen(ACCURACY);
        if let Some(d) = want
            && total + len >= d
        {
            let t = if len > 0. { seg.inv_arclen(d - total, ACCURACY) } else { 0. };
            let p = seg.eval(t);
            return Some(((total + len) as f32, (p.x as f32, p.y as f32)));
        }
        total += len;
        last = seg.end();
    }
    Some((total as f32, (last.x as f32, last.y as f32)))
}
type F = (f32, f32);
#[derive(Clone, Copy)]
enum Seg {
    Line(F, F),
    Quad([F; 3]),
    Cubic([F; 4]),
    Conic([F; 3], f32),
}
/// One measured piece: where its distance ends, its segment, and the
/// curve parameter it ends at (Skia's fixed point, 30 bits).
struct Piece {
    distance: f32,
    seg: usize,
    t: u32,
}
struct Contour {
    segs: Vec<Seg>,
    pieces: Vec<Piece>,
}
const MAX_T: u32 = 0x3FFF_FFFF;
const TOLERANCE: f32 = 0.5;
fn t_scalar(t: u32) -> f32 {
    t as f32 * (1. / MAX_T as f32)
}
fn big_enough(span: u32) -> bool {
    span >> 10 != 0
}
fn dist(a: F, b: F) -> f32 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    (dx * dx + dy * dy).sqrt()
}
fn half(a: F, b: F) -> F {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}
fn exceeds(p: F, x: f32, y: f32) -> bool {
    (x - p.0).abs().max((y - p.1).abs()) > TOLERANCE
}
fn quad_at(p: &[F; 3], t: f32) -> F {
    let u = 1. - t;
    (u * u * p[0].0 + 2. * u * t * p[1].0 + t * t * p[2].0, u * u * p[0].1 + 2. * u * t * p[1].1 + t * t * p[2].1)
}
fn cubic_at(p: &[F; 4], t: f32) -> F {
    let u = 1. - t;
    let (a, b, c, d) = (u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t);
    (a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0, a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1)
}
/// A conic at t, as Skia's SkConicCoeff evaluates it (numerator and
/// denominator as quadratics in t, single precision), so lengths come out
/// as Chrome's to the last bit.
fn conic_at(p: &[F; 3], w: f32, t: f32) -> F {
    let axis = |p0: f32, p1: f32, p2: f32| {
        let p1w = p1 * w;
        let (a, b, c) = (p2 - 2. * p1w + p0, 2. * (p1w - p0), p0);
        (a * t + b) * t + c
    };
    let db = 2. * (w - 1.);
    let denom = (-db * t + db) * t + 1.;
    (axis(p[0].0, p[1].0, p[2].0) / denom, axis(p[0].1, p[1].1, p[2].1) / denom)
}
impl Contour {
    fn length(&self) -> f32 {
        self.pieces.last().map_or(0., |p| p.distance)
    }
    fn push(&mut self, distance: &mut f32, d: f32, seg: usize, t: u32) {
        let before = *distance;
        *distance += d;
        if *distance > before {
            self.pieces.push(Piece { distance: *distance, seg, t });
        }
    }
    fn quad(&mut self, p: [F; 3], distance: &mut f32, min: u32, max: u32, seg: usize) {
        let dx = p[1].0 * 0.5 - (p[0].0 + p[2].0) * 0.5 * 0.5;
        let dy = p[1].1 * 0.5 - (p[0].1 + p[2].1) * 0.5 * 0.5;
        if big_enough(max - min) && dx.abs().max(dy.abs()) > TOLERANCE {
            let (a, b) = (half(p[0], p[1]), half(p[1], p[2]));
            let m = half(a, b);
            let mid = (min + max) >> 1;
            self.quad([p[0], a, m], distance, min, mid, seg);
            self.quad([m, b, p[2]], distance, mid, max, seg);
        } else {
            self.push(distance, dist(p[0], p[2]), seg, max);
        }
    }
    fn cubic(&mut self, p: [F; 4], distance: &mut f32, min: u32, max: u32, seg: usize) {
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let curvy = exceeds(p[1], lerp(p[0].0, p[3].0, 1. / 3.), lerp(p[0].1, p[3].1, 1. / 3.)) || exceeds(p[2], lerp(p[0].0, p[3].0, 2. / 3.), lerp(p[0].1, p[3].1, 2. / 3.));
        if big_enough(max - min) && curvy {
            let (ab, bc, cd) = (half(p[0], p[1]), half(p[1], p[2]), half(p[2], p[3]));
            let (abc, bcd) = (half(ab, bc), half(bc, cd));
            let m = half(abc, bcd);
            let mid = (min + max) >> 1;
            self.cubic([p[0], ab, abc, m], distance, min, mid, seg);
            self.cubic([m, bcd, cd, p[3]], distance, mid, max, seg);
        } else {
            self.push(distance, dist(p[0], p[3]), seg, max);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn conic(&mut self, p: [F; 3], w: f32, distance: &mut f32, min: u32, min_pt: F, max: u32, max_pt: F, seg: usize) {
        let mid = (min + max) >> 1;
        let half_pt = conic_at(&p, w, t_scalar(mid));
        if !(half_pt.0.is_finite() && half_pt.1.is_finite()) {
            return;
        }
        let ends = half(min_pt, max_pt);
        let curvy = (half_pt.0 - ends.0).abs().max((half_pt.1 - ends.1).abs()) > TOLERANCE;
        if big_enough(max - min) && curvy {
            self.conic(p, w, distance, min, min_pt, mid, half_pt, seg);
            self.conic(p, w, distance, mid, half_pt, max, max_pt, seg);
        } else {
            self.push(distance, dist(min_pt, max_pt), seg, max);
        }
    }
    fn measure(&mut self) {
        let mut distance = 0f32;
        for i in 0..self.segs.len() {
            match self.segs[i] {
                Seg::Line(a, b) => self.push(&mut distance, dist(a, b), i, MAX_T),
                Seg::Quad(p) => self.quad(p, &mut distance, 0, MAX_T, i),
                Seg::Cubic(p) => self.cubic(p, &mut distance, 0, MAX_T, i),
                Seg::Conic(p, w) => self.conic(p, w, &mut distance, 0, p[0], MAX_T, p[2], i),
            }
        }
    }
    fn point_at(&self, d: f32) -> F {
        let d = d.clamp(0., self.length());
        let i = self.pieces.partition_point(|p| p.distance < d).min(self.pieces.len() - 1);
        let piece = &self.pieces[i];
        let (start_d, start_t) = match i.checked_sub(1).map(|j| &self.pieces[j]) {
            Some(prev) => (prev.distance, if prev.seg == piece.seg { t_scalar(prev.t) } else { 0. }),
            None => (0., 0.),
        };
        let span = piece.distance - start_d;
        let t = start_t + (t_scalar(piece.t) - start_t) * if span > 0. { (d - start_d) / span } else { 0. };
        match self.segs[piece.seg] {
            Seg::Line(a, b) => (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
            Seg::Quad(p) => quad_at(&p, t),
            Seg::Cubic(p) => cubic_at(&p, t),
            Seg::Conic(p, w) => conic_at(&p, w, t),
        }
    }
}
/// The outline as Chrome's Skia path has it: contours of lines, cubics
/// (path data's quadratics raised to cubics, as Blink normalizes them)
/// and conics (circles, ellipses and rounded corners, a quarter each),
/// each measured.
fn skia_contours(doc: &Document, node: NodeId) -> Option<Vec<Contour>> {
    let name = svg_name(doc, node)?;
    let f = |x: f64, y: f64| (x as f32, y as f32);
    let w = std::f32::consts::FRAC_1_SQRT_2;
    let mut contours: Vec<Contour> = Vec::new();
    let mut close_to: Option<F> = None;
    let mut at: F = (0., 0.);
    let new = |contours: &mut Vec<Contour>| contours.push(Contour { segs: Vec::new(), pieces: Vec::new() });
    let ellipse = |contours: &mut Vec<Contour>, cx: f64, cy: f64, rx: f64, ry: f64| {
        let (l, t, r, b) = (f(cx - rx, cy).0, f(cx, cy - ry).1, f(cx + rx, cy).0, f(cx, cy + ry).1);
        let (cx, cy) = f(cx, cy);
        contours.push(Contour {
            segs: vec![Seg::Conic([(r, cy), (r, b), (cx, b)], w), Seg::Conic([(cx, b), (l, b), (l, cy)], w), Seg::Conic([(l, cy), (l, t), (cx, t)], w), Seg::Conic([(cx, t), (r, t), (r, cy)], w)],
            pieces: Vec::new(),
        });
    };
    match name {
        "circle" | "ellipse" => {
            let (cx, cy) = (len0(doc, node, "cx", 0), len0(doc, node, "cy", 1));
            let (rx, ry) = if name == "circle" {
                let r = len0(doc, node, "r", 2);
                (r, r)
            } else {
                let rx = length(doc, node, "rx", 0);
                let ry = length(doc, node, "ry", 1);
                (rx.or(ry).unwrap_or(0.), ry.or(rx).unwrap_or(0.))
            };
            if rx > 0. && ry > 0. {
                ellipse(&mut contours, cx, cy, rx, ry);
            }
        }
        "rect" => {
            let (x, y) = (len0(doc, node, "x", 0), len0(doc, node, "y", 1));
            let (wd, ht) = (len0(doc, node, "width", 0), len0(doc, node, "height", 1));
            if wd > 0. && ht > 0. {
                let (rx, ry) = match (length(doc, node, "rx", 0), length(doc, node, "ry", 1)) {
                    (Some(rx), Some(ry)) => (rx, ry),
                    (Some(r), None) | (None, Some(r)) => (r, r),
                    (None, None) => (0., 0.),
                };
                let (rx, ry) = (rx.clamp(0., wd / 2.), ry.clamp(0., ht / 2.));
                let p = |px: f64, py: f64| f(px, py);
                let segs = if rx > 0. && ry > 0. {
                    vec![
                        Seg::Line(p(x + rx, y), p(x + wd - rx, y)),
                        Seg::Conic([p(x + wd - rx, y), p(x + wd, y), p(x + wd, y + ry)], w),
                        Seg::Line(p(x + wd, y + ry), p(x + wd, y + ht - ry)),
                        Seg::Conic([p(x + wd, y + ht - ry), p(x + wd, y + ht), p(x + wd - rx, y + ht)], w),
                        Seg::Line(p(x + wd - rx, y + ht), p(x + rx, y + ht)),
                        Seg::Conic([p(x + rx, y + ht), p(x, y + ht), p(x, y + ht - ry)], w),
                        Seg::Line(p(x, y + ht - ry), p(x, y + ry)),
                        Seg::Conic([p(x, y + ry), p(x, y), p(x + rx, y)], w),
                    ]
                } else {
                    vec![Seg::Line(p(x, y), p(x + wd, y)), Seg::Line(p(x + wd, y), p(x + wd, y + ht)), Seg::Line(p(x + wd, y + ht), p(x, y + ht)), Seg::Line(p(x, y + ht), p(x, y))]
                };
                contours.push(Contour { segs, pieces: Vec::new() });
            }
        }
        _ => {
            // Lines, polylines, polygons and path data: through the outline,
            // quadratics as cubics.
            for e in outline(doc, node)?.elements() {
                match *e {
                    kurbo::PathEl::MoveTo(p) => {
                        at = f(p.x, p.y);
                        close_to = Some(at);
                        new(&mut contours);
                    }
                    kurbo::PathEl::LineTo(p) => {
                        if contours.is_empty() {
                            new(&mut contours);
                        }
                        let to = f(p.x, p.y);
                        contours.last_mut()?.segs.push(Seg::Line(at, to));
                        at = to;
                    }
                    kurbo::PathEl::QuadTo(q, p) => {
                        if contours.is_empty() {
                            new(&mut contours);
                        }
                        let (q, to) = (f(q.x, q.y), f(p.x, p.y));
                        let c1 = (at.0 + 2. / 3. * (q.0 - at.0), at.1 + 2. / 3. * (q.1 - at.1));
                        let c2 = (to.0 + 2. / 3. * (q.0 - to.0), to.1 + 2. / 3. * (q.1 - to.1));
                        contours.last_mut()?.segs.push(Seg::Cubic([at, c1, c2, to]));
                        at = to;
                    }
                    kurbo::PathEl::CurveTo(a, b, p) => {
                        if contours.is_empty() {
                            new(&mut contours);
                        }
                        let to = f(p.x, p.y);
                        contours.last_mut()?.segs.push(Seg::Cubic([at, f(a.x, a.y), f(b.x, b.y), to]));
                        at = to;
                    }
                    kurbo::PathEl::ClosePath => {
                        if let (Some(start), Some(c)) = (close_to, contours.last_mut())
                            && start != at
                        {
                            c.segs.push(Seg::Line(at, start));
                            at = start;
                        }
                    }
                }
            }
        }
    }
    for c in &mut contours {
        c.measure();
    }
    Some(contours)
}
/// An element's own `transform` attribute (identity when it has none or
/// it doesn't parse).
pub fn own_transform(doc: &Document, node: NodeId) -> Affine {
    let Some(t) = doc.attribute(node, "transform").ok().flatten().and_then(|v| v.parse::<svgtypes::Transform>().ok()) else { return Affine::IDENTITY };
    Affine::new([t.a, t.b, t.c, t.d, t.e, t.f])
}
/// getBBox: the element's geometry in its own user space, its children's
/// (each through its transform) for a container, and a text's from
/// `text` (its advance and font box, which the caller shapes). None for
/// an element with nothing to bound.
pub fn bbox(doc: &Document, node: NodeId, text: &mut dyn FnMut(NodeId) -> Option<Rect>) -> Option<Rect> {
    bbox_at(doc, node, text, 0)
}
fn bbox_at(doc: &Document, node: NodeId, text: &mut dyn FnMut(NodeId) -> Option<Rect>, depth: usize) -> Option<Rect> {
    if depth > 256 {
        return None;
    }
    let name = svg_name(doc, node)?;
    match name {
        // The basic shapes bound by their own geometry (exact, as Blink's).
        "circle" | "ellipse" => {
            let (cx, cy) = (len0(doc, node, "cx", 0), len0(doc, node, "cy", 1));
            let (rx, ry) = if name == "circle" {
                let r = len0(doc, node, "r", 2).max(0.);
                (r, r)
            } else {
                let rx = length(doc, node, "rx", 0);
                let ry = length(doc, node, "ry", 1);
                (rx.or(ry).unwrap_or(0.).max(0.), ry.or(rx).unwrap_or(0.).max(0.))
            };
            Some(Rect::new(cx - rx, cy - ry, cx + rx, cy + ry))
        }
        "rect" => {
            let (x, y) = (len0(doc, node, "x", 0), len0(doc, node, "y", 1));
            let (w, h) = (len0(doc, node, "width", 0).max(0.), len0(doc, node, "height", 1).max(0.));
            Some(Rect::new(x, y, x + w, y + h))
        }
        "path" | "line" | "polyline" | "polygon" => {
            let path = outline(doc, node)?;
            if path.elements().is_empty() {
                // An empty shape: its position, no size (Chrome's for an empty rect).
                return Some(Rect::ZERO);
            }
            Some(path.bounding_box())
        }
        "text" | "tspan" | "textPath" => text(node),
        "use" => {
            let href = doc.attribute(node, "href").ok().flatten().or_else(|| doc.attribute(node, "xlink:href").ok().flatten())?;
            let target = find_id(doc, href.trim().strip_prefix('#')?)?;
            let inner = bbox_at(doc, target, text, depth + 1)?;
            let moved = own_transform(doc, target) * Affine::translate((len0(doc, node, "x", 0), len0(doc, node, "y", 1)));
            Some(moved.transform_rect_bbox(inner))
        }
        "g" | "svg" | "a" | "switch" | "symbol" => {
            let mut out: Option<Rect> = None;
            let mut child = doc.first_child(node).ok().flatten();
            while let Some(c) = child {
                if doc.node_type(c).ok() == Some(NodeType::Element)
                    && let Some(b) = bbox_at(doc, c, text, depth + 1)
                {
                    let b = own_transform(doc, c).transform_rect_bbox(b);
                    out = Some(out.map_or(b, |o| o.union(b)));
                }
                child = doc.next_sibling(c).ok().flatten();
            }
            out.or(Some(Rect::ZERO))
        }
        _ => None,
    }
}
/// The viewBox mapping of an `<svg>` whose viewport is `width` × `height`
/// (its preserveAspectRatio aligning and scaling it).
pub fn view_box_transform(doc: &Document, svg: NodeId, width: f64, height: f64) -> Affine {
    let Some(vb) = doc.attribute(svg, "viewBox").ok().flatten().and_then(|v| v.parse::<svgtypes::ViewBox>().ok()) else { return Affine::IDENTITY };
    if !(vb.w > 0. && vb.h > 0. && width > 0. && height > 0.) {
        return Affine::IDENTITY;
    }
    let aspect = doc.attribute(svg, "preserveAspectRatio").ok().flatten().and_then(|v| v.parse::<svgtypes::AspectRatio>().ok()).unwrap_or_default();
    let (mut sx, mut sy) = (width / vb.w, height / vb.h);
    use svgtypes::Align::*;
    if aspect.align != None {
        let k = if aspect.slice { sx.max(sy) } else { sx.min(sy) };
        (sx, sy) = (k, k);
    }
    let ax = match aspect.align {
        XMidYMin | XMidYMid | XMidYMax => 0.5,
        XMaxYMin | XMaxYMid | XMaxYMax => 1.,
        _ => 0.,
    };
    let ay = match aspect.align {
        XMinYMid | XMidYMid | XMaxYMid => 0.5,
        XMinYMax | XMidYMax | XMaxYMax => 1.,
        _ => 0.,
    };
    Affine::new([sx, 0., 0., sy, -vb.x * sx + (width - vb.w * sx) * ax, -vb.y * sy + (height - vb.h * sy) * ay])
}
/// getCTM (`screen` false) and getScreenCTM (true): from the element's
/// user space through its and its ancestors' transforms to its viewport's
/// (its viewBox included), and for the screen on through nested viewports
/// to the outermost `<svg>`'s content box, which `placed` gives in client
/// coordinates (x, y, width, height). None outside an `<svg>`.
pub fn ctm(doc: &Document, node: NodeId, screen: bool, placed: &mut dyn FnMut(NodeId) -> Option<[f64; 4]>) -> Option<Affine> {
    svg_name(doc, node)?;
    let mut m = Affine::IDENTITY;
    let mut at = Some(node);
    // An outermost <svg> asked for its own: its viewport's (identity) or
    // the page's position.
    for _ in 0..4096 {
        let n = at?;
        let is_svg = svg_name(doc, n) == Some("svg");
        if is_svg && n != node {
            // A viewport: its viewBox, its x/y when nested.
            let outer = viewport_of(doc, n).is_none();
            let size = if outer {
                placed(n).map(|r| (r[2], r[3]))
            } else {
                Some((length(doc, n, "width", 0).unwrap_or_else(|| percent_basis(doc, n).0), length(doc, n, "height", 1).unwrap_or_else(|| percent_basis(doc, n).1)))
            };
            let (w, h) = size.unwrap_or((0., 0.));
            m = view_box_transform(doc, n, w, h) * m;
            if !screen {
                return Some(m);
            }
            if outer {
                let r = placed(n)?;
                return Some(Affine::translate((r[0], r[1])) * m);
            }
            m = Affine::translate((len0(doc, n, "x", 0), len0(doc, n, "y", 1))) * m;
        } else if is_svg {
            // The element itself an <svg>: nested, its x/y; outermost, the page's.
            if viewport_of(doc, n).is_none() {
                if !screen {
                    return Some(m);
                }
                let r = placed(n)?;
                return Some(Affine::translate((r[0], r[1])) * m);
            }
            if screen {
                m = Affine::translate((len0(doc, n, "x", 0), len0(doc, n, "y", 1))) * m;
            }
        } else {
            m = own_transform(doc, n) * m;
        }
        at = doc.parent(n).ok().flatten();
    }
    None
}
