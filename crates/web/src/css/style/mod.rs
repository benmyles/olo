//! Frozen computed styles over the implemented property registry. Worker-only;
//! no layout, CSSOM, font selection/shaping, UA stylesheet or live invalidation.
mod compute;
mod context;
mod coupled;
mod font_pool;
mod job;
mod memory;
use super::{Syntax, cascade, color, math, properties as p, variables};
pub use cascade::{Axes, Direction, TextOrientation, WritingMode};
pub use font_pool::FontCacheStats;
pub use job::{Job, JobLimits, JobOptions, Progress, Report, StyleKey, StyleTree};
pub use memory::Memory;
pub use p::{Display, Families, Family, GenericFamily, Inner, Internal, Keyword, Outer, Property};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    ContextUnavailable,
    Stale,
    Limit,
    Cancelled,
    Incomplete,
    Failed,
    Dom(crate::dom::Error),
    Font(crate::font::Error),
    Css(super::Error),
    Cascade(cascade::Error),
    Property(p::Error),
    Numeric(math::Error),
    Color(color::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS style {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    fn numeric(error: math::Error) -> Self {
        match error {
            math::Error::Cancelled => Self::Cancelled,
            math::Error::Limit | math::Error::DepthLimit | math::Error::TypeLimit => Self::Limit,
            math::Error::ContextUnavailable => Self::ContextUnavailable,
            error => Self::Numeric(error),
        }
    }
    fn property(error: p::Error) -> Self {
        match error {
            p::Error::Cancelled | p::Error::Variables(variables::Error::Cancelled) => Self::Cancelled,
            p::Error::Limit | p::Error::DepthLimit | p::Error::Variables(variables::Error::Limit | variables::Error::DepthLimit) => Self::Limit,
            p::Error::Numeric(error) => Self::numeric(error),
            p::Error::Unsupported(why) => Self::Unsupported(why),
            error => Self::Property(error),
        }
    }
    fn color(error: color::Error) -> Self {
        match error {
            color::Error::Cancelled => Self::Cancelled,
            color::Error::Limit | color::Error::DepthLimit => Self::Limit,
            color::Error::ContextUnavailable => Self::ContextUnavailable,
            error => Self::Color(error),
        }
    }
    fn css(error: super::Error) -> Self {
        match error {
            super::Error::Cancelled => Self::Cancelled,
            super::Error::InputLimit | super::Error::TokenLimit | super::Error::DecodedLimit | super::Error::DepthLimit | super::Error::DiagnosticLimit | super::Error::ItemLimit => Self::Limit,
        }
    }
    fn cascade(error: cascade::Error) -> Self {
        match error {
            cascade::Error::Cancelled | cascade::Error::Variables(variables::Error::Cancelled) | cascade::Error::Selectors(super::selectors::Error::Cancelled) => Self::Cancelled,
            cascade::Error::Limit | cascade::Error::Variables(variables::Error::Limit | variables::Error::DepthLimit) => Self::Limit,
            cascade::Error::Selectors(super::selectors::Error::NumberLimit | super::selectors::Error::CompileLimit | super::selectors::Error::MatchLimit | super::selectors::Error::DepthLimit) => {
                Self::Limit
            }
            cascade::Error::Property(error) => Self::property(error),
            cascade::Error::Unsupported(why) => Self::Unsupported(why),
            error => Self::Cascade(error),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub properties: usize,
    pub custom_properties: usize,
    pub retained_bytes: usize,
    pub work: usize,
    pub math: math::Limits,
    pub color: color::Limits,
}
impl Default for Limits {
    fn default() -> Self {
        Self { properties: 256, custom_properties: 65536, retained_bytes: 16 * 1024 * 1024, work: 4_000_000, math: Default::default(), color: Default::default() }
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
/// Stable, already-selected font metrics in CSS pixels. Host selection and
/// caching run outside the UI thread; this module never guesses missing fonts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub id: u64,
    pub ex: f64,
    pub cap: f64,
    pub ch: f64,
    pub ic: f64,
    pub normal_line_height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontSlant {
    Normal,
    Italic,
    Left,
    Right,
    Oblique(f64),
}
#[derive(Clone, Debug, PartialEq)]
pub struct FontRequest {
    pub size: f64,
    pub weight: f64,
    /// Percentage; this is a face-selection request, never geometric stretching.
    pub width: f64,
    pub slant: FontSlant,
    pub families: Families,
    pub axes: Axes,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub request: FontRequest,
    pub metrics: FontMetrics,
    context: u64,
}
impl Font {
    pub fn context(&self) -> u64 {
        self.context
    }
    fn new(host: &impl Host, request: FontRequest) -> Result<Self> {
        if !request.size.is_finite()
            || request.size.is_sign_negative()
            || !(1. ..=1000.).contains(&request.weight)
            || !request.width.is_finite()
            || request.width.is_sign_negative()
            || matches!(request.slant, FontSlant::Oblique(angle) if !(-90. ..=90.).contains(&angle))
        {
            return Err(Error::ContextUnavailable);
        }
        let context = host.font_context();
        let metrics = host.font(&request)?;
        if host.font_context() != context {
            return Err(Error::Stale);
        }
        if [request.size, request.weight, metrics.ex, metrics.cap, metrics.ch, metrics.ic, metrics.normal_line_height].into_iter().any(|v| !v.is_finite() || v.is_sign_negative()) {
            return Err(Error::ContextUnavailable);
        }
        Ok(Self { request, metrics, context })
    }
}
/// All callbacks belong to one immutable generation. Viewport/container/tree
/// callbacks are bound to the subject element and enforce its tree scope.
pub trait Host {
    fn revision(&self) -> u64;
    /// Document-wide computed environment. Called once when Defaults freezes
    /// this generation; a changed environment requires a changed revision.
    fn environment(&self) -> Result<Option<Arc<variables::Environment>>> {
        Ok(None)
    }
    fn scheme(&self) -> color::Scheme;
    fn system_color(&self, color: color::SystemColor, scheme: color::Scheme) -> color::Result<color::Color>;
    fn font(&self, request: &FontRequest) -> Result<FontMetrics>;
    /// Stable selection context within this generation, including language,
    /// generic-family policy and any subject-specific font inputs. Equal CSS
    /// requests can share selected metrics only when this identity also agrees.
    fn font_context(&self) -> u64;
    fn font_size(&self, keyword: Keyword, parent_size: f64) -> Result<f64>;
    /// Device pixels per CSS pixel, including page zoom. The revision changes
    /// with this scale; computed border widths must be snapped for it.
    fn device_scale(&self) -> f64;
    fn factor(&self, _unit: math::RelativeUnit, _root_axes: Option<Axes>) -> math::Result<f64> {
        Err(math::Error::ContextUnavailable)
    }
    fn tree_count(&self, _kind: math::TreeCount) -> math::Result<u32> {
        Err(math::Error::ContextUnavailable)
    }
}
impl<H: Host + ?Sized> Host for &H {
    fn revision(&self) -> u64 {
        (**self).revision()
    }
    fn environment(&self) -> Result<Option<Arc<variables::Environment>>> {
        (**self).environment()
    }
    fn scheme(&self) -> color::Scheme {
        (**self).scheme()
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        (**self).system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> Result<FontMetrics> {
        (**self).font(r)
    }
    fn font_context(&self) -> u64 {
        (**self).font_context()
    }
    fn font_size(&self, k: Keyword, parent: f64) -> Result<f64> {
        (**self).font_size(k, parent)
    }
    fn device_scale(&self) -> f64 {
        (**self).device_scale()
    }
    fn factor(&self, u: math::RelativeUnit, a: Option<Axes>) -> math::Result<f64> {
        (**self).factor(u, a)
    }
    fn tree_count(&self, k: math::TreeCount) -> math::Result<u32> {
        (**self).tree_count(k)
    }
}
#[derive(Clone, Debug)]
pub enum Value {
    Keyword(Keyword),
    Numeric(p::Numeric),
    Color(color::Computed),
    Display(Display),
    FitContent(p::Numeric),
    Families(Families),
    LocalUrl(p::LocalUrl),
    Inset(p::Inset),
    Tracks(p::Tracks),
    GridLine(p::GridLine),
    Radius(p::Radius),
    Images(p::Images<color::Computed>),
    Shadows(p::Shadows<color::Computed>),
    Sizes(p::Sizes),
    Positions(p::Positions),
    Repeats(p::Repeats),
    Transforms(p::Transforms),
    Features(p::Features),
    Content(p::Content),
    /// Explicit nonzero oblique angle in degrees; bare oblique is a keyword.
    Oblique(f64),
}
impl Value {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Keyword(a), Self::Keyword(b)) => a == b,
            (Self::Numeric(a), Self::Numeric(b)) | (Self::FitContent(a), Self::FitContent(b)) => a.same(b),
            (Self::Color(a), Self::Color(b)) => a.same(b),
            (Self::Display(a), Self::Display(b)) => a == b,
            (Self::Families(a), Self::Families(b)) => a == b,
            (Self::LocalUrl(a), Self::LocalUrl(b)) => a == b,
            (Self::Inset(a), Self::Inset(b)) => a.same(b),
            (Self::Tracks(a), Self::Tracks(b)) => a.same(b),
            (Self::GridLine(a), Self::GridLine(b)) => a == b,
            (Self::Radius(a), Self::Radius(b)) => a.same(b),
            (Self::Images(a), Self::Images(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Self::Shadows(a), Self::Shadows(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Self::Sizes(a), Self::Sizes(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Self::Positions(a), Self::Positions(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Self::Repeats(a), Self::Repeats(b)) => Arc::ptr_eq(&a.0, &b.0),
            (Self::Transforms(a), Self::Transforms(b)) => a.same(b),
            (Self::Features(a), Self::Features(b)) => Arc::ptr_eq(&a.0, &b.0) || a == b,
            (Self::Content(a), Self::Content(b)) => Arc::ptr_eq(&a.0, &b.0) || a == b,
            (Self::Oblique(a), Self::Oblique(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
    pub fn scalar(&self) -> Option<f64> {
        if let Self::Numeric(n) = self { n.expression().constant().map(|q| q.value) } else { None }
    }
    fn reserved_bytes(&self) -> usize {
        match self {
            Self::Numeric(n) | Self::FitContent(n) => n.expression().memory().accounted_reserved_bytes,
            Self::Families(f) => f.reserved_bytes(),
            Self::LocalUrl(u) => u.reserved_bytes(),
            Self::Inset(s) => s.reserved_bytes(),
            Self::Tracks(t) => t.reserved_bytes(),
            Self::GridLine(l) => l.reserved_bytes(),
            Self::Radius(r) => r.reserved_bytes(),
            Self::Images(i) => i.reserved_bytes(),
            Self::Shadows(s) => s.reserved_bytes(),
            Self::Sizes(s) => s.reserved_bytes(),
            Self::Positions(p) => p.reserved_bytes(),
            Self::Repeats(r) => r.reserved_bytes(),
            Self::Transforms(t) => t.reserved_bytes(),
            Self::Features(f) => f.reserved_bytes(),
            Self::Content(c) => c.reserved_bytes(),
            _ => 0,
        }
    }
}
#[derive(Clone, Debug)]
struct Entry {
    property: Property,
    value: Value,
}
#[derive(Clone)]
struct Custom {
    name: Arc<str>,
    value: variables::Computed,
}
/// An element's custom properties: what it declares, sorted by name, over
/// its parent's dictionary, shared rather than copied (a root's hundreds
/// of variables are then held once, not once per element declaring one).
struct Dictionary {
    parent: Option<Arc<Dictionary>>,
    values: Box<[Custom]>,
    /// This dictionary's own bytes (its parent's are its parent's).
    bytes: usize,
    /// How many names this and its parents hold together (an upper bound).
    len: usize,
}
impl Dictionary {
    fn get(&self, name: &str) -> Option<&variables::Computed> {
        let mut d = self;
        loop {
            if let Ok(i) = d.values.binary_search_by(|v| v.name.as_ref().cmp(name)) {
                return Some(&d.values[i].value);
            }
            d = d.parent.as_deref()?;
        }
    }
    /// This dictionary and its parents, nearest first.
    fn chain(&self) -> impl Iterator<Item = &Dictionary> {
        std::iter::successors(Some(self), |d| d.parent.as_deref())
    }
    /// Whether two dictionaries hold the same names with the same values,
    /// level by level (shared levels compare by pointer).
    fn same_as(&self, other: &Self) -> bool {
        let mut a = Some(self);
        let mut b = Some(other);
        loop {
            match (a, b) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if !std::ptr::eq(x, y) && (x.values.len() != y.values.len() || !x.values.iter().zip(y.values.iter()).all(|(p, q)| p.name == q.name && p.value.same(&q.value))) {
                        return false;
                    }
                    if std::ptr::eq(x, y) {
                        return true;
                    }
                    a = x.parent.as_deref();
                    b = y.parent.as_deref();
                }
                _ => return false,
            }
        }
    }
}
/// Typed initial declarations and their contextual computed baseline are
/// compiled once per host generation, shared by all its element styles.
pub struct Defaults {
    specified: Box<[p::Value]>,
    values: Box<[Entry]>,
    /// What the defaults' specified values depend on, all together: every
    /// style has some default, so every style has these.
    dependencies: Dependencies,
    positions: [u8; Property::ALL.len()],
    initial_font: Arc<Font>,
    device_scale: f64,
    revision: u64,
    environment: Option<Arc<variables::Environment>>,
    empty: Arc<[Entry]>,
    empty_customs: Arc<Dictionary>,
}
impl Defaults {
    /// Whether these defaults are the host's as it is now (its revision and
    /// device scale), so styles made with them can be kept.
    pub fn is_current(&self, host: &impl Host) -> bool {
        self.revision == host.revision() && self.device_scale == host.device_scale() && self.initial_font.context == host.font_context()
    }
    pub fn new(host: &impl Host, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Arc<Self>> {
        check(cancelled)?;
        if !host.device_scale().is_finite() || host.device_scale() <= 0. {
            return Err(Error::ContextUnavailable);
        }
        if Property::ALL.len() > limits.properties {
            return Err(Error::Limit);
        }
        let revision = host.revision();
        let font_context = host.font_context();
        let mut specified = Vec::with_capacity(Property::ALL.len());
        for &property in Property::ALL {
            check(cancelled)?;
            let syntax = Syntax::parse(Arc::from(property.initial_spelling()), Default::default(), cancelled).map_err(Error::css)?;
            let p::Declaration::Known { mut values, .. } = p::Declaration::parse(property.as_str(), syntax.components(), false, Default::default(), cancelled).map_err(Error::property)? else {
                return Err(Error::Invalid);
            };
            specified.push(values.remove(0).value);
        }
        let size = host.font_size(Keyword::Medium, 0.)?;
        let p::Value::Families(families) = &specified[Property::FontFamily as usize] else { return Err(Error::Invalid) };
        let initial_font = Arc::new(Font::new(host, FontRequest { size, weight: 400., width: 100., slant: FontSlant::Normal, families: families.clone(), axes: Axes::default() })?);
        let mut defaults = Self {
            dependencies: {
                let mut d = Dependencies::default();
                for v in &specified {
                    let s = v.dependencies();
                    d.substitution |= s.substitution;
                    d.color.current_color |= s.color.current_color;
                    d.color.system_colors |= s.color.system_colors;
                    d.color.scheme |= s.color.scheme;
                    d.color.math = d.color.math.union(s.color.math);
                }
                d
            },
            specified: specified.into_boxed_slice(),
            values: Box::new([]),
            positions: [u8::MAX; Property::ALL.len()],
            initial_font,
            device_scale: host.device_scale(),
            revision,
            environment: host.environment()?,
            empty: Arc::from([]),
            empty_customs: Arc::new(Dictionary { parent: None, values: Box::new([]), bytes: 0, len: 0 }),
        };
        let cascade = cascade::Builder::new(Default::default(), cancelled).finish(false, None).map_err(Error::cascade)?;
        let computed = compute::values(&defaults, &cascade, None, None, None, host, Options::default(), limits, cancelled)?;
        defaults.values = computed.values.into_boxed_slice();
        for (i, entry) in defaults.values.iter().enumerate() {
            defaults.positions[entry.property as usize] = i as u8;
        }
        if host.revision() != revision || host.font_context() != font_context || host.device_scale() != defaults.device_scale {
            return Err(Error::Stale);
        }
        check(cancelled)?;
        Ok(Arc::new(defaults))
    }
    fn value(&self, physical: Property) -> &Value {
        &self.values[self.positions[physical as usize] as usize].value
    }
}
#[derive(Clone, Copy, Debug)]
pub struct BoxParent {
    pub display: Display,
    pub axes: Axes,
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub root: bool,
    /// The nearest box-tree parent (possibly skipping display:contents).
    /// This is distinct from the flattened inheritance parent.
    pub layout_parent: Option<BoxParent>,
    pub allow_animation_tainted: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self { root: false, layout_parent: None, allow_animation_tainted: true }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Dependencies {
    pub inherited: bool,
    pub substitution: bool,
    pub color: color::Dependencies,
    pub layout_parent: bool,
    /// Declarations whose value could not be computed in this context and
    /// were dropped as invalid, leaving the inherited or initial value.
    pub fallbacks: u16,
}
/// A coherent generation's computed values; logical aliases are queried using
/// this element's axes. No parent/cascade/source tree is retained. Sparse
/// overrides and unchanged fonts/custom dictionaries share immutable storage.
#[derive(Clone)]
pub struct Style {
    defaults: Arc<Defaults>,
    values: Arc<[Entry]>,
    customs: Arc<Dictionary>,
    axes: Axes,
    font: Arc<Font>,
    line_height: f64,
    dependencies: Dependencies,
    root: bool,
}
impl Style {
    /// Scale at which computed device-dependent values (such as borders) froze.
    pub fn device_scale(&self) -> f64 {
        self.defaults.device_scale
    }
    pub fn axes(&self) -> Axes {
        self.axes
    }
    pub fn font(&self) -> &Arc<Font> {
        &self.font
    }
    pub fn line_height(&self) -> f64 {
        self.line_height
    }
    pub fn dependencies(&self) -> Dependencies {
        self.dependencies
    }
    #[allow(clippy::too_many_arguments)] // Coherent parent/root/host snapshots and explicit worker bounds.
    pub fn compute(
        defaults: Arc<Defaults>,
        cascade: &cascade::Cascade<'_>,
        parent: Option<&Style>,
        root: Option<&Style>,
        host: &impl Host,
        options: Options,
        limits: Limits,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Self> {
        Self::compute_reusing(defaults, cascade, parent, root, None, None, host, options, limits, cancelled)
    }
    #[allow(clippy::too_many_arguments)]
    fn compute_reusing(
        defaults: Arc<Defaults>,
        cascade: &cascade::Cascade<'_>,
        parent: Option<&Style>,
        root: Option<&Style>,
        reuse: Option<&Style>,
        fonts: Option<&mut font_pool::FontPool>,
        host: &impl Host,
        options: Options,
        limits: Limits,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Self> {
        check(cancelled)?;
        let font_context = host.font_context();
        if options.root && font_context != defaults.initial_font.context {
            return Err(Error::Stale);
        }
        if host.revision() != defaults.revision
            || host.device_scale() != defaults.device_scale
            || parent.is_some_and(|p| !Arc::ptr_eq(&p.defaults, &defaults))
            || root.is_some_and(|r| !r.root || !Arc::ptr_eq(&r.defaults, &defaults))
        {
            return Err(Error::Stale);
        }
        if cascade.has_parent() != parent.is_some() || options.root && parent.is_some() {
            return Err(Error::Invalid);
        }
        let mut computed = compute::values(&defaults, cascade, parent, root, fonts, host, options, limits, cancelled)?;
        let clock = compute::TIMING.load(std::sync::atomic::Ordering::Relaxed).then(std::time::Instant::now);
        if !cfg!(feature = "baseline-style-dense") {
            computed.values.retain(|entry| !entry.value.same(defaults.value(entry.property)));
        }
        let matches = |s: &&Style| {
            Arc::ptr_eq(&s.defaults, &defaults) && s.values.len() == computed.values.len() && s.values.iter().zip(&computed.values).all(|(a, b)| a.property == b.property && a.value.same(&b.value))
        };
        let reuse = if cfg!(feature = "baseline-style-dense") { None } else { parent.filter(matches).or_else(|| reuse.filter(matches)) };
        let values = if !cfg!(feature = "baseline-style-dense") && computed.values.is_empty() {
            defaults.empty.clone()
        } else if !cfg!(feature = "baseline-style-dense") && reuse.is_some() {
            reuse.unwrap().values.clone()
        } else {
            Arc::from(computed.values)
        };
        if host.revision() != defaults.revision || host.font_context() != font_context || computed.font.context != font_context {
            return Err(Error::Stale);
        }
        if let Some(c) = clock {
            compute::PHASES[5].fetch_add(c.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        check(cancelled)?;
        Ok(Self { defaults, values, customs: computed.customs, axes: computed.axes, font: computed.font, line_height: computed.line_height, dependencies: computed.dependencies, root: options.root })
    }
    /// This style with one value replaced, sharing everything else.
    pub fn with_value(&self, property: Property, value: Value) -> Self {
        let physical = self.axes.physical(property);
        let mut values: Vec<Entry> = self.values.to_vec();
        match values.binary_search_by_key(&(physical as u16), |e| e.property as u16) {
            Ok(i) => values[i].value = value,
            Err(i) => values.insert(i, Entry { property: physical, value }),
        }
        Self {
            defaults: self.defaults.clone(),
            values: Arc::from(values),
            customs: self.customs.clone(),
            axes: self.axes,
            font: self.font.clone(),
            line_height: self.line_height,
            dependencies: self.dependencies,
            root: self.root,
        }
    }
    pub fn value(&self, property: Property) -> &Value {
        let physical = self.axes.physical(property);
        self.values.binary_search_by_key(&(physical as u16), |e| e.property as u16).ok().map_or_else(|| self.defaults.value(physical), |i| &self.values[i].value)
    }
    pub fn used_color(&self, property: Property) -> Result<color::Color> {
        let Value::Color(color::Computed::Absolute(current)) = self.value(Property::Color) else { return Err(Error::Invalid) };
        match self.value(property) {
            Value::Color(color) => Ok(color.used(*current)),
            Value::Keyword(Keyword::Auto) if property == Property::CaretColor => Ok(*current),
            _ => Err(Error::Invalid),
        }
    }
    /// A hidden/absent border has zero used width while retaining its computed
    /// width for inheritance. CSSOM's resolved width also uses this stage.
    pub fn used_border_width(&self, property: Property) -> Result<f64> {
        let style = match property {
            Property::BorderTopWidth => Property::BorderTopStyle,
            Property::BorderRightWidth => Property::BorderRightStyle,
            Property::BorderBottomWidth => Property::BorderBottomStyle,
            Property::BorderLeftWidth => Property::BorderLeftStyle,
            _ => return Err(Error::Invalid),
        };
        if matches!(self.value(style), Value::Keyword(Keyword::None | Keyword::Hidden)) {
            return Ok(0.);
        }
        self.value(property).scalar().ok_or(Error::Invalid)
    }
    /// Percent text spacing inherits intact and uses the subject's used font
    /// size. No font metric/layout default is substituted for that input.
    pub fn used_text_spacing(&self, property: Property, used_font_size: f64, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<f64> {
        check(cancelled)?;
        if !matches!(property, Property::LetterSpacing | Property::WordSpacing) {
            return Err(Error::Invalid);
        }
        if !used_font_size.is_finite() || used_font_size.is_sign_negative() {
            return Err(Error::ContextUnavailable);
        }
        struct Basis(f64);
        impl math::Context for Basis {
            fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
                if unit == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
            }
        }
        let Value::Numeric(n) = self.value(property) else { return Err(Error::Invalid) };
        n.resolve(&Basis(used_font_size), &mut Default::default(), limits, cancelled).map_err(Error::property)
    }
    /// Accounted unique entry payload, excluding shared defaults/fonts/custom
    /// dictionaries, Arc/allocator controls and shared numeric program storage.
    pub fn override_payload_bytes(&self) -> usize {
        self.values.len() * size_of::<Entry>()
    }
    pub fn defaults(&self) -> Arc<Defaults> {
        self.defaults.clone()
    }
    pub fn overrides_share(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.values, &other.values)
    }
    /// Whether two styles are the same in every value (so children styled
    /// against either compute the same): the sparse entries, the font,
    /// the custom properties, the axes.
    pub fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.defaults, &other.defaults)
            && self.axes == other.axes
            && self.line_height == other.line_height
            && (Arc::ptr_eq(&self.values, &other.values)
                || self.values.len() == other.values.len() && self.values.iter().zip(other.values.iter()).all(|(a, b)| a.property == b.property && a.value.same(&b.value)))
            && (Arc::ptr_eq(&self.font, &other.font) || self.font.request == other.font.request && self.font.context == other.font.context)
            && (Arc::ptr_eq(&self.customs, &other.customs) || self.customs.same_as(&other.customs))
    }
    pub fn custom(&self, name: &str) -> Option<variables::Computed> {
        self.customs.get(name).cloned()
    }
}
impl cascade::Inherited for Style {
    fn custom(&self, name: &str) -> Option<variables::Computed> {
        self.custom(name)
    }
}
