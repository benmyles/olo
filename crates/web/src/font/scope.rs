use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
};

#[derive(Clone)]
struct Binding {
    generic: GenericFamily,
    language: Arc<str>,
    range: Option<(char, char)>,
    names: Box<[Arc<str>]>,
}
/// Explicit trusted UA policy. No environment/locale guessed from the process.
/// Complete generics fall back to the configured default families; incomplete
/// and script-specific generics can be absent and allow the next CSS family.
#[derive(Clone)]
pub struct GenericPolicy {
    default: Box<[Arc<str>]>,
    bindings: Vec<Binding>,
    /// Named families answered by others when absent (metric-compatible
    /// substitutes, as fontconfig's aliases): lower-cased name → families.
    aliases: Vec<(String, Box<[Arc<str>]>)>,
}
impl GenericPolicy {
    fn complete(generic: GenericFamily) -> bool {
        matches!(generic, GenericFamily::Serif | GenericFamily::SansSerif | GenericFamily::Monospace | GenericFamily::Cursive | GenericFamily::Fantasy | GenericFamily::SystemUi | GenericFamily::Math)
    }
    pub fn new(default: &[&str]) -> Result<Self> {
        Ok(Self { default: names(default)?, bindings: Vec::new(), aliases: Vec::new() })
    }
    /// Families to try for `name` when no installed family has it.
    pub fn alias(&mut self, name: &str, families: &[&str]) -> Result<()> {
        if self.aliases.len() >= 1024 || name.is_empty() || name.len() > 256 {
            return Err(Error::Limit);
        }
        let key = name.to_ascii_lowercase();
        let families = names(families)?;
        match self.aliases.iter_mut().find(|(k, _)| *k == key) {
            Some((_, f)) => *f = families,
            None => self.aliases.push((key, families)),
        }
        Ok(())
    }
    fn aliases_of(&self, name: &str) -> &[Arc<str>] {
        self.aliases.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map_or(&[], |(_, f)| f)
    }
    pub fn set(&mut self, generic: GenericFamily, language: &str, range: Option<(char, char)>, families: &[&str]) -> Result<()> {
        if self.bindings.len() >= 1024 || range.is_some_and(|(a, b)| a > b) {
            return Err(Error::Limit);
        }
        let language = language_key(language)?;
        let names = names(families)?;
        self.bindings.retain(|b| !(b.generic == generic && b.language.as_ref() == language && b.range == range));
        self.bindings.push(Binding { generic, language: Arc::from(language), range, names });
        Ok(())
    }
    fn get(&self, generic: GenericFamily, language: &str, c: char) -> &[Arc<str>] {
        let binding = self
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.generic == generic && language_matches(language, &b.language) && b.range.is_none_or(|(a, b)| (a..=b).contains(&c)))
            .max_by_key(|(i, b)| (b.language.len(), b.range.is_some(), *i));
        if let Some((_, b)) = binding {
            return &b.names;
        }
        if Self::complete(generic) { &self.default } else { &[] }
    }
}
fn names(input: &[&str]) -> Result<Box<[Arc<str>]>> {
    if input.is_empty() || input.len() > 64 || input.iter().any(|s| s.is_empty() || s.len() > 4096) {
        return Err(Error::Invalid);
    }
    Ok(input.iter().map(|s| Arc::from(*s)).collect())
}
fn language_key(input: &str) -> Result<String> {
    if input.len() > 255 {
        return Err(Error::Limit);
    }
    if !input.is_empty() && input.split('-').any(|part| part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Err(Error::Invalid);
    }
    Ok(input.to_ascii_lowercase())
}
fn language_matches(language: &str, prefix: &str) -> bool {
    prefix.is_empty() || language == prefix || language.strip_prefix(prefix).is_some_and(|s| s.starts_with('-'))
}
pub struct FontSet {
    pub(super) catalog: Arc<Catalog>,
    policy: GenericPolicy,
    pub(super) options: MatchOptions,
    revision: u64,
}
impl FontSet {
    pub fn new(catalog: Arc<Catalog>, policy: GenericPolicy, options: MatchOptions) -> Result<Arc<Self>> {
        if catalog.faces.is_empty() {
            return Err(Error::Missing);
        }
        let mut found = false;
        for name in &policy.default {
            let name = casefold::fold(name, catalog.limits, None)?;
            found |= catalog.families.contains_key(name.as_ref());
        }
        if !found {
            return Err(Error::Missing);
        }
        Ok(Arc::new(Self { catalog, policy, options, revision: identity()? }))
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }
    /// Reuse a Scope (or clone it) for the same complete language/font context.
    /// A newly created scope gets a conservative distinct identity; scopes are
    /// worker-local and their bounded metric caches are not copied by Clone.
    pub fn scope(self: &Arc<Self>, language: &str) -> Result<Scope> {
        Ok(Scope { set: self.clone(), language: Arc::from(language_key(language)?), context: identity()?, cache: RefCell::new(Cache::default()), hits: Cell::new(0), misses: Cell::new(0) })
    }
    pub fn policy(&self) -> &GenericPolicy {
        &self.policy
    }
}
impl Scope {
    /// This scope's fonts plus a page's web faces, as a new scope with its own
    /// identity. Web families shadow installed ones of the same name.
    pub fn with_faces(&self, faces: Vec<(Arc<[u8]>, super::Descriptor)>, cancelled: Option<&AtomicBool>) -> Result<Scope> {
        let catalog = &self.set.catalog;
        let mut builder = Builder::from_catalog(catalog);
        let mut shadowed = Vec::new();
        for (data, descriptor) in faces {
            check(cancelled)?;
            let folded = casefold::fold(&descriptor.family, catalog.limits, cancelled)?.into_owned();
            if let Err(e) = builder.add_face(data, descriptor, cancelled) {
                match e {
                    Error::Cancelled | Error::Limit => return Err(e),
                    _ => continue, // A broken file: that face is not available, as in other browsers.
                }
            }
            shadowed.push(folded);
        }
        builder.shadow(&shadowed, cancelled)?;
        let set = FontSet::new(builder.finish(cancelled)?, self.set.policy.clone(), self.set.options)?;
        set.scope(&self.language)
    }
}
pub struct Scope {
    pub(super) set: Arc<FontSet>,
    language: Arc<str>,
    context: u64,
    cache: RefCell<Cache>,
    hits: Cell<usize>,
    misses: Cell<usize>,
}
impl Clone for Scope {
    fn clone(&self) -> Self {
        Self { set: self.set.clone(), language: self.language.clone(), context: self.context, cache: RefCell::new(Cache::default()), hits: Cell::new(0), misses: Cell::new(0) }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
    pub entries: usize,
    pub conservative_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    families: usize,
    numbers: [u64; 4],
    kind: u8,
    axes: [u8; 3],
}
impl Key {
    fn new(r: &FontRequest) -> Self {
        let (kind, slope) = match r.slant {
            FontSlant::Normal => (0, 0.),
            FontSlant::Italic => (1, 0.),
            FontSlant::Left => (2, 0.),
            FontSlant::Right => (3, 0.),
            FontSlant::Oblique(a) => (4, a),
        };
        let bits = |v: f64| if v == 0. { 0 } else { v.to_bits() };
        Self {
            families: r.families.identity(),
            numbers: [bits(r.size), bits(r.weight), bits(r.width), bits(slope)],
            kind,
            axes: [r.axes.writing_mode as u8, r.axes.direction as u8, r.axes.text_orientation as u8],
        }
    }
}
struct Cached {
    metrics: super::metrics::ResolvedMetrics,
    _families: crate::css::properties::Families,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    values: HashMap<Key, Cached>,
    order: VecDeque<Key>,
    bytes: usize,
}
impl Scope {
    pub fn context(&self) -> u64 {
        self.context
    }
    pub fn revision(&self) -> u64 {
        self.set.revision
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn cache_stats(&self) -> CacheStats {
        let c = self.cache.borrow();
        CacheStats {
            hits: self.hits.get(),
            misses: self.misses.get(),
            entries: c.values.len(),
            conservative_bytes: c.bytes + c.values.capacity() * size_of::<(Key, Cached)>() + c.order.capacity() * size_of::<Key>(),
        }
    }
    pub fn metrics(&self, request: &FontRequest, cancelled: Option<&AtomicBool>) -> Result<super::metrics::ResolvedMetrics> {
        check(cancelled)?;
        matching::validate(request)?;
        let key = Key::new(request);
        if let Some(c) = self.cache.borrow().values.get(&key) {
            self.hits.set(self.hits.get().saturating_add(1));
            return Ok(c.metrics);
        }
        self.misses.set(self.misses.get().saturating_add(1));
        let metrics = self.resolve_metrics(request, cancelled)?;
        check(cancelled)?;
        let limits = self.set.catalog.limits;
        let bytes = request.families.reserved_bytes() + size_of::<(Key, Cached)>() * 4 + size_of::<Key>() * 2;
        if limits.cache_entries != 0 && bytes <= limits.cache_bytes {
            let mut cache = self.cache.borrow_mut();
            while cache.values.len() >= limits.cache_entries || cache.bytes.saturating_add(bytes) > limits.cache_bytes {
                let Some(old) = cache.order.pop_front() else { break };
                if let Some(old) = cache.values.remove(&old) {
                    cache.bytes = cache.bytes.saturating_sub(old.bytes);
                }
            }
            cache.bytes += bytes;
            cache.order.push_back(key);
            cache.values.insert(key, Cached { metrics, _families: request.families.clone(), bytes });
        }
        Ok(metrics)
    }
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.set.catalog
    }
    /// The family's face for this request, and for this character among
    /// faces split by `unicode-range`.
    fn family(&self, name: &str, r: &FontRequest, scalar: char, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        self.set.catalog.select_family_for(name, r, self.set.options, Some(scalar), cancelled)
    }
    fn selection(&self, r: &FontRequest, scalar: char, coverage: bool, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        let catalog = &self.set.catalog;
        let supported = |s: &Selection| -> Result<bool> {
            if !coverage {
                return Ok(true);
            }
            let f = catalog.face(s.face)?;
            if !f.in_range(scalar) {
                return Ok(false);
            }
            Ok(match scalar {
                '0' => f.zero.is_some(),
                '水' => f.water.is_some(),
                _ => s.glyph_id(scalar)?.is_some_and(|g| g != 0),
            })
        };
        self.choose(r, scalar, coverage, supported, cancelled)
    }
    fn choose(&self, r: &FontRequest, scalar: char, coverage: bool, supported: impl Fn(&Selection) -> Result<bool>, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        matching::validate(r)?;
        let catalog = &self.set.catalog;
        for family in r.families.as_slice() {
            check(cancelled)?;
            match family {
                Family::Named(name) => {
                    if let Some(s) = self.family(name, r, scalar, cancelled)? {
                        if supported(&s)? {
                            return Ok(Some(s));
                        }
                    } else {
                        for alias in self.set.policy.aliases_of(name) {
                            if let Some(s) = self.family(alias, r, scalar, cancelled)?
                                && supported(&s)?
                            {
                                return Ok(Some(s));
                            }
                        }
                    }
                }
                Family::Generic(generic) => {
                    let mut available = false;
                    for name in self.set.policy.get(*generic, &self.language, scalar) {
                        if let Some(s) = self.family(name, r, scalar, cancelled)? {
                            available = true;
                            if supported(&s)? {
                                return Ok(Some(s));
                            }
                        }
                    }
                    // An unavailable complete generic still maps to the UA
                    // default before advancing to the next CSS family. A
                    // present face lacking this glyph advances normally.
                    if !available && GenericPolicy::complete(*generic) {
                        for name in &self.set.policy.default {
                            if let Some(s) = self.family(name, r, scalar, cancelled)?
                                && supported(&s)?
                            {
                                return Ok(Some(s));
                            }
                        }
                    }
                }
            }
        }
        for name in &self.set.policy.default {
            check(cancelled)?;
            if let Some(s) = self.family(name, r, scalar, cancelled)?
                && supported(&s)?
            {
                return Ok(Some(s));
            }
        }
        if coverage {
            // Deterministic installed fallback; a family is matched once, not
            // by selecting an arbitrary wrong-width/weight face with coverage.
            let mut seen = HashSet::new();
            for (work, face) in catalog.faces.iter().enumerate() {
                check(cancelled)?;
                if work >= catalog.limits.work {
                    return Err(Error::Limit);
                }
                let name = &face.families[0];
                let key = casefold::fold(name, catalog.limits, cancelled)?;
                if !seen.insert(key.into_owned()) {
                    continue;
                }
                if let Some(s) = self.family(name, r, scalar, cancelled)?
                    && supported(&s)?
                {
                    return Ok(Some(s));
                }
            }
        }
        Ok(None)
    }
    pub fn first_available(&self, r: &FontRequest, cancelled: Option<&AtomicBool>) -> Result<Selection> {
        self.selection(r, ' ', false, cancelled)?.ok_or(Error::Missing)
    }
    /// Whole-grapheme cmap coverage for the text worker, with strict UVS first
    /// and an explicit relaxed retry at the caller. Not full script/GSUB
    /// suitability: a font may map scalars and still lack required shaping.
    pub(crate) fn cluster_selection(&self, r: &FontRequest, text: &str, strict_variations: bool, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        use crate::text::unicode::{ignorable, variation_selector};
        let mut scalar = ' ';
        for c in text.chars() {
            check(cancelled)?;
            if !ignorable(c) {
                scalar = c;
                break;
            }
        }
        let work = Cell::new(0usize);
        self.choose(
            r,
            scalar,
            true,
            |s| {
                let metadata = self.set.catalog.face(s.face)?;
                let face = s.parsed()?;
                let mut previous = None;
                for c in text.chars() {
                    check(cancelled)?;
                    work.set(work.get().saturating_add(1));
                    if work.get() > self.set.catalog.limits.work {
                        return Err(Error::Limit);
                    }
                    if !ignorable(c) && !metadata.in_range(c) {
                        return Ok(false);
                    }
                    if variation_selector(c) {
                        if strict_variations && previous.is_some_and(|base| face.glyph_variation_index(base, c).is_none_or(|g| g.0 == 0)) {
                            return Ok(false);
                        }
                    } else if !ignorable(c) && face.glyph_index(c).is_none_or(|g| g.0 == 0) {
                        return Ok(false);
                    }
                    previous = Some(c);
                }
                Ok(true)
            },
            cancelled,
        )
    }
    /// Single-codepoint cmap fallback, not shaping/cluster suitability. The
    /// shaper must perform script/cluster/variation-selector matching separately.
    pub fn cmap_selection(&self, r: &FontRequest, c: char, cancelled: Option<&AtomicBool>) -> Result<Option<Selection>> {
        self.selection(r, c, true, cancelled)
    }
}
