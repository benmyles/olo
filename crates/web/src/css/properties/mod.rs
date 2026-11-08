//! Typed property declarations, before cascade and layout. Worker-only APIs.
pub mod animation;
mod clip;
mod content;
mod decoration;
mod display;
mod flex;
mod font;
mod grid;
mod numeric;
mod parse;
mod registry;
mod transform;
use super::{Component, Components, Kind, Statement, StatementKind, color, math, variables};
pub use clip::{GeometryBox, Inset, LocalUrl};
pub use content::{Content, ContentPart};
pub use decoration::{Direction, Image, Images, Positions, RadialSize, Radius, RepeatAxis, Repeats, Shadow, Shadows, Size, Sizes, Stop};
pub use display::{Display, Inner, Internal, Outer};
pub use font::{Families, Family, Feature, Features, GenericFamily};
pub use grid::{AutoRepeat, Entry as TrackEntry, GridLine, Track, TrackSize, Tracks};
pub use numeric::{Numeric, NumericKind, Range};
pub use registry::{Keyword, Name, Property, Shorthand};
use smallvec::SmallVec;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub use transform::{Function as TransformFunction, IDENTITY, Transforms, apply as apply_transform, is_identity, multiply as multiply_transforms};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub nodes: usize,
    pub work: usize,
    pub depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { bytes: 1024 * 1024, nodes: 65536, work: 1_000_000, depth: 64 }
    }
}
impl Limits {
    fn variables(self) -> variables::Limits {
        variables::Limits { bytes: self.bytes, nodes: self.nodes, work: self.work, depth: self.depth, ..Default::default() }
    }
    fn math(self) -> math::Limits {
        math::Limits { instructions: self.nodes, work: self.work, depth: self.depth, stack: self.nodes.min(4096) }
    }
    fn color(self) -> color::Limits {
        color::Limits { nodes: self.nodes, work: self.work, depth: self.depth }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    UnknownProperty,
    Unsupported(&'static str),
    Limit,
    DepthLimit,
    Cancelled,
    Variables(variables::Error),
    Numeric(math::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS property {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn variable_error(e: variables::Error) -> Error {
    match e {
        variables::Error::Invalid => Error::Invalid,
        variables::Error::Limit => Error::Limit,
        variables::Error::DepthLimit => Error::DepthLimit,
        variables::Error::Cancelled => Error::Cancelled,
        e => Error::Variables(e),
    }
}
/// Declared token/numeric/color dependencies. Property-specific font, layout,
/// cascade and logical-axis dependencies still belong to the style host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub color: color::Dependencies,
    pub substitution: bool,
}
#[derive(Clone, Debug)]
pub enum Value {
    Wide(variables::Wide),
    Keyword(Keyword),
    Numeric(Numeric),
    Color(color::Value),
    Display(Display),
    FitContent(Numeric),
    Families(Families),
    LocalUrl(LocalUrl),
    Inset(Inset),
    Tracks(Tracks),
    GridLine(GridLine),
    Radius(Radius),
    Images(Images<color::Value>),
    Shadows(Shadows<color::Value>),
    Sizes(Sizes),
    Positions(Positions),
    Repeats(Repeats),
    Transforms(Transforms),
    Features(Features),
    Content(Content),
    Animation(animation::Animations),
    Pending(Arc<Pending>),
}

impl Value {
    pub fn dependencies(&self) -> Dependencies {
        match self {
            Self::Numeric(v) | Self::FitContent(v) => Dependencies { color: color::Dependencies { math: v.expression().dependencies(), ..Default::default() }, substitution: false },
            Self::Inset(v) => Dependencies { color: color::Dependencies { math: v.dependencies(), ..Default::default() }, substitution: false },
            Self::Color(v) => Dependencies { color: v.dependencies(), substitution: false },
            Self::Pending(_) => Dependencies { substitution: true, ..Default::default() },
            _ => Dependencies::default(),
        }
    }
    /// Heap storage owned by this value. Shared allocations must be counted
    /// once across clones. Pending values retain their complete token arena.
    pub fn reserved_bytes(&self) -> usize {
        match self {
            Self::Numeric(v) | Self::FitContent(v) => v.expression().memory().accounted_reserved_bytes,
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
            Self::Color(v) => v.expression_reserved_bytes(),
            Self::Pending(v) => size_of::<Pending>() + 2 * size_of::<usize>() + v.tokens.arena_reserved_bytes(),
            _ => 0,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Assignment {
    pub property: Property,
    pub value: Value,
}
pub type Assignments = SmallVec<[Assignment; 1]>;
/// All longhands of a pending shorthand share this single immutable allocation.
#[derive(Debug)]
pub struct Pending {
    name: Name,
    tokens: variables::Value,
}
impl Pending {
    pub fn name(&self) -> Name {
        self.name
    }
    pub fn tokens(&self) -> &variables::Value {
        &self.tokens
    }
    /// Resolve after winning longhands are selected. Invalid computed values
    /// become unset for every affected longhand; do not recascade older values.
    /// Unsupported features/resource/cancellation errors remain explicit.
    pub fn resolve<C: variables::Context>(&self, resolver: &mut variables::Resolver<'_, C>, allow_animation_tainted: bool, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
        check(cancelled)?;
        let computed = resolver.resolve(&self.tokens, allow_animation_tainted).map_err(variable_error)?;
        let Some(values) = computed.components() else {
            return Ok(self.name.wide(variables::Wide::Unset));
        };
        match parse::known(self.name, values, limits, cancelled) {
            Err(Error::Invalid) => Ok(self.name.wide(variables::Wide::Unset)),
            result => result,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Declaration {
    Known { important: bool, values: Assignments },
    Custom { name: Arc<str>, important: bool, value: variables::Value },
}
impl Declaration {
    pub fn compile(statement: &Statement<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        if statement.kind() != StatementKind::Declaration {
            return Err(Error::Invalid);
        }
        Self::parse(statement.name().ok_or(Error::Invalid)?, statement.value(), statement.important(), limits, cancelled)
    }
    pub fn parse(name: &str, values: Components<'_>, important: bool, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        check(cancelled)?;
        if name.len() > limits.bytes {
            return Err(Error::Limit);
        }
        if name.starts_with("--") && name.len() > 2 {
            let value = variables::Value::compile(values, limits.variables(), cancelled).map_err(variable_error)?;
            return Ok(Self::Custom { name: name.into(), important, value });
        }
        if inert(name) {
            // Accepted and dropped: nothing a static rendering shows.
            return Ok(Self::Known { important, values: Assignments::new() });
        }
        let name = Name::parse(name).ok_or(Error::UnknownProperty)?;
        parse::preflight(values.clone(), limits, cancelled)?;
        // `content: attr(x)` is the legacy attr(): parsed as content's own
        // syntax, not deferred as an arbitrary substitution (CSS Values 5).
        let legacy_attr = name == Name::Longhand(Property::Content)
            && values.clone().all(|c| c.kind() != Kind::Function || c.text().is_some_and(|t| t.eq_ignore_ascii_case("attr")))
            && values.clone().filter(|c| c.kind() == Kind::Function).any(|_| true);
        let pending = !legacy_attr && variables::inspect(values.clone(), limits.variables(), cancelled).map_err(variable_error)?;
        let values = if pending {
            let tokens = variables::Value::compile(values, limits.variables(), cancelled).map_err(variable_error)?;
            let value = Value::Pending(Arc::new(Pending { name, tokens }));
            name.properties().iter().map(|&property| Assignment { property, value: value.clone() }).collect()
        } else {
            parse::known(name, values, limits, cancelled)?
        };
        Ok(Self::Known { important, values })
    }
    pub fn important(&self) -> bool {
        match self {
            Self::Known { important, .. } | Self::Custom { important, .. } => *important,
        }
    }
}

/// Properties that change nothing in a static rendering (interaction,
/// transitions, scrolling, platform hints), accepted so a page's stylesheet
/// issues count what it actually misses. Sorted, ASCII lower case.
const INERT: &[&str] = &[
    "-moz-osx-font-smoothing",
    "-moz-tab-size",
    "-moz-text-size-adjust",
    "-moz-user-select",
    "-ms-overflow-style",
    "-ms-text-size-adjust",
    "-ms-user-select",
    "-o-tab-size",
    "-webkit-font-smoothing",
    "-webkit-overflow-scrolling",
    "-webkit-tap-highlight-color",
    "-webkit-text-size-adjust",
    "-webkit-touch-callout",
    "-webkit-user-drag",
    "-webkit-user-select",
    "anchor-name",
    "anchor-scope",
    "backface-visibility",
    "contain-intrinsic-size",
    "cursor",
    "forced-color-adjust",
    "overscroll-behavior",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "print-color-adjust",
    "scroll-behavior",
    "scroll-margin",
    "scroll-margin-block",
    "scroll-margin-bottom",
    "scroll-margin-inline",
    "scroll-margin-left",
    "scroll-margin-right",
    "scroll-margin-top",
    "scroll-padding",
    "scroll-padding-block",
    "scroll-padding-bottom",
    "scroll-padding-inline",
    "scroll-padding-left",
    "scroll-padding-right",
    "scroll-padding-top",
    "scroll-snap-align",
    "scroll-snap-stop",
    "scroll-snap-type",
    "scrollbar-color",
    "scrollbar-gutter",
    "scrollbar-width",
    "speak",
    "tab-size",
    "text-size-adjust",
    "touch-action",
    "transition",
    "transition-behavior",
    "transition-delay",
    "transition-duration",
    "transition-property",
    "transition-timing-function",
    "user-select",
    "will-change",
];
fn inert(name: &str) -> bool {
    if name.len() > 32 || !name.is_ascii() {
        return false;
    }
    let mut buffer = [0u8; 32];
    for (target, source) in buffer.iter_mut().zip(name.bytes()) {
        *target = source.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&buffer[..name.len()]).unwrap_or("");
    INERT.binary_search(&lower).is_ok()
}
#[cfg(test)]
mod inert_tests {
    #[test]
    fn inert_names_are_sorted() {
        assert!(super::INERT.windows(2).all(|w| w[0] < w[1]));
    }
}
