use super::*;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceId(std::num::NonZeroU64);
impl FaceId {
    pub fn value(self) -> u64 {
        self.0.get()
    }
}
pub(super) enum Bytes {
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
    /// Bytes someone else holds, such as a file the caller mapped read-only
    /// (the OS pages in the tables that are used; nothing is copied).
    Leased(Arc<dyn AsRef<[u8]> + Send + Sync>),
}
impl Bytes {
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Owned(v) => v,
            Self::Shared(v) => v,
            Self::Leased(l) => (**l).as_ref(),
        }
    }
    /// Heap bytes retained; a lease holds none of its own.
    fn reserved(&self) -> usize {
        match self {
            Self::Owned(v) => v.capacity(),
            Self::Shared(v) => v.len() + 2 * size_of::<usize>(),
            Self::Leased(_) => 2 * size_of::<usize>(),
        }
    }
}
pub(super) struct Source {
    pub bytes: Bytes,
}
impl AsRef<[u8]> for Source {
    fn as_ref(&self) -> &[u8] {
        self.bytes.data()
    }
}
/// Installed-face metadata. Names include localized typographic/legacy family
/// aliases; subfamily/full/PostScript names are not mistaken for families.
#[derive(Clone)]
pub struct Face {
    pub families: Box<[Arc<str>]>,
    pub weight: Span,
    pub width: Span,
    pub slopes: Slopes,
    pub axes: Box<[Axis]>,
    pub units_per_em: u16,
    pub monospaced: bool,
    /// `unicode-range` of a web face: the code points it is used for.
    pub ranges: Option<Box<[(u32, u32)]>>,
    pub(super) source: usize,
    pub(super) collection_index: u32,
    pub(super) zero: Option<u16>,
    pub(super) water: Option<u16>,
}
/// What a `@font-face` rule says about its font, over what the file says.
#[derive(Clone, Debug)]
pub struct Descriptor {
    pub family: Arc<str>,
    pub weight: Span,
    pub width: Span,
    pub slopes: Slopes,
    pub ranges: Option<Box<[(u32, u32)]>>,
}
impl Face {
    pub fn collection_index(&self) -> u32 {
        self.collection_index
    }
    /// Whether a web face covers this code point (an installed face covers all).
    pub fn in_range(&self, c: char) -> bool {
        self.ranges.as_ref().is_none_or(|ranges| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32))))
    }
    pub(super) fn axis(&self, tag: &[u8; 4]) -> Option<Axis> {
        self.axes.iter().copied().find(|a| &a.tag == tag)
    }
}
/// Frozen catalog. Sources share across collection faces; selected leases keep
/// only their source alive, not the whole catalog or an open file descriptor.
pub struct Catalog {
    pub(super) id: u32,
    pub(super) sources: Box<[Arc<Source>]>,
    pub(super) faces: Box<[Face]>,
    pub(super) families: HashMap<Arc<str>, Vec<u32>>,
    pub(super) limits: Limits,
    payload: usize,
}
impl Catalog {
    pub fn faces(&self) -> impl Iterator<Item = (FaceId, &Face)> {
        self.faces.iter().enumerate().map(|(i, f)| (self.face_id(i), f))
    }
    pub fn face(&self, id: FaceId) -> Result<&Face> {
        if id.value() >> 32 != self.id as u64 {
            return Err(Error::Stale);
        }
        self.faces.get(id.value() as u32 as usize).ok_or(Error::Stale)
    }
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
    /// Referenced bytes/capacities, source data shared once. Includes Arc
    /// controls, excludes allocator/hash controls; not process RSS.
    pub fn reserved_payload_bytes(&self) -> usize {
        self.payload
    }
    pub(super) fn face_id(&self, index: usize) -> FaceId {
        FaceId(std::num::NonZeroU64::new((self.id as u64) << 32 | index as u64).unwrap())
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadProgress {
    pub bytes: usize,
    pub faces: usize,
}
pub struct Builder {
    limits: Limits,
    sources: Vec<Arc<Source>>,
    faces: Vec<Face>,
    bytes: usize,
}
impl Builder {
    pub fn new(limits: Limits) -> Self {
        Self { limits, sources: Vec::new(), faces: Vec::new(), bytes: 0 }
    }
    pub fn add_owned(&mut self, data: Vec<u8>, cancelled: Option<&AtomicBool>) -> Result<usize> {
        self.add(Bytes::Owned(data), cancelled)
    }
    pub fn add_shared(&mut self, data: Arc<[u8]>, cancelled: Option<&AtomicBool>) -> Result<usize> {
        self.add(Bytes::Shared(data), cancelled)
    }
    /// Start from an existing catalog's faces and sources (shared, not copied).
    pub fn from_catalog(catalog: &Catalog) -> Self {
        Self { limits: catalog.limits, sources: catalog.sources.to_vec(), faces: catalog.faces.to_vec(), bytes: catalog.payload }
    }
    /// A web font: the descriptor names the family and the ranges it answers
    /// for, over the file's own names. WOFF data is decoded first by the
    /// caller; the first face of a collection is the one used.
    pub fn add_face(&mut self, data: Arc<[u8]>, descriptor: Descriptor, cancelled: Option<&AtomicBool>) -> Result<()> {
        let before = self.faces.len();
        self.add(Bytes::Shared(data), cancelled)?;
        let Some(face) = self.faces.get_mut(before) else { return Err(Error::Invalid) };
        face.families = Box::new([descriptor.family]);
        // A variable font's own range is only used within the descriptor's.
        face.weight = descriptor.weight;
        face.width = descriptor.width;
        face.slopes = descriptor.slopes;
        face.ranges = descriptor.ranges;
        self.faces.truncate(before + 1);
        Ok(())
    }
    /// Bytes the caller keeps alive, such as a file it mapped read-only
    /// (system fonts: many files, few used). Faces are indexed from them;
    /// `file_bytes` still bounds the file.
    pub fn add_leased(&mut self, data: Arc<dyn AsRef<[u8]> + Send + Sync>, cancelled: Option<&AtomicBool>) -> Result<usize> {
        self.add(Bytes::Leased(data), cancelled)
    }
    /// Native bounded file I/O; call on a worker. Reads at most 64 KiB between
    /// cancel/progress checks. No source bytes are copied after loading.
    pub fn load_file(&mut self, path: &std::path::Path, mut progress: impl FnMut(LoadProgress), cancelled: Option<&AtomicBool>) -> Result<usize> {
        check(cancelled)?;
        let mut file = std::fs::File::open(path).map_err(Error::Io)?;
        let length = usize::try_from(file.metadata().map_err(Error::Io)?.len()).map_err(|_| Error::Limit)?;
        if length > self.limits.file_bytes || length > self.limits.retained_bytes.saturating_sub(self.bytes) {
            return Err(Error::Limit);
        }
        check(cancelled)?;
        let mut data = Vec::with_capacity(length);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            check(cancelled)?;
            let n = match file.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            };
            if n == 0 {
                break;
            }
            if data.len().saturating_add(n) > self.limits.file_bytes || data.len().saturating_add(n) > self.limits.retained_bytes.saturating_sub(self.bytes) {
                return Err(Error::Limit);
            }
            data.try_reserve_exact(n).map_err(|_| Error::Limit)?;
            if data.capacity() > self.limits.file_bytes || data.capacity() > self.limits.retained_bytes.saturating_sub(self.bytes) {
                return Err(Error::Limit);
            }
            data.extend_from_slice(&chunk[..n]);
            progress(LoadProgress { bytes: data.len(), faces: 0 });
        }
        let bytes = data.len();
        let faces = self.add_owned(data, cancelled)?;
        progress(LoadProgress { bytes, faces });
        Ok(faces)
    }
    fn add(&mut self, data: Bytes, cancelled: Option<&AtomicBool>) -> Result<usize> {
        check(cancelled)?;
        let bytes = data.data();
        if bytes.len() > self.limits.file_bytes || data.reserved() > self.limits.retained_bytes.saturating_sub(self.bytes) {
            return Err(Error::Limit);
        }
        let count = preflight(bytes, self.limits, cancelled)?;
        let available = self.limits.retained_bytes.saturating_sub(self.bytes.saturating_add(data.reserved()));
        if self.faces.len().saturating_add(count) > self.limits.faces || count.saturating_mul(size_of::<Face>()) > available {
            return Err(Error::Limit);
        }
        let mut faces = Vec::with_capacity(count);
        let mut metadata = 0usize;
        let mut work = 0usize;
        for index in 0..count {
            check(cancelled)?;
            let parsed = ttf_parser::Face::parse(bytes, index as u32).map_err(|_| Error::Invalid)?;
            let mut names = Vec::<Arc<str>>::new();
            let records = parsed.names();
            if records.len() as usize > self.limits.names {
                return Err(Error::Limit);
            }
            for name in records {
                check(cancelled)?;
                work = work.saturating_add(1);
                if work > self.limits.work {
                    return Err(Error::Limit);
                }
                if !matches!(name.name_id, 1 | 16) {
                    continue;
                }
                if name.name.len() > self.limits.name_bytes {
                    return Err(Error::Limit);
                }
                // Charge decoding and duplicate comparisons before allocating;
                // repeated TTC aliases must not bypass the per-load work cap.
                work = work.saturating_add(name.name.len().saturating_mul(names.len().saturating_add(1)));
                if work > self.limits.work {
                    return Err(Error::Limit);
                }
                let decoded = if name.is_unicode() {
                    name.to_string()
                } else if name.platform_id == ttf_parser::PlatformId::Macintosh && name.encoding_id == 0 {
                    Some(encoding_rs::MACINTOSH.decode_without_bom_handling(name.name).0.into_owned())
                } else {
                    None
                };
                if let Some(name) = decoded {
                    if name.is_empty() {
                        continue;
                    }
                    if name.len() > self.limits.name_bytes {
                        return Err(Error::Limit);
                    }
                    if !names.iter().any(|n| n.as_ref() == name) {
                        metadata = metadata.saturating_add(name.len() + 2 * size_of::<usize>() + size_of::<Arc<str>>());
                        names.push(Arc::from(name));
                    }
                }
                if metadata > available {
                    return Err(Error::Limit);
                }
            }
            if names.is_empty() {
                return Err(Error::Unsupported("font family name encoding"));
            }
            let mut axes = Vec::new();
            if parsed.variation_axes().len() as usize > self.limits.axes {
                return Err(Error::Limit);
            }
            if parsed.variation_axes().len() >= 64 {
                return Err(Error::Unsupported("64 or more variation axes"));
            }
            for axis in parsed.variation_axes() {
                check(cancelled)?;
                work = work.saturating_add(axes.len().saturating_add(1));
                if work > self.limits.work {
                    return Err(Error::Limit);
                }
                let range = Span { min: axis.min_value as f64, max: axis.max_value as f64 };
                let default = axis.def_value as f64;
                if ![range.min, range.max, default].into_iter().all(f64::is_finite) || !range.contains(default) {
                    return Err(Error::Invalid);
                }
                let tag = axis.tag.to_bytes();
                if axes.iter().any(|a: &Axis| a.tag == tag) {
                    return Err(Error::Invalid);
                }
                axes.push(Axis { tag, range, default });
            }
            let axis = |tag: &[u8; 4]| axes.iter().find(|a| &a.tag == tag).copied();
            let raw_weight = axis(b"wght").map_or(Span::point(parsed.weight().to_number() as f64), |a| a.range);
            let weight = Span { min: raw_weight.min.clamp(1., 1000.), max: raw_weight.max.clamp(1., 1000.) };
            let width = axis(b"wdth").map_or(Span::point([50., 62.5, 75., 87.5, 100., 112.5, 125., 150., 200.][parsed.width().to_number().saturating_sub(1) as usize]), |a| a.range);
            if width.min < 0. {
                return Err(Error::Unsupported("font weight/width axis range"));
            }
            let mut slopes = Slopes { oblique: None, italic: None };
            if let Some(a) = axis(b"slnt") {
                slopes.oblique = Some(Span { min: -a.range.max, max: -a.range.min });
            }
            if let Some(a) = axis(b"ital") {
                slopes.italic = Some(a.range);
                if a.range.contains(0.) && slopes.oblique.is_none() {
                    slopes.oblique = Some(Span::point(0.));
                }
            }
            // ttf-parser's is_italic also returns true for any nonzero post
            // angle, including explicitly oblique faces. Keep OS/2 oblique
            // distinct from a labeled italic.
            if slopes.italic.is_none() && !parsed.is_oblique() && parsed.is_italic() {
                slopes.italic = Some(Span::point(if parsed.italic_angle() > 0. { -1. } else { 1. }));
            }
            if slopes.italic.is_none() && slopes.oblique.is_none() {
                let angle = -f64::from(parsed.italic_angle());
                slopes.oblique = Some(Span::point(if parsed.is_oblique() { if angle == 0. { 14. } else { angle } } else { 0. }));
            }
            metadata = metadata.saturating_add(size_of::<Face>() + axes.len() * size_of::<Axis>());
            if metadata > available {
                return Err(Error::Limit);
            }
            let reference = |c| parsed.glyph_index(c).filter(|g| g.0 != 0).map(|g| g.0);
            faces.push(Face {
                families: names.into_boxed_slice(),
                weight,
                width,
                slopes,
                axes: axes.into_boxed_slice(),
                units_per_em: parsed.units_per_em(),
                monospaced: parsed.is_monospaced(),
                ranges: None,
                source: self.sources.len(),
                collection_index: index as u32,
                zero: reference('0'),
                water: reference('水'),
            });
        }
        let retained = self.bytes.saturating_add(data.reserved()).saturating_add(metadata).saturating_add(size_of::<Source>() + 2 * size_of::<usize>());
        if retained > self.limits.retained_bytes {
            return Err(Error::Limit);
        }
        check(cancelled)?;
        self.sources.push(Arc::new(Source { bytes: data }));
        self.faces.extend(faces);
        self.bytes = retained;
        Ok(count)
    }
    /// Remove installed faces (those without ranges, from the original catalog)
    /// whose family a web face declares, so `@font-face` wins by name.
    pub(super) fn shadow(&mut self, families: &[String], cancelled: Option<&AtomicBool>) -> Result<()> {
        if families.is_empty() {
            return Ok(());
        }
        let limits = self.limits;
        let mut keep = Vec::with_capacity(self.faces.len());
        for face in &self.faces {
            check(cancelled)?;
            let web = face.families.len() == 1 && families.iter().any(|f| casefold::fold(&face.families[0], limits, cancelled).map(|n| n.as_ref() == f).unwrap_or(false));
            let shadowed = !web && face.families.iter().any(|n| casefold::fold(n, limits, cancelled).map(|n| families.iter().any(|f| f == n.as_ref())).unwrap_or(false));
            keep.push(!shadowed);
        }
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        Ok(())
    }
    pub fn finish(self, cancelled: Option<&AtomicBool>) -> Result<Arc<Catalog>> {
        check(cancelled)?;
        let mut families = HashMap::<Arc<str>, Vec<u32>>::new();
        let mut payload = self.bytes;
        for (i, face) in self.faces.iter().enumerate() {
            for name in &face.families {
                check(cancelled)?;
                let folded = casefold::fold(name, self.limits, cancelled)?;
                if !families.contains_key(folded.as_ref()) {
                    payload = payload.saturating_add(folded.len() + 2 * size_of::<usize>());
                }
                let entries = families.entry(Arc::from(folded.as_ref())).or_default();
                if entries.last() != Some(&(i as u32)) {
                    entries.push(i as u32);
                }
            }
            if payload > self.limits.retained_bytes {
                return Err(Error::Limit);
            }
        }
        payload = payload
            .saturating_add(self.sources.len() * size_of::<Arc<Source>>())
            .saturating_add(families.capacity() * size_of::<(Arc<str>, Vec<u32>)>())
            .saturating_add(families.values().map(|v| v.capacity() * size_of::<u32>()).sum::<usize>());
        if payload > self.limits.retained_bytes {
            return Err(Error::Limit);
        }
        let id = u32::try_from(identity()?).map_err(|_| Error::Limit)?;
        Ok(Arc::new(Catalog { id, sources: self.sources.into_boxed_slice(), faces: self.faces.into_boxed_slice(), families, limits: self.limits, payload }))
    }
}
fn u16_at(data: &[u8], at: usize) -> Result<usize> {
    Ok(u16::from_be_bytes(data.get(at..at + 2).ok_or(Error::Invalid)?.try_into().unwrap()) as usize)
}
fn u32_at(data: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_be_bytes(data.get(at..at + 4).ok_or(Error::Invalid)?.try_into().unwrap()) as usize)
}
fn preflight(data: &[u8], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<usize> {
    if data.len() < 12 {
        return Err(Error::Invalid);
    }
    if matches!(&data[..4], b"wOFF" | b"wOF2") {
        return Err(Error::Unsupported("WOFF decoding"));
    }
    let collection = &data[..4] == b"ttcf";
    let count = if collection { u32_at(data, 8)? } else { 1 };
    if count == 0 || count > limits.collection_faces {
        return Err(Error::Limit);
    }
    let mut work = 0usize;
    for index in 0..count {
        check(cancelled)?;
        let at = if collection { u32_at(data, 12 + index * 4)? } else { 0 };
        let tables = u16_at(data, at.checked_add(4).ok_or(Error::Limit)?)?;
        if tables > limits.tables {
            return Err(Error::Limit);
        }
        let mut previous = None;
        for i in 0..tables {
            check(cancelled)?;
            work = work.saturating_add(1);
            if work > limits.work {
                return Err(Error::Limit);
            }
            let record = at.checked_add(12 + i * 16).ok_or(Error::Limit)?;
            let tag = u32_at(data, record)?;
            if previous.is_some_and(|p| p >= tag) {
                return Err(Error::Invalid);
            }
            previous = Some(tag);
            let offset = u32_at(data, record + 8)?;
            let length = u32_at(data, record + 12)?;
            if offset.checked_add(length).is_none_or(|end| end > data.len()) {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(count)
}
