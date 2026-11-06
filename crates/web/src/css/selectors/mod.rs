//! Original selector compilation and tree matching. No layout, styling,
//! Web IDL, live event state or renderer integration is implied by these APIs.
mod anb;
pub use anb::AnB;
mod compile;
pub(crate) use compile::RuleCompiler;
mod matching;
use super::Components;
pub use matching::{Attribute, Content, DomTree, ElementName, MatchLimits, MatchOptions, MatchProgress, StateSource, Tree};
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const ANY: u32 = u32::MAX;
const NONE: u32 = u32::MAX - 1;
const HTML: &str = "http://www.w3.org/1999/xhtml";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    pub ids: u32,
    pub classes: u32,
    pub types: u32,
}
impl Specificity {
    fn add(self, rhs: Self) -> Self {
        Self { ids: self.ids.saturating_add(rhs.ids), classes: self.classes.saturating_add(rhs.classes), types: self.types.saturating_add(rhs.types) }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct CompileLimits {
    pub selectors: usize,
    pub steps: usize,
    pub tests: usize,
    pub symbol_bytes: usize,
    pub pattern_entries: usize,
    pub depth: usize,
    pub work: usize,
}
impl Default for CompileLimits {
    fn default() -> Self {
        Self { selectors: 100_000, steps: 1_000_000, tests: 2_000_000, symbol_bytes: 8 * 1024 * 1024, pattern_entries: 8 * 1024 * 1024, depth: 64, work: 16_000_000 }
    }
}
#[derive(Clone, Copy, Default)]
pub struct CompileOptions<'a> {
    /// Resolved prefix bindings; if repeated, the last binding wins.
    pub namespaces: &'a [(&'a str, &'a str)],
    pub default_namespace: Option<&'a str>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    NumberLimit,
    CompileLimit,
    MatchLimit,
    DepthLimit,
    Cancelled,
    InvalidNode,
    StateUnavailable,
    Dom(crate::dom::Error),
}
impl From<crate::dom::Error> for Error {
    fn from(e: crate::dom::Error) -> Self {
        Self::Dom(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Selector {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
/// States are supplied by the host's live element/control model. A missing
/// state source is an error, not a silent false match. History privacy and
/// visited-style restrictions must be enforced by the eventual host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    AnyLink,
    Link,
    Visited,
    LocalLink,
    Target,
    Hover,
    Active,
    Focus,
    FocusWithin,
    FocusVisible,
    Enabled,
    Disabled,
    Checked,
    Unchecked,
    Indeterminate,
    Default,
    Defined,
    Required,
    Optional,
    Valid,
    Invalid,
    UserValid,
    UserInvalid,
    InRange,
    OutOfRange,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Autofill,
    Open,
    PopoverOpen,
    Modal,
    Fullscreen,
    PictureInPicture,
    Playing,
    Paused,
    Seeking,
    Buffering,
    Stalled,
    Muted,
    VolumeLocked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoElement {
    Before,
    After,
    FirstLine,
    FirstLetter,
    Marker,
    Selection,
    Placeholder,
    Backdrop,
    FileSelectorButton,
    SpellingError,
    GrammarError,
    TargetText,
}
#[derive(Clone, Copy)]
enum Relation {
    None,
    Descendant,
    Child,
    Next,
    Later,
}
#[derive(Clone, Copy)]
enum AttrOp {
    Exists,
    Equal,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Contains,
}
#[derive(Clone, Copy)]
enum Case {
    Default,
    Sensitive,
    Ascii,
}
#[derive(Clone, Copy)]
enum Logical {
    Is,
    Not,
    Has,
}
#[derive(Clone, Copy)]
enum Structural {
    Root,
    Scope,
    Empty,
    First,
    Last,
    Only,
    FirstType,
    LastType,
    OnlyType,
    Anchor,
}
#[derive(Clone, Copy)]
enum Test {
    Type { ns: u32, name: u32 },
    Id(u32),
    Class(u32),
    Attr { ns: u32, name: u32, value: u32, failure: u32, op: AttrOp, case: Case },
    Structural(Structural),
    State(State),
    Pseudo(PseudoElement),
    Logical { mode: Logical, list: u32 },
    Nth(u32),
    Lang { start: u32, end: u32 },
    Dir(bool),
}
struct Nth {
    formula: AnB,
    list: u32,
    of_type: bool,
    last: bool,
}
#[derive(Clone)]
struct Step {
    tests: Range<u32>,
    relation: Relation,
    namespace: u32,
    implicit_ns: bool,
    featureless: bool,
}
struct Complex {
    steps: Range<u32>,
    specificity: Specificity,
    relative: Relation,
    pseudo: Option<PseudoElement>,
    featureless: bool,
}
struct List {
    links: Range<u32>,
    specificity: Specificity,
    featureless: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub selectors: usize,
    pub steps: usize,
    pub tests: usize,
    pub symbols: usize,
    pub symbol_bytes: usize,
    /// Retained vector capacities, symbol payload/refcount headers; excludes
    /// allocator bookkeeping, this struct and temporary compiler maps.
    pub accounted_reserved_bytes: usize,
    pub test_size: usize,
}
/// Flat compiled arenas; child lists and logical functions never recursively
/// own allocations. Symbols are shared once per compiled selector collection.
/// Debug deliberately omits selector text, IDs, class names and attribute values.
pub struct Selectors {
    lists: Vec<List>,
    links: Vec<u32>,
    complexes: Vec<Complex>,
    steps: Vec<Step>,
    tests: Vec<Test>,
    nth: Vec<Nth>,
    symbols: Vec<Arc<str>>,
    symbol_bytes: usize,
    failures: Vec<u32>,
    languages: Vec<u32>,
    root: u32,
}
impl std::fmt::Debug for Selectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selectors").field("memory", &self.memory()).finish_non_exhaustive()
    }
}
impl Selectors {
    pub fn compile(values: Components<'_>, options: CompileOptions<'_>, limits: CompileLimits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        compile::compile(values, options, limits, cancelled)
    }
    /// Conservative state dependencies, including nested logical selectors.
    pub fn state_mask(&self) -> u64 {
        self.tests.iter().fold(0, |mask, test| match test {
            Test::State(state) => mask | (1u64 << *state as u8),
            _ => mask,
        })
    }
    pub fn specificities(&self) -> impl ExactSizeIterator<Item = Specificity> + '_ {
        self.links[self.lists[self.root as usize].links.start as usize..self.lists[self.root as usize].links.end as usize].iter().map(|&i| self.complexes[i as usize].specificity)
    }
    fn symbol(&self, id: u32) -> &str {
        &self.symbols[id as usize]
    }
    pub fn memory(&self) -> Memory {
        Memory {
            selectors: self.complexes.len(),
            steps: self.steps.len(),
            tests: self.tests.len(),
            symbols: self.symbols.len(),
            symbol_bytes: self.symbol_bytes,
            test_size: std::mem::size_of::<Test>(),
            accounted_reserved_bytes: self.lists.capacity() * std::mem::size_of::<List>()
                + self.links.capacity() * 4
                + self.complexes.capacity() * std::mem::size_of::<Complex>()
                + self.steps.capacity() * std::mem::size_of::<Step>()
                + self.tests.capacity() * std::mem::size_of::<Test>()
                + self.nth.capacity() * std::mem::size_of::<Nth>()
                + self.symbols.capacity() * std::mem::size_of::<Arc<str>>()
                + self.symbol_bytes
                + self.symbols.len() * 2 * std::mem::size_of::<usize>()
                + self.failures.capacity() * 4
                + self.languages.capacity() * 4,
        }
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn white(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c')
}

/// Conservative necessary subject keys, one per root branch. A logical-only
/// subject stays universal. Indexing may admit false positives; matching still
/// decides every branch and its actual specificity.
pub(crate) enum SubjectKey {
    Universal,
    Id(Arc<str>),
    Class(Arc<str>),
    Type(Arc<str>),
}
impl Selectors {
    pub(crate) fn subject_keys(&self, root: u32) -> Vec<SubjectKey> {
        self.links[self.lists[root as usize].links.start as usize..self.lists[root as usize].links.end as usize]
            .iter()
            .map(|&i| {
                let complex = &self.complexes[i as usize];
                let step = &self.steps[(complex.steps.end - 1) as usize];
                let tests = &self.tests[step.tests.start as usize..step.tests.end as usize];
                for choose in 0..3 {
                    for test in tests {
                        match (choose, test) {
                            (0, Test::Id(n)) => return SubjectKey::Id(self.symbols[*n as usize].clone()),
                            (1, Test::Class(n)) => return SubjectKey::Class(self.symbols[*n as usize].clone()),
                            (2, Test::Type { name, .. }) if *name != ANY => return SubjectKey::Type(self.symbols[*name as usize].clone()),
                            _ => {}
                        }
                    }
                }
                SubjectKey::Universal
            })
            .collect()
    }
}
