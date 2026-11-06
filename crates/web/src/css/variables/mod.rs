//! Original token-preserving CSS custom-property substitution. The caller
//! supplies a stable cascaded element snapshot; no DOM/cascade is guessed.
mod compile;
mod environment;
mod resolve;
use super::{Components, Kind, Syntax, syntax::Assembler};
pub(crate) use compile::inspect;
pub use environment::Environment;
pub use resolve::{Binding, Context, Resolver, Rollback, Stats};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
const PENDING: u32 = 1 << 31;
const END: u32 = !PENDING;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub nodes: usize,
    pub work: usize,
    pub depth: usize,
    pub properties: usize,
    pub retained_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { bytes: 1024 * 1024, nodes: 262144, work: 4_000_000, depth: 128, properties: 65536, retained_bytes: 16 * 1024 * 1024 }
    }
}
impl Limits {
    fn syntax(self) -> super::Limits {
        super::Limits { input_bytes: self.bytes, tokens: self.nodes.min(END as usize), decoded_bytes: self.bytes, depth: self.depth.min(256), diagnostics: 0, items: self.nodes }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    RequiresCascade,
    ContextChanged,
    Limit,
    DepthLimit,
    Cancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS variables {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn syntax_error(e: super::Error) -> Error {
    match e {
        super::Error::Cancelled => Error::Cancelled,
        super::Error::DepthLimit => Error::DepthLimit,
        _ => Error::Limit,
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wide {
    Initial,
    Inherit,
    Unset,
    Revert,
    RevertLayer,
    RevertRule,
}
/// Sixteen-byte view into a compact shared token arena. Compilation copies
/// only supplied component values, releasing the original stylesheet Syntax.
#[derive(Clone)]
pub struct Value {
    syntax: Arc<Syntax>,
    start: u32,
    end: u32,
}
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VariableValue").field("pending", &self.needs_substitution()).field("nodes", &(self.end & END).saturating_sub(self.start)).finish_non_exhaustive()
    }
}
impl Value {
    pub fn compile(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        compile::one(values, limits, cancelled)
    }
    /// Batch compilation shares one compact source/token arena across values.
    pub fn compile_batch<'a>(values: impl IntoIterator<Item = Components<'a>>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Vec<Self>> {
        compile::batch(values, limits, cancelled)
    }
    pub fn components(&self) -> Components<'_> {
        self.syntax.range(self.start..(self.end & END))
    }
    pub fn needs_substitution(&self) -> bool {
        self.end & PENDING != 0
    }
    /// Coherent spelling of the assembled token stream, with boundary comments
    /// and implicit EOF closures made explicit. This is not CSSOM serialization.
    pub fn source_spelling(&self) -> &str {
        let end = self.end & END;
        if self.start == end {
            return "";
        }
        let start = self.syntax.component(self.start).span().start as usize;
        let end = self.components().last().unwrap().span().end as usize;
        &self.syntax.source()[start..end]
    }
    pub fn wide(&self) -> Option<Wide> {
        wide(self.components())
    }
    pub(crate) fn arena_identity(&self) -> usize {
        Arc::as_ptr(&self.syntax) as usize
    }
    /// Whole shared allocation's accounted capacities/header. Count once per
    /// arena, not once per Value/clone in a batch; excludes allocator metadata.
    pub fn arena_reserved_bytes(&self) -> usize {
        arena_bytes(&self.syntax)
    }
}
fn arena_bytes(syntax: &Syntax) -> usize {
    let m = syntax.memory();
    size_of::<Syntax>()
        + 2 * size_of::<usize>()
        + size_of::<String>()
        + 2 * size_of::<usize>()
        + m.source_reserved_bytes
        + m.node_reserved_bytes
        + m.decoded_slot_reserved_bytes
        + m.decoded_bytes
        + m.dimension_reserved_bytes
        + m.diagnostic_reserved_bytes
}
/// Empty token content is valid. `None` alone denotes guaranteed-invalid.
#[derive(Clone, Debug)]
pub struct Computed {
    value: Option<Value>,
    animation_tainted: bool,
}
impl Computed {
    pub fn invalid() -> Self {
        Self { value: None, animation_tainted: false }
    }
    pub fn components(&self) -> Option<Components<'_>> {
        self.value.as_ref().map(Value::components)
    }
    pub fn is_invalid(&self) -> bool {
        self.value.is_none()
    }
    pub fn animation_tainted(&self) -> bool {
        self.animation_tainted
    }
    pub fn value(&self) -> Option<&Value> {
        self.value.as_ref()
    }
}
fn single(mut values: Components<'_>) -> Option<super::Component<'_>> {
    let first = values.find(|c| c.kind() != Kind::Whitespace)?;
    if values.any(|c| c.kind() != Kind::Whitespace) { None } else { Some(first) }
}
fn wide(values: Components<'_>) -> Option<Wide> {
    wide_component(single(values)?)
}
fn wide_component(c: super::Component<'_>) -> Option<Wide> {
    if c.kind() != Kind::Ident {
        return None;
    }
    let name = c.text()?;
    [("initial", Wide::Initial), ("inherit", Wide::Inherit), ("unset", Wide::Unset), ("revert", Wide::Revert), ("revert-layer", Wide::RevertLayer), ("revert-rule", Wide::RevertRule)]
        .into_iter()
        .find_map(|(s, w)| name.eq_ignore_ascii_case(s).then_some(w))
}
fn substitution(name: &str) -> bool {
    ["var", "env", "attr", "if", "inherit", "first-valid", "random-item", "ident"].iter().any(|s| name.eq_ignore_ascii_case(s)) || name.starts_with("--") && name.len() > 2
}
fn name_component(c: super::Component<'_>) -> Option<&str> {
    if c.kind() != Kind::Ident {
        return None;
    }
    let s = c.text()?;
    (s.starts_with("--") && s.len() > 2).then_some(s)
}
