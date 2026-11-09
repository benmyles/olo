//! Text-only min/max-content measures from retained shaping and break points.
//! Worker-only; creates no line fragments, glyph positions or paint output.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IntrinsicWidths {
    pub min_content: f64,
    pub max_content: f64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct IntrinsicProgress {
    pub work: usize,
    pub finished: bool,
}
/// Retain the result with the layout snapshot. Available-width changes do not
/// invalidate it. Initial scope: one horizontal LTR shaping item per hard line.
/// Box decorations and inline objects need separate sizing. Mixed script/font
/// items and bidi remain explicit until their item boundaries are represented.
pub struct IntrinsicJob<'a> {
    text: Arc<Text>,
    scale: Option<f64>,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    prefixes: Vec<f64>,
    runs: Vec<usize>,
    run: usize,
    glyph: usize,
    metric: LinearMetric,
    from: u32,
    origin: u32,
    leading_end: u32,
    scan: usize,
    widths: IntrinsicWidths,
    progress: IntrinsicProgress,
    maximum: bool,
    measured: bool,
    failed: bool,
    /// Lines may break between any two glyphs and those breaks count at
    /// min-content (`overflow-wrap: anywhere`, `word-break: break-word |
    /// break-all`; CSS Text 3 §5.2, §5.5): min-content is the widest glyph.
    anywhere: bool,
    widest: f64,
}
impl<'a> IntrinsicJob<'a> {
    pub fn new(text: Arc<Text>, scale: Option<f64>, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if scale.is_some_and(|s| !s.is_finite() || s <= 0.) {
            return Err(Error::Invalid);
        }
        if text.prepared.text().len() > limits.text_bytes || text.points.len() > limits.points {
            return Err(Error::Limit);
        }
        if text.object_count() != 0 {
            return Err(Error::Unsupported("intrinsic inline objects need box sizing"));
        }
        if text.prepared.text().contains('\u{ad}') {
            return Err(Error::Unsupported("discretionary hyphen intrinsic sizing"));
        }
        if text.paragraph.runs.iter().any(|r| r.direction != Direction::LeftToRight) {
            return Err(Error::Unsupported("bidi intrinsic item sizing"));
        }
        if text.paragraph.runs.windows(2).any(|pair| pair[0].range.end == pair[1].range.start) {
            return Err(Error::Unsupported("multi-run intrinsic item sizing"));
        }
        let mut glyphs: usize = text.paragraph.runs.len();
        for r in &text.paragraph.runs {
            check(cancel)?;
            glyphs = glyphs.checked_add(r.glyphs.len()).ok_or(Error::Limit)?;
            if glyphs > limits.clusters {
                return Err(Error::Limit);
            }
        }
        Ok(Self {
            text,
            scale,
            limits,
            cancel,
            prefixes: Vec::new(),
            runs: Vec::new(),
            run: 0,
            glyph: 0,
            metric: LinearMetric::default(),
            from: 0,
            origin: 0,
            leading_end: 0,
            scan: 0,
            widths: IntrinsicWidths::default(),
            progress: IntrinsicProgress::default(),
            maximum: false,
            measured: false,
            failed: false,
            anywhere: false,
            widest: 0.,
        })
    }
    /// See the field: breaks between any glyphs count at min-content.
    pub fn set_anywhere(&mut self, anywhere: bool) {
        self.anywhere = anywhere;
    }
    pub fn progress(&self) -> IntrinsicProgress {
        self.progress
    }
    /// Temporary numeric prefix payload; excludes capacity, headers and shared
    /// Text/fonts. Released at finish; the published result is two numbers.
    pub fn working_payload_bytes(&self) -> usize {
        self.prefixes.len() * size_of::<f64>() + self.runs.len() * size_of::<usize>()
    }
    pub fn step(&mut self, budget: usize) -> Result<IntrinsicProgress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn begin_line(&mut self) {
        let source = self.text.prepared.text();
        let spaces = source[self.from as usize..].bytes().take_while(|b| *b == b' ').count() as u32;
        self.leading_end = self.from + spaces;
        self.origin = if collapses(self.text.whitespace) { self.leading_end } else { self.from };
        self.from = self.origin;
    }
    fn position(&self, ri: usize, byte: u32) -> Result<f64> {
        let edge = self.text.index.retained_edge(&self.text.paragraph, ri, byte)?;
        Ok(self.prefixes[self.runs[ri] + edge])
    }
    fn snapped(&self, value: f64) -> f64 {
        self.scale.map_or(value, |s| (value * 64.).ceil() / (64. * s))
    }
    fn interval(&self, start: u32, end: u32, origin: u32, prefix_difference: bool) -> Result<f64> {
        if end <= start {
            return Ok(0.);
        }
        let paragraph = &self.text.paragraph;
        let first = paragraph.runs.partition_point(|r| r.range.end <= start);
        let mut width = 0.;
        for ri in first..paragraph.runs.len() {
            let run = &paragraph.runs[ri];
            if run.range.start >= end {
                break;
            }
            let start = start.max(run.range.start);
            let end = end.min(run.range.end);
            let origin = origin.max(run.range.start).min(start);
            let base = self.position(ri, origin)?;
            let a = self.position(ri, start)? - base;
            let b = self.position(ri, end)? - base;
            width += if prefix_difference { self.snapped(b) - self.snapped(a) } else { self.snapped(b - a) };
        }
        if width.is_finite() { Ok(width) } else { Err(Error::Invalid) }
    }
    fn advance(&mut self, budget: usize) -> Result<IntrinsicProgress> {
        check(self.cancel)?;
        for _ in 0..budget {
            if self.progress.finished {
                break;
            }
            check(self.cancel)?;
            self.progress.work = self.progress.work.checked_add(1).ok_or(Error::Limit)?;
            if self.progress.work > self.limits.work {
                return Err(Error::Limit);
            }
            if !self.measured {
                let Some(run) = self.text.paragraph.runs.get(self.run) else {
                    self.measured = true;
                    self.begin_line();
                    continue;
                };
                if self.glyph == 0 {
                    self.metric = LinearMetric::new(run.size, run.font.units_per_em(), self.scale)?;
                    self.runs.push(self.prefixes.len());
                    self.prefixes.push(0.);
                }
                if let Some(glyph) = run.glyphs.get(self.glyph) {
                    let raw = self.metric.advance(glyph.x_advance);
                    self.widest = self.widest.max(raw);
                    let next = self.prefixes.last().copied().unwrap() + raw;
                    if !next.is_finite() {
                        return Err(Error::Invalid);
                    }
                    self.prefixes.push(next);
                    self.glyph += 1;
                } else {
                    self.run += 1;
                    self.glyph = 0;
                }
                continue;
            }
            let Some(point) = self.text.points.get(self.scan).copied() else {
                if self.maximum {
                    self.progress.finished = true;
                } else {
                    self.maximum = true;
                    self.from = 0;
                    self.scan = 0;
                    self.begin_line();
                }
                continue;
            };
            self.scan += 1;
            if self.maximum && point.kind != Kind::Required {
                continue;
            }
            if point.offset <= self.from {
                continue;
            }
            let end = if point.hang != NONE && (point.kind == Kind::Allowed || !self.maximum) { self.text.hanging[point.hang as usize].byte } else { point.end.byte };
            let width = if self.maximum {
                if self.text.whitespace.wrap && self.leading_end > self.origin && self.leading_end < end {
                    self.interval(self.from, self.leading_end, self.from, false)? + self.interval(self.leading_end, end, self.leading_end, false)?
                } else {
                    self.interval(self.from, end, self.from, false)?
                }
            } else {
                self.interval(self.from, end, self.origin.max(self.leading_end.min(self.from)), true)?
            };
            let target = if self.maximum { &mut self.widths.max_content } else { &mut self.widths.min_content };
            *target = target.max(width);
            self.from = point.offset;
            if point.kind == Kind::Required {
                self.begin_line();
            } else if collapses(self.text.whitespace) {
                let source = self.text.prepared.text();
                self.from += source[self.from as usize..].bytes().take_while(|b| *b == b' ').count() as u32;
            }
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<IntrinsicWidths> {
        check(self.cancel)?;
        if self.failed {
            Err(Error::Failed)
        } else if !self.progress.finished {
            Err(Error::Incomplete)
        } else if self.anywhere {
            Ok(IntrinsicWidths { min_content: self.widths.min_content.min(self.snapped(self.widest)), ..self.widths })
        } else {
            Ok(self.widths)
        }
    }
}

// Linear scalable-font metric quantization, before layout-grid enclosure. Keep
// f32 centipixel arithmetic: rounding the product in f64 changes cache buckets.
#[derive(Default)]
struct LinearMetric {
    multiplier: f64,
    quantized: bool,
}
impl LinearMetric {
    fn new(size: f64, upem: u16, scale: Option<f64>) -> Result<Self> {
        let multiplier = if let Some(scale) = scale {
            let device_size = (size as f32) * (scale as f32);
            let effective = f64::from((device_size * 100.).floor() / 100.);
            let fixed_size = (effective * 64.).trunc();
            let fixed_scale = (fixed_size * 65536. / f64::from(upem)).round();
            fixed_scale / 64.
        } else {
            size / f64::from(upem)
        };
        if !multiplier.is_finite() {
            return Err(Error::Invalid);
        }
        Ok(Self { multiplier, quantized: scale.is_some() })
    }
    fn advance(&self, units: i32) -> f64 {
        let raw = f64::from(units) * self.multiplier;
        if self.quantized { raw.round() / 65536. } else { raw }
    }
}
