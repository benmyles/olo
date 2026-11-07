//! The 2D canvas: a raster per `<canvas>` element, drawn by the page's
//! scripts through CanvasRenderingContext2D, which the prelude forwards here
//! call by call. Paths are kept in device space (points transformed as they
//! are added, as the specification says), fills and strokes go through
//! tiny-skia, clips are masks. The page renders a canvas as an image of its
//! pixels.
use olo_web::css::{self, color};
use std::sync::Arc;
use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Point, Shader, Stroke, Transform};

#[derive(Clone)]
enum Style {
    Color([u8; 4]),
    Gradient(u32),
}
#[derive(Clone)]
struct State {
    transform: Transform,
    fill: Style,
    stroke: Style,
    line_width: f32,
    cap: LineCap,
    join: LineJoin,
    miter: f32,
    dash: Vec<f32>,
    dash_offset: f32,
    alpha: f32,
    blend: BlendMode,
    clip: Option<Arc<Mask>>,
    smoothing: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            transform: Transform::identity(),
            fill: Style::Color([0, 0, 0, 255]),
            stroke: Style::Color([0, 0, 0, 255]),
            line_width: 1.,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 10.,
            dash: Vec::new(),
            dash_offset: 0.,
            alpha: 1.,
            blend: BlendMode::SourceOver,
            clip: None,
            smoothing: true,
        }
    }
}
struct Gradient {
    kind: GradientKind,
    stops: Vec<(f32, [u8; 4])>,
}
enum GradientKind {
    Linear { x0: f32, y0: f32, x1: f32, y1: f32 },
    Radial { x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32 },
}
/// One canvas's raster and drawing state.
pub struct Canvas {
    pixmap: Pixmap,
    state: State,
    stack: Vec<State>,
    path: PathBuilder,
    /// The current subpath's start and the last point, in user space.
    start: Option<(f32, f32)>,
    last: Option<(f32, f32)>,
    gradients: Vec<Gradient>,
    /// Drawn since last exported.
    pub dirty: bool,
    /// Ever drawn on (an untouched canvas paints as nothing, transparent).
    pub touched: bool,
    last_image: Option<Arc<olo_web::image::Image>>,
}
/// Canvas sizes are bounded (a canvas is a raster the page keeps).
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u32 = 16 << 20;
impl Canvas {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let width = width.clamp(1, MAX_SIDE);
        let height = height.clamp(1, MAX_SIDE);
        if width.saturating_mul(height) > MAX_PIXELS {
            return None;
        }
        Some(Self {
            pixmap: Pixmap::new(width, height)?,
            state: State::default(),
            stack: Vec::new(),
            path: PathBuilder::new(),
            start: None,
            last: None,
            gradients: Vec::new(),
            dirty: false,
            touched: false,
            last_image: None,
        })
    }
    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }
    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }
    /// Setting a canvas's size clears it and resets its state (HTML §4.12.5).
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(fresh) = Self::new(width, height) {
            let gradients = std::mem::take(&mut self.gradients);
            *self = fresh;
            self.gradients = gradients;
            self.dirty = true;
        }
    }
    /// The pixels, premultiplied RGBA, as the page's image of the canvas.
    /// The raster as the page's image, kept while nothing was drawn since
    /// (the same image means the same pixels to the painter).
    pub fn image(&mut self) -> Option<Arc<olo_web::image::Image>> {
        if !self.dirty && self.last_image.is_some() {
            return self.last_image.clone();
        }
        self.dirty = false;
        self.last_image = olo_web::image::Image::from_premultiplied(self.pixmap.width(), self.pixmap.height(), self.pixmap.data().to_vec(), 1., Default::default(), None).ok();
        self.last_image.clone()
    }
    fn paint(&self, style: &Style) -> Paint<'static> {
        let mut paint = Paint { anti_alias: self.state.smoothing, blend_mode: self.state.blend, ..Default::default() };
        match style {
            Style::Color(c) => {
                let a = (f32::from(c[3]) / 255. * self.state.alpha).clamp(0., 1.);
                paint.set_color(tiny_skia::Color::from_rgba(f32::from(c[0]) / 255., f32::from(c[1]) / 255., f32::from(c[2]) / 255., a).unwrap_or(tiny_skia::Color::BLACK));
            }
            Style::Gradient(id) => {
                let shader = self.gradients.get(*id as usize).and_then(|g| {
                    let stops: Vec<tiny_skia::GradientStop> = g
                        .stops
                        .iter()
                        .map(|(o, c)| tiny_skia::GradientStop::new(*o, tiny_skia::Color::from_rgba8(c[0], c[1], c[2], (f32::from(c[3]) * self.state.alpha).round().clamp(0., 255.) as u8)))
                        .collect();
                    if stops.is_empty() {
                        return None;
                    }
                    match g.kind {
                        GradientKind::Linear { x0, y0, x1, y1 } => {
                            tiny_skia::LinearGradient::new(Point::from_xy(x0, y0), Point::from_xy(x1, y1), stops, tiny_skia::SpreadMode::Pad, self.state.transform)
                        }
                        GradientKind::Radial { x0, y0, r0, x1, y1, r1 } => {
                            tiny_skia::RadialGradient::new(Point::from_xy(x0, y0), r0.max(0.), Point::from_xy(x1, y1), r1.max(0.01), stops, tiny_skia::SpreadMode::Pad, self.state.transform)
                        }
                    }
                });
                match shader {
                    Some(s) => paint.shader = s,
                    None => paint.set_color_rgba8(0, 0, 0, 0),
                }
            }
        }
        paint
    }
    fn point(&self, x: f32, y: f32) -> Point {
        let mut p = Point::from_xy(x, y);
        self.state.transform.map_point(&mut p);
        p
    }
    pub fn save(&mut self) {
        if self.stack.len() < 256 {
            self.stack.push(self.state.clone());
        }
    }
    pub fn restore(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }
    pub fn reset_state(&mut self) {
        self.state = State::default();
        self.stack.clear();
        self.begin_path();
    }
    pub fn transform(&mut self, t: Transform) {
        self.state.transform = self.state.transform.pre_concat(t);
    }
    pub fn set_transform(&mut self, t: Transform) {
        self.state.transform = t;
    }
    pub fn current_transform(&self) -> Transform {
        self.state.transform
    }
    pub fn begin_path(&mut self) {
        self.path = PathBuilder::new();
        self.start = None;
        self.last = None;
    }
    pub fn close_path(&mut self) {
        if self.last.is_some() {
            self.path.close();
            self.last = self.start;
        }
    }
    pub fn move_to(&mut self, x: f32, y: f32) {
        let p = self.point(x, y);
        self.path.move_to(p.x, p.y);
        self.start = Some((x, y));
        self.last = Some((x, y));
    }
    pub fn line_to(&mut self, x: f32, y: f32) {
        if self.last.is_none() {
            return self.move_to(x, y);
        }
        let p = self.point(x, y);
        self.path.line_to(p.x, p.y);
        self.last = Some((x, y));
    }
    pub fn bezier_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if self.last.is_none() {
            self.move_to(x1, y1);
        }
        let (a, b, c) = (self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        self.path.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y);
        self.last = Some((x, y));
    }
    pub fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if self.last.is_none() {
            self.move_to(x1, y1);
        }
        let (a, c) = (self.point(x1, y1), self.point(x, y));
        self.path.quad_to(a.x, a.y, c.x, c.y);
        self.last = Some((x, y));
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.close_path();
        self.move_to(x, y);
    }
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let r = r.min(w.abs() / 2.).min(h.abs() / 2.).max(0.);
        if r == 0. {
            return self.rect(x, y, w, h);
        }
        self.move_to(x + r, y);
        self.line_to(x + w - r, y);
        self.arc_to_point(x + w, y, x + w, y + r, r);
        self.line_to(x + w, y + h - r);
        self.arc_to_point(x + w, y + h, x + w - r, y + h, r);
        self.line_to(x + r, y + h);
        self.arc_to_point(x, y + h, x, y + h - r, r);
        self.line_to(x, y + r);
        self.arc_to_point(x, y, x + r, y, r);
        self.close_path();
        self.move_to(x + r, y);
    }
    /// An elliptical arc (user space), as cubic Béziers of at most 90° each.
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, rotation: f32, mut start: f32, mut end: f32, ccw: bool) {
        if !(rx.is_finite() && ry.is_finite() && start.is_finite() && end.is_finite()) || rx < 0. || ry < 0. {
            return;
        }
        let tau = std::f32::consts::TAU;
        // The swept angle, in the direction asked (HTML §4.12.5.1.14).
        let mut sweep = end - start;
        if !ccw {
            if sweep < 0. {
                sweep = sweep.rem_euclid(tau);
            } else if sweep > tau {
                sweep = tau;
            }
        } else {
            if sweep > 0. {
                sweep = -((-sweep).rem_euclid(tau));
            } else if sweep < -tau {
                sweep = -tau;
            }
            if sweep == 0. && start != end {
                sweep = -tau;
            }
        }
        end = start + sweep;
        let (sin_r, cos_r) = rotation.sin_cos();
        let at = |a: f32| -> (f32, f32) {
            let (sa, ca) = a.sin_cos();
            let (x, y) = (rx * ca, ry * sa);
            (cx + x * cos_r - y * sin_r, cy + x * sin_r + y * cos_r)
        };
        let (sx, sy) = at(start);
        if self.last.is_some() {
            self.line_to(sx, sy);
        } else {
            self.move_to(sx, sy);
        }
        let segments = ((sweep.abs() / (tau / 4.)).ceil() as usize).clamp(1, 8);
        let step = sweep / segments as f32;
        let k = 4. / 3. * (step / 4.).tan();
        for i in 0..segments {
            let a0 = start + step * i as f32;
            let a1 = a0 + step;
            let (sa0, ca0) = a0.sin_cos();
            let (sa1, ca1) = a1.sin_cos();
            // Control points in the ellipse's own frame, then rotated.
            let p1 = (rx * (ca0 - k * sa0), ry * (sa0 + k * ca0));
            let p2 = (rx * (ca1 + k * sa1), ry * (sa1 - k * ca1));
            let p3 = (rx * ca1, ry * sa1);
            let rot = |(x, y): (f32, f32)| (cx + x * cos_r - y * sin_r, cy + x * sin_r + y * cos_r);
            let (c1, c2, c3) = (rot(p1), rot(p2), rot(p3));
            self.bezier_to(c1.0, c1.1, c2.0, c2.1, c3.0, c3.1);
        }
        start = end;
        let _ = start;
    }
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, end: f32, ccw: bool) {
        self.ellipse(cx, cy, r, r, 0., start, end, ccw);
    }
    /// `arcTo(x1, y1, x2, y2, r)`: the arc tangent to both lines.
    pub fn arc_to_point(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, r: f32) {
        let Some((x0, y0)) = self.last else { return self.move_to(x1, y1) };
        let (ax, ay) = (x0 - x1, y0 - y1);
        let (bx, by) = (x2 - x1, y2 - y1);
        let (la, lb) = (ax.hypot(ay), bx.hypot(by));
        if r <= 0. || la == 0. || lb == 0. {
            return self.line_to(x1, y1);
        }
        let (ax, ay, bx, by) = (ax / la, ay / la, bx / lb, by / lb);
        let cos = (ax * bx + ay * by).clamp(-1., 1.);
        let angle = cos.acos();
        if angle.abs() < 1e-4 || (std::f32::consts::PI - angle).abs() < 1e-4 {
            return self.line_to(x1, y1);
        }
        let d = r / (angle / 2.).tan();
        let (tx1, ty1) = (x1 + ax * d, y1 + ay * d);
        let (tx2, ty2) = (x1 + bx * d, y1 + by * d);
        // The centre lies along the bisector at distance r / sin(angle/2).
        let (mx, my) = (ax + bx, ay + by);
        let ml = mx.hypot(my);
        let (cx, cy) = (x1 + mx / ml * r / (angle / 2.).sin(), y1 + my / ml * r / (angle / 2.).sin());
        let a0 = (ty1 - cy).atan2(tx1 - cx);
        let a1 = (ty2 - cy).atan2(tx2 - cx);
        let cross = ax * by - ay * bx;
        self.line_to(tx1, ty1);
        self.ellipse(cx, cy, r, r, 0., a0, a1, cross > 0.);
    }
    fn finished_path(&self) -> Option<Path> {
        self.path.clone().finish()
    }
    pub fn fill(&mut self, rule: FillRule) {
        let Some(path) = self.finished_path() else { return };
        let paint = self.paint(&self.state.fill);
        let mask = self.state.clip.clone();
        self.pixmap.fill_path(&path, &paint, rule, Transform::identity(), mask.as_deref());
        self.touch();
    }
    fn stroke_style(&self) -> Stroke {
        let mut stroke = Stroke { width: self.line_width_device(), line_cap: self.state.cap, line_join: self.state.join, miter_limit: self.state.miter, dash: None };
        if !self.state.dash.is_empty() && self.state.dash.iter().all(|d| d.is_finite() && *d >= 0.) && self.state.dash.iter().sum::<f32>() > 0. {
            let mut dash = self.state.dash.clone();
            if dash.len() % 2 == 1 {
                dash.extend_from_slice(&self.state.dash);
            }
            let scale = self.state.transform.get_scale();
            let scale = ((scale.0.abs() + scale.1.abs()) / 2.).max(1e-3);
            stroke.dash = tiny_skia::StrokeDash::new(dash.iter().map(|d| d * scale).collect(), self.state.dash_offset * scale);
        }
        stroke
    }
    /// Paths are in device space; the line width follows the transform's scale.
    fn line_width_device(&self) -> f32 {
        let (sx, sy) = self.state.transform.get_scale();
        (self.state.line_width * (sx.abs() + sy.abs()) / 2.).max(0.)
    }
    pub fn stroke(&mut self) {
        let Some(path) = self.finished_path() else { return };
        let paint = self.paint(&self.state.stroke);
        let stroke = self.stroke_style();
        if stroke.width > 0. {
            let mask = self.state.clip.clone();
            self.pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_deref());
            self.touch();
        }
    }
    pub fn clip(&mut self, rule: FillRule) {
        let Some(path) = self.finished_path() else { return };
        let mut mask = match &self.state.clip {
            Some(m) => (**m).clone(),
            None => {
                let mut m = Mask::new(self.pixmap.width(), self.pixmap.height()).expect("the canvas's size");
                m.data_mut().fill(255);
                m
            }
        };
        mask.intersect_path(&path, rule, self.state.smoothing, Transform::identity());
        self.state.clip = Some(Arc::new(mask));
    }
    fn device_rect_path(&self, x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
        let mut b = PathBuilder::new();
        let p = |x: f32, y: f32| self.point(x, y);
        let (a, b2, c, d) = (p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h));
        b.move_to(a.x, a.y);
        b.line_to(b2.x, b2.y);
        b.line_to(c.x, c.y);
        b.line_to(d.x, d.y);
        b.close();
        b.finish()
    }
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(path) = self.device_rect_path(x, y, w, h) else { return };
        let paint = self.paint(&self.state.fill);
        let mask = self.state.clip.clone();
        self.pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), mask.as_deref());
        self.touch();
    }
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(path) = self.device_rect_path(x, y, w, h) else { return };
        let paint = self.paint(&self.state.stroke);
        let stroke = self.stroke_style();
        if stroke.width > 0. {
            let mask = self.state.clip.clone();
            self.pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_deref());
            self.touch();
        }
    }
    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(path) = self.device_rect_path(x, y, w, h) else { return };
        let mut paint = Paint::default();
        paint.set_color_rgba8(0, 0, 0, 0);
        paint.blend_mode = BlendMode::Source;
        paint.anti_alias = false;
        let mask = self.state.clip.clone();
        self.pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), mask.as_deref());
        self.touch();
    }
    /// Draw another raster (an image or canvas), premultiplied RGBA, the
    /// source rectangle to the destination rectangle (user space).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image(&mut self, source: &[u8], sw: u32, sh: u32, sx: f32, sy: f32, sw2: f32, sh2: f32, dx: f32, dy: f32, dw: f32, dh: f32) {
        let Some(src) = tiny_skia::PixmapRef::from_bytes(source, sw, sh) else { return };
        if sw2 <= 0. || sh2 <= 0. || dw == 0. || dh == 0. {
            return;
        }
        // Map the source rectangle onto the destination rectangle.
        let scale = Transform::from_scale(dw / sw2, dh / sh2);
        let local = Transform::from_translate(dx, dy).pre_concat(scale).pre_concat(Transform::from_translate(-sx, -sy));
        let transform = self.state.transform.pre_concat(local);
        let pattern = tiny_skia::Pattern::new(
            src,
            tiny_skia::SpreadMode::Pad,
            if self.state.smoothing { tiny_skia::FilterQuality::Bilinear } else { tiny_skia::FilterQuality::Nearest },
            self.state.alpha,
            transform,
        );
        let paint = Paint { shader: pattern, blend_mode: self.state.blend, anti_alias: self.state.smoothing, ..Default::default() };
        let Some(path) = self.device_rect_path(dx, dy, dw, dh) else { return };
        let mask = self.state.clip.clone();
        self.pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), mask.as_deref());
        self.touch();
    }
    pub fn pixels(&self) -> &[u8] {
        self.pixmap.data()
    }
    /// Straight (unpremultiplied) RGBA of a rectangle, as getImageData gives it.
    pub fn image_data(&self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data();
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let (px, py) = (x + col, y + row);
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    continue;
                }
                let i = ((py * pw + px) * 4) as usize;
                let o = ((row * w as i32 + col) * 4) as usize;
                let a = data[i + 3];
                if a == 0 {
                    continue;
                }
                for c in 0..3 {
                    out[o + c] = ((u32::from(data[i + c]) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
                }
                out[o + 3] = a;
            }
        }
        out
    }
    /// Straight RGBA written at a point, ignoring the transform and clip.
    pub fn put_image_data(&mut self, rgba: &[u8], w: u32, h: u32, x: i32, y: i32) {
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let (px, py) = (x + col, y + row);
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    continue;
                }
                let i = ((row * w as i32 + col) * 4) as usize;
                let Some(src) = rgba.get(i..i + 4) else { continue };
                let o = ((py * pw + px) * 4) as usize;
                let a = u32::from(src[3]);
                for c in 0..3 {
                    data[o + c] = ((u32::from(src[c]) * a + 127) / 255) as u8;
                }
                data[o + 3] = src[3];
            }
        }
        self.touch();
    }
    fn touch(&mut self) {
        self.dirty = true;
        self.touched = true;
    }
    pub fn set_line_width(&mut self, w: f32) {
        if w.is_finite() && w > 0. {
            self.state.line_width = w;
        }
    }
    pub fn set_line_cap(&mut self, cap: &str) {
        self.state.cap = match cap {
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => LineCap::Butt,
        };
    }
    pub fn set_line_join(&mut self, join: &str) {
        self.state.join = match join {
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
    }
    pub fn set_miter_limit(&mut self, m: f32) {
        if m.is_finite() && m > 0. {
            self.state.miter = m;
        }
    }
    pub fn set_dash(&mut self, dash: Vec<f32>, offset: f32) {
        self.state.dash = dash;
        self.state.dash_offset = offset;
    }
    pub fn set_alpha(&mut self, a: f32) {
        if a.is_finite() {
            self.state.alpha = a.clamp(0., 1.);
        }
    }
    pub fn set_smoothing(&mut self, on: bool) {
        self.state.smoothing = on;
    }
    pub fn set_composite(&mut self, op: &str) {
        self.state.blend = match op {
            "source-over" => BlendMode::SourceOver,
            "source-in" => BlendMode::SourceIn,
            "source-out" => BlendMode::SourceOut,
            "source-atop" => BlendMode::SourceAtop,
            "destination-over" => BlendMode::DestinationOver,
            "destination-in" => BlendMode::DestinationIn,
            "destination-out" => BlendMode::DestinationOut,
            "destination-atop" => BlendMode::DestinationAtop,
            "lighter" => BlendMode::Plus,
            "copy" => BlendMode::Source,
            "xor" => BlendMode::Xor,
            "multiply" => BlendMode::Multiply,
            "screen" => BlendMode::Screen,
            "overlay" => BlendMode::Overlay,
            "darken" => BlendMode::Darken,
            "lighten" => BlendMode::Lighten,
            "color-dodge" => BlendMode::ColorDodge,
            "color-burn" => BlendMode::ColorBurn,
            "hard-light" => BlendMode::HardLight,
            "soft-light" => BlendMode::SoftLight,
            "difference" => BlendMode::Difference,
            "exclusion" => BlendMode::Exclusion,
            "hue" => BlendMode::Hue,
            "saturation" => BlendMode::Saturation,
            "color" => BlendMode::Color,
            "luminosity" => BlendMode::Luminosity,
            _ => BlendMode::SourceOver,
        };
    }
    /// A fill or stroke style: a CSS colour, or a gradient's id.
    pub fn set_style(&mut self, fill: bool, value: StyleValue) {
        let style = match value {
            StyleValue::Color(text) => match parse_color(&text) {
                Some(c) => Style::Color(c),
                None => return,
            },
            StyleValue::Gradient(id) => Style::Gradient(id),
        };
        if fill {
            self.state.fill = style;
        } else {
            self.state.stroke = style;
        }
    }
    pub fn linear_gradient(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> u32 {
        self.gradients.push(Gradient { kind: GradientKind::Linear { x0, y0, x1, y1 }, stops: Vec::new() });
        self.gradients.len() as u32 - 1
    }
    #[allow(clippy::too_many_arguments)]
    pub fn radial_gradient(&mut self, x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32) -> u32 {
        self.gradients.push(Gradient { kind: GradientKind::Radial { x0, y0, r0, x1, y1, r1 }, stops: Vec::new() });
        self.gradients.len() as u32 - 1
    }
    pub fn add_color_stop(&mut self, id: u32, offset: f32, color: &str) {
        if let Some(g) = self.gradients.get_mut(id as usize)
            && let Some(c) = parse_color(color)
            && offset.is_finite()
        {
            g.stops.push((offset.clamp(0., 1.), c));
            g.stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
}
pub enum StyleValue {
    Color(String),
    Gradient(u32),
}
/// A CSS colour as the engine parses it, to straight sRGB bytes.
pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let syntax = css::Syntax::parse(Arc::from(text.trim()), Default::default(), None).ok()?;
    match color::Value::parse(syntax.components(), Default::default(), None).ok()? {
        color::Value::Absolute(c) => Some(c.srgb8_clipped()),
        color::Value::CurrentColor => Some([0, 0, 0, 255]),
        _ => None,
    }
}
