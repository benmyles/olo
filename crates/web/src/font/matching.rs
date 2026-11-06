use super::*;
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinate {
    pub tag: [u8; 4],
    pub value: f32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Synthesis {
    pub bold: bool,
    pub shear_degrees: Option<f64>,
}
#[derive(Clone, Copy, Debug)]
pub struct MatchOptions {
    pub synthesize_weight: bool,
    pub synthesize_style: bool,
    pub optical_sizing: bool,
}
impl Default for MatchOptions {
    fn default() -> Self {
        Self { synthesize_weight: true, synthesize_style: true, optical_sizing: true }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slope {
    Oblique(f64),
    Italic(f64),
}
/// Selected file face plus actual variation coordinates. Data leases survive
/// catalog/cache eviction. Synthesis is a request to a future shaper/rasterizer;
/// this module never geometrically stretches glyphs or claims to draw them.
#[derive(Clone)]
pub struct Selection {
    pub face: FaceId,
    pub width: f64,
    pub weight: f64,
    pub slope: Slope,
    pub coordinates: SmallVec<[Coordinate; 5]>,
    pub synthesis: Synthesis,
    pub(super) source: Arc<catalog::Source>,
    pub(super) index: u32,
}
impl Selection {
    pub fn data(&self) -> &[u8] {
        self.source.bytes.data()
    }
    pub fn collection_index(&self) -> u32 {
        self.index
    }
    pub(super) fn parsed(&self) -> Result<ttf_parser::Face<'_>> {
        let mut face = ttf_parser::Face::parse(self.data(), self.index).map_err(|_| Error::Invalid)?;
        for coordinate in &self.coordinates {
            face.set_variation(ttf_parser::Tag::from_bytes(&coordinate.tag), coordinate.value).ok_or(Error::Invalid)?;
        }
        Ok(face)
    }
    /// Cmap coverage only. This is not shaping/cluster suitability.
    pub fn glyph_id(&self, c: char) -> Result<Option<u16>> {
        Ok(self.parsed()?.glyph_index(c).map(|g| g.0))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rank(u8, f64);
struct Candidate {
    rank: (Rank, Rank, Rank),
    index: usize,
    width: f64,
    weight: f64,
    slope: Slope,
    shear: Option<f64>,
}
impl Rank {
    fn cmp(self, rhs: Self) -> std::cmp::Ordering {
        self.0.cmp(&rhs.0).then_with(|| self.1.total_cmp(&rhs.1))
    }
}
fn width(range: Span, target: f64) -> (Rank, f64) {
    if range.contains(target) {
        return (Rank(0, 0.), target);
    }
    if range.max < target { (Rank(if target <= 100. { 1 } else { 2 }, target - range.max), range.max) } else { (Rank(if target > 100. { 1 } else { 2 }, range.min - target), range.min) }
}
fn weight(range: Span, target: f64) -> (Rank, f64) {
    if range.contains(target) {
        return (Rank(0, 0.), target);
    }
    if (400. ..=500.).contains(&target) {
        if range.max < target {
            return (Rank(2, target - range.max), range.max);
        }
        return (Rank(if range.min <= 500. { 1 } else { 3 }, range.min - target), range.min);
    }
    if range.max < target { (Rank(if target < 400. { 1 } else { 2 }, target - range.max), range.max) } else { (Rank(if target > 500. { 1 } else { 2 }, range.min - target), range.min) }
}
fn mirrored(range: Span, sign: f64) -> Span {
    if sign > 0. { range } else { Span { min: -range.max, max: -range.min } }
}
fn positive(range: Span) -> Option<Span> {
    (range.max > 0.).then_some(Span { min: range.min.max(f64::MIN_POSITIVE), max: range.max })
}
fn near(range: Span, target: f64, above_first: bool, base: u8) -> (Rank, f64) {
    if range.contains(target) {
        return (Rank(base, 0.), target);
    }
    if range.max < target { (Rank(base + if above_first { 1 } else { 0 }, target - range.max), range.max) } else { (Rank(base + if above_first { 0 } else { 1 }, range.min - target), range.min) }
}
fn slope(face: &Face, target: FontSlant, options: MatchOptions) -> Result<(Rank, Slope, Option<f64>)> {
    let mut best: Option<(Rank, Slope, Option<f64>)> = None;
    let mut add = |rank: Rank, slope: Slope, shear| {
        if best.is_none_or(|v| rank.cmp(v.0).is_lt()) {
            best = Some((rank, slope, shear));
        }
    };
    match target {
        FontSlant::Left | FontSlant::Right => return Err(Error::Unsupported("font-style left/right face matching")),
        FontSlant::Normal => {
            if let Some(r) = face.slopes.oblique {
                if r.max >= 0. {
                    let value = r.min.max(0.);
                    add(Rank(0, value), Slope::Oblique(value), None);
                } else {
                    add(Rank(2, -r.max), Slope::Oblique(r.max), None);
                }
            }
            if let Some(r) = face.slopes.italic {
                if r.max >= 0. {
                    let value = r.min.max(0.);
                    add(Rank(1, value), Slope::Italic(value), None);
                } else {
                    add(Rank(3, -r.max), Slope::Italic(r.max), None);
                }
            }
        }
        FontSlant::Italic => {
            if let Some(r) = face.slopes.italic {
                if let Some(r) = positive(r) {
                    let (rank, value) = near(r, 1., true, 0);
                    add(rank, Slope::Italic(value), None);
                }
                if r.min <= 0. {
                    let value = r.max.min(0.);
                    add(Rank(4, -value), Slope::Italic(value), None);
                }
            }
            if let Some(r) = face.slopes.oblique {
                if let Some(r) = positive(r) {
                    let (rank, value) = near(r, 11., true, 2);
                    add(rank, Slope::Oblique(value), None);
                }
                if r.min <= 0. {
                    let value = r.max.min(0.);
                    add(Rank(5, -value), Slope::Oblique(value), options.synthesize_style.then_some(14. - value));
                }
            }
        }
        FontSlant::Oblique(angle) => {
            let sign = if angle < 0. { -1. } else { 1. };
            let target = angle.abs();
            let above = target >= 11.;
            if let Some(original) = face.slopes.oblique {
                let range = mirrored(original, sign);
                if range.contains(target) {
                    add(Rank(0, 0.), Slope::Oblique(angle), None);
                } else if let Some(r) = positive(range) {
                    let (rank, value) = near(r, target, above, 1);
                    add(rank, Slope::Oblique(value * sign), None);
                }
                if range.min <= 0. {
                    let value = range.max.min(0.) * sign;
                    if let Some(axis) = face.axis(b"slnt") {
                        let value = -axis.range.clamp(-angle);
                        add(Rank(3, 0.), Slope::Oblique(value), None);
                    } else if options.synthesize_style {
                        add(Rank(3, 0.), Slope::Oblique(value), Some(angle - value));
                    }
                    add(Rank(6, (range.max.min(0.)).abs()), Slope::Oblique(value), None);
                }
            }
            if let Some(original) = face.slopes.italic {
                let range = mirrored(original, sign);
                if let Some(r) = positive(range) {
                    let (rank, value) = near(r, 1., above, 4);
                    add(rank, Slope::Italic(value * sign), None);
                }
                if range.min <= 0. {
                    let value = range.max.min(0.);
                    add(Rank(7, -value), Slope::Italic(value * sign), None);
                }
            }
        }
    }
    best.ok_or(Error::Invalid)
}
pub(super) fn validate(request: &FontRequest) -> Result<()> {
    if ![request.size, request.weight, request.width].into_iter().all(f64::is_finite)
        || request.size < 0.
        || request.width < 0.
        || !(1. ..=1000.).contains(&request.weight)
        || matches!(request.slant,FontSlant::Oblique(a) if !(-90. ..=90.).contains(&a))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
impl Catalog {
    /// Match an installed family, narrowing width, then style, then weight.
    /// Full default Unicode folding is independent of language and normalization.
    pub fn select_family(&self, name: &str, request: &FontRequest, options: MatchOptions, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        validate(request)?;
        let folded = casefold::fold(name, self.limits, cancelled)?;
        let Some(indices) = self.families.get(folded.as_ref()) else { return Ok(None) };
        let mut best: Option<Candidate> = None;
        for (work, &index) in indices.iter().enumerate() {
            check(cancelled)?;
            if work >= self.limits.work {
                return Err(Error::Limit);
            }
            let face = &self.faces[index as usize];
            let (wr, width) = width(face.width, request.width);
            let (sr, slope, shear) = slope(face, request.slant, options)?;
            let (gr, weight) = weight(face.weight, request.weight);
            let rank = (wr, sr, gr);
            let better = best.as_ref().is_none_or(|b| rank.0.cmp(b.rank.0).then_with(|| rank.1.cmp(b.rank.1)).then_with(|| rank.2.cmp(b.rank.2)).is_lt());
            if better {
                best = Some(Candidate { rank, index: index as usize, width, weight, slope, shear });
            }
        }
        let Some(Candidate { index, width, weight, slope, shear, .. }) = best else { return Ok(None) };
        let face = &self.faces[index];
        let mut coordinates = SmallVec::new();
        for axis in &face.axes {
            check(cancelled)?;
            let value = match &axis.tag {
                b"wght" => weight,
                b"wdth" => width,
                b"slnt" => {
                    if let Slope::Oblique(a) = slope {
                        -a
                    } else {
                        axis.default
                    }
                }
                b"ital" => {
                    if let Slope::Italic(a) = slope {
                        a
                    } else {
                        0.
                    }
                }
                // Chrome's automatic optical sizing uses the CSS-pixel font
                // size, observed with loaded variable fixture fonts. Keep the
                // UA policy explicit; never assume a px-to-point conversion.
                b"opsz" if options.optical_sizing => request.size,
                _ => continue,
            };
            coordinates.push(Coordinate { tag: axis.tag, value: axis.range.clamp(value) as f32 });
        }
        Ok(Some(Selection {
            face: self.face_id(index),
            width,
            weight,
            slope,
            coordinates,
            synthesis: Synthesis { bold: options.synthesize_weight && request.weight >= 600. && weight < 600., shear_degrees: shear },
            source: self.sources[face.source].clone(),
            index: face.collection_index,
        }))
    }
}
