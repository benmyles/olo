//! Retained SVG path geometry and initial path-only viewport compilation.
//! Stroke/paint-server/nested SVG and full intrinsic sizing remain incomplete.
pub(crate) mod clip;
pub(crate) mod rect;
pub(crate) mod used;
mod viewport;
use std::sync::atomic::{AtomicBool, Ordering};
use svgtypes::{PathParser, PathSegment as Segment};
pub(crate) use viewport::Geometry;
pub use viewport::{Shape, Viewport};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub source_bytes: usize,
    /// Work actions: source commands plus generated arc cubics.
    pub segments: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { source_bytes: 1024 * 1024, segments: 65536 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Cancelled,
    Coordinates,
    Incomplete,
    Failed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SVG geometry {self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub segments: usize,
    pub finished: bool,
    /// SVG renders the valid command prefix of malformed path data. This flag
    /// preserves the parse diagnostic rather than claiming the input was valid.
    pub malformed: bool,
}
pub struct Path {
    geometry: Option<tiny_skia::Path>,
    malformed: bool,
}
impl Path {
    /// User-space vector data; transform it at paint time for the current
    /// viewport and device scale. Empty/move-only paths have no drawable data.
    pub fn geometry(&self) -> Option<&tiny_skia::Path> {
        self.geometry.as_ref()
    }
    pub fn malformed(&self) -> bool {
        self.malformed
    }
    pub(crate) fn from_geometry(geometry: Option<tiny_skia::Path>) -> Self {
        Self { geometry, malformed: false }
    }
    /// The same path in another coordinate space; geometry a transform cannot
    /// represent in f32 is dropped rather than drawn somewhere wrong.
    pub(crate) fn transformed(self, transform: Option<tiny_skia::Transform>) -> Self {
        match transform {
            Some(t) => Self { geometry: self.geometry.and_then(|g| g.transform(t)), malformed: self.malformed },
            None => self,
        }
    }
    /// Occupied verbs and points, excluding allocator capacity and headers.
    pub fn payload_bytes(&self) -> usize {
        self.geometry.as_ref().map_or(0, |p| std::mem::size_of_val(p.verbs()) + std::mem::size_of_val(p.points()))
    }
}
/// Borrows the retained DOM attribute. One action handles one source command
/// or generated arc cubic. svgtypes supplies grammar; our state machine owns
/// current points, closepath and smooth controls. Kurbo supplies arc geometry.
/// Parsing a single numeric token can scan up to the source-byte limit, so an
/// action is a work bound, not a hard wall-clock deadline. Run on a worker.
pub struct PathJob<'a> {
    parser: PathParser<'a>,
    current: (f64, f64),
    start: (f64, f64),
    cubic: Option<(f64, f64)>,
    quadratic: Option<(f64, f64)>,
    arc: Option<kurbo::ArcAppendIter>,
    builder: tiny_skia::PathBuilder,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    progress: Progress,
    failed: bool,
}
impl<'a> PathJob<'a> {
    pub fn new(source: &'a str, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self, Error> {
        check(cancel)?;
        if source.len() > limits.source_bytes {
            return Err(Error::Limit);
        }
        Ok(Self {
            parser: source.into(),
            current: (0., 0.),
            start: (0., 0.),
            cubic: None,
            quadratic: None,
            arc: None,
            builder: tiny_skia::PathBuilder::new(),
            limits,
            cancel,
            progress: Progress::default(),
            failed: false,
        })
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress, Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance(&mut self, budget: usize) -> Result<Progress, Error> {
        check(self.cancel)?;
        for _ in 0..budget {
            if self.progress.finished {
                break;
            }
            check(self.cancel)?;
            if let Some(arc) = self.arc.as_mut() {
                if let Some(kurbo::PathEl::CurveTo(a, b, c)) = arc.next() {
                    self.charge()?;
                    self.builder.cubic_to(n(a.x)?, n(a.y)?, n(b.x)?, n(b.y)?, n(c.x)?, n(c.y)?);
                    continue;
                }
                self.arc = None;
            }
            let segment = match self.parser.next() {
                None => {
                    self.progress.finished = true;
                    break;
                }
                Some(Err(_)) => {
                    self.progress.malformed = true;
                    self.progress.finished = true;
                    break;
                }
                Some(Ok(segment)) => segment,
            };
            self.charge()?;
            let old = self.current;
            let point = |abs: bool, x: f64, y: f64| if abs { (x, y) } else { (old.0 + x, old.1 + y) };
            let previous_cubic = self.cubic.take();
            let previous_quadratic = self.quadratic.take();
            match segment {
                Segment::MoveTo { abs, x, y } => {
                    self.current = point(abs, x, y);
                    self.start = self.current;
                    self.builder.move_to(n(self.current.0)?, n(self.current.1)?);
                }
                Segment::LineTo { abs, x, y } => self.line(point(abs, x, y))?,
                Segment::HorizontalLineTo { abs, x } => self.line((if abs { x } else { old.0 + x }, old.1))?,
                Segment::VerticalLineTo { abs, y } => self.line((old.0, if abs { y } else { old.1 + y }))?,
                Segment::CurveTo { abs, x1, y1, x2, y2, x, y } => {
                    self.curve(point(abs, x1, y1), point(abs, x2, y2), point(abs, x, y))?;
                }
                Segment::SmoothCurveTo { abs, x2, y2, x, y } => {
                    let a = previous_cubic.map_or(old, |p| (2. * old.0 - p.0, 2. * old.1 - p.1));
                    self.curve(a, point(abs, x2, y2), point(abs, x, y))?;
                }
                Segment::Quadratic { abs, x1, y1, x, y } => self.quad(point(abs, x1, y1), point(abs, x, y))?,
                Segment::SmoothQuadratic { abs, x, y } => {
                    let a = previous_quadratic.map_or(old, |p| (2. * old.0 - p.0, 2. * old.1 - p.1));
                    self.quad(a, point(abs, x, y))?;
                }
                Segment::ClosePath { .. } => {
                    self.builder.close();
                    self.current = self.start;
                }
                Segment::EllipticalArc { abs, rx, ry, x_axis_rotation, large_arc, sweep, x, y } => {
                    let end = point(abs, x, y);
                    if end == old {
                        continue;
                    }
                    if rx == 0. || ry == 0. {
                        self.line(end)?;
                        continue;
                    }
                    // Guard the endpoint-to-center calculation's products and
                    // divisions; unsupported extremes must never disappear.
                    for v in [old.0, old.1, end.0, end.1, rx, ry, x_axis_rotation] {
                        n(v)?;
                    }
                    if [old.0, old.1, end.0, end.1, rx, ry].into_iter().any(|v| v != 0. && v.abs() < 1e-30) {
                        return Err(Error::Coordinates);
                    }
                    let arc = kurbo::Arc::from_svg_arc(&kurbo::SvgArc {
                        from: (old.0, old.1).into(),
                        to: (end.0, end.1).into(),
                        radii: (rx, ry).into(),
                        x_rotation: x_axis_rotation.to_radians(),
                        large_arc,
                        sweep,
                    })
                    .ok_or(Error::Coordinates)?;
                    for v in [arc.center.x, arc.center.y, arc.radii.x, arc.radii.y, arc.start_angle, arc.sweep_angle] {
                        n(v)?;
                    }
                    // Initial user-space tolerance, NOT device-scale fidelity.
                    // Arc cubics are generated incrementally and quota bounded.
                    self.arc = Some(arc.append_iter(0.001));
                    self.current = end;
                }
            }
        }
        Ok(self.progress)
    }
    fn charge(&mut self) -> Result<(), Error> {
        if self.progress.segments >= self.limits.segments {
            return Err(Error::Limit);
        }
        self.progress.segments += 1;
        Ok(())
    }
    fn line(&mut self, p: (f64, f64)) -> Result<(), Error> {
        self.builder.line_to(n(p.0)?, n(p.1)?);
        self.current = p;
        Ok(())
    }
    fn curve(&mut self, a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> Result<(), Error> {
        self.builder.cubic_to(n(a.0)?, n(a.1)?, n(b.0)?, n(b.1)?, n(p.0)?, n(p.1)?);
        self.current = p;
        self.cubic = Some(b);
        Ok(())
    }
    fn quad(&mut self, a: (f64, f64), p: (f64, f64)) -> Result<(), Error> {
        self.builder.quad_to(n(a.0)?, n(a.1)?, n(p.0)?, n(p.1)?);
        self.current = p;
        self.quadratic = Some(a);
        Ok(())
    }
    pub fn finish(self) -> Result<Path, Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        let drawable = self.builder.len() > 1;
        let geometry = self.builder.finish();
        if drawable && geometry.is_none() {
            return Err(Error::Coordinates);
        }
        Ok(Path { geometry, malformed: self.progress.malformed })
    }
}
fn check(cancel: Option<&AtomicBool>) -> Result<(), Error> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}

fn n(v: f64) -> Result<f32, Error> {
    let value = v as f32;
    if value.is_finite() { Ok(value) } else { Err(Error::Coordinates) }
}
