//! Original selector compilation and tree matching. No layout, styling,
//! Web IDL, live event state or renderer integration is implied by these APIs.
mod anb;
pub use anb::AnB;
mod compile;
pub(crate) use compile::RuleCompiler;
mod matching;
use super::Components;
pub use matching::{Attribute, Content, DomTree, ElementName, KeyCache, MatchLimits, MatchOptions, MatchProgress, Matcher, StateSource, Tree};
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
    UnsupportedState(State),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Copy, Debug)]
enum Logical {
    Is,
    Not,
    Has,
}
#[derive(Clone, Copy, Debug)]
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
    /// Hashes (in `ancestor_hashes`) of the tags, ids and classes its
    /// ancestor compounds require: an element whose ancestors lack one
    /// cannot match, and is rejected without a walk.
    ancestors: Range<u32>,
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
    /// Each symbol's `token_hash`, compared before its text when matching
    /// ids and classes.
    symbol_hashes: Vec<u64>,
    ancestor_hashes: Vec<u64>,
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
    /// Whether any selector addresses a pseudo-element (`::before`, …).
    pub fn has_pseudo_elements(&self) -> bool {
        self.complexes.iter().any(|c| c.pseudo.is_some())
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
                + self.symbol_hashes.capacity() * 8
                + self.ancestor_hashes.capacity() * 8
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
/// What an element's ancestors are: their tags, ids and classes, hashed
/// into a counting Bloom filter the style walk keeps as it descends. A
/// selector needing an ancestor key the filter lacks is rejected at once;
/// a key it has may still fail to match (the walk decides).
pub struct AncestorFilter {
    counts: Box<[u8; 1 << 12]>,
}
impl Default for AncestorFilter {
    fn default() -> Self {
        Self { counts: Box::new([0; 1 << 12]) }
    }
}
impl AncestorFilter {
    fn slots(hash: u64) -> [usize; 2] {
        [(hash & 0xfff) as usize, ((hash >> 20) & 0xfff) as usize]
    }
    pub fn insert(&mut self, hash: u64) {
        for i in Self::slots(hash) {
            self.counts[i] = self.counts[i].saturating_add(1);
        }
    }
    /// A count that saturated stays: the filter then over-admits, never
    /// rejects wrongly.
    pub fn remove(&mut self, hash: u64) {
        for i in Self::slots(hash) {
            if self.counts[i] != u8::MAX {
                self.counts[i] = self.counts[i].saturating_sub(1);
            }
        }
    }
    pub fn may_contain(&self, hash: u64) -> bool {
        Self::slots(hash).into_iter().all(|i| self.counts[i] != 0)
    }
    /// The keys of one element, as the filter and selectors hash them: its
    /// tag (ASCII-folded), id and classes.
    pub fn element_hashes(local_name: &str, id: Option<&str>, classes: &str, out: &mut impl Extend<u64>) {
        out.extend(std::iter::once(folded_hash(local_name)));
        if let Some(id) = id {
            out.extend(std::iter::once(token_hash(id)));
        }
        out.extend(classes.split(white).filter(|s| !s.is_empty()).map(token_hash));
    }
}
/// `token_hash` of a name's ASCII lower case (type selectors match HTML
/// tags case-insensitively).
pub(crate) fn folded_hash(s: &str) -> u64 {
    s.bytes().map(|b| b.to_ascii_lowercase()).fold(0u64, |h, b| (h.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95))
}
/// A fast hash of an id or class token (FxHash's step), the same on both
/// sides of a match.
pub(crate) fn token_hash(s: &str) -> u64 {
    s.bytes().fold(0u64, |h, b| (h.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95))
}

/// Conservative necessary subject keys, one per root branch. A logical-only
/// subject stays universal. Indexing may admit false positives; matching still
/// decides every branch and its actual specificity.
#[derive(Clone)]
pub(crate) enum SubjectKey {
    Universal,
    Id(Arc<str>),
    Class(Arc<str>),
    Type(Arc<str>),
    /// An attribute the subject must have (`[hidden]`, `[aria-invalid=…]`).
    Attr(Arc<str>),
    /// `:root`: the document's root element only.
    Root,
}
impl Selectors {
    /// The kinds of tests in each branch's subject compound, for seeing why
    /// a rule is universal (diagnostics).
    pub(crate) fn subject_summary(&self, root: u32) -> String {
        self.links[self.lists[root as usize].links.start as usize..self.lists[root as usize].links.end as usize]
            .iter()
            .map(|&i| {
                let complex = &self.complexes[i as usize];
                let step = &self.steps[(complex.steps.end - 1) as usize];
                self.tests[step.tests.start as usize..step.tests.end as usize]
                    .iter()
                    .map(|t| match t {
                        Test::Type { .. } => "type".to_owned(),
                        Test::Id(_) => "id".into(),
                        Test::Class(_) => "class".into(),
                        Test::Attr { name, .. } => format!("[{}]", self.symbol(*name)),
                        Test::Structural(s) => format!("{s:?}"),
                        Test::State(s) => format!(":{s:?}"),
                        Test::Pseudo(p) => format!("::{p:?}"),
                        Test::Logical { mode, .. } => format!("{mode:?}()"),
                        Test::Nth(_) => "nth".into(),
                        Test::Lang { .. } => "lang".into(),
                        Test::Dir(_) => "dir".into(),
                    })
                    .collect::<Vec<_>>()
                    .join("+")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
    /// Each branch's key, with the pseudo-element it addresses, if any: a
    /// `::before` branch can only match a ::before, never the element.
    /// Each branch's keys (several when the subject is `:is(a, b)`: one per
    /// branch of the list, all needed to be indexable), with the
    /// pseudo-element it addresses.
    pub(crate) fn subject_keys(&self, root: u32) -> Vec<(SubjectKey, Option<PseudoElement>)> {
        let mut out = Vec::new();
        for &i in &self.links[self.lists[root as usize].links.start as usize..self.lists[root as usize].links.end as usize] {
            let complex = &self.complexes[i as usize];
            let mut keys = Vec::new();
            self.compound_keys(complex.steps.end - 1, 0, &mut keys);
            for key in keys {
                out.push((key, complex.pseudo));
            }
        }
        out
    }
    /// The keys a compound's subject must have, best first: an id, a class,
    /// a type, an attribute, :root; else the branches of an `:is()` whose
    /// every branch has a key; else universal.
    fn compound_keys(&self, step: u32, depth: usize, out: &mut Vec<SubjectKey>) {
        let step = &self.steps[step as usize];
        let tests = &self.tests[step.tests.start as usize..step.tests.end as usize];
        for choose in 0..5 {
            for test in tests {
                match (choose, test) {
                    (0, Test::Id(n)) => {
                        out.push(SubjectKey::Id(self.symbols[*n as usize].clone()));
                        return;
                    }
                    (1, Test::Class(n)) => {
                        out.push(SubjectKey::Class(self.symbols[*n as usize].clone()));
                        return;
                    }
                    (2, Test::Type { name, .. }) if *name != ANY => {
                        out.push(SubjectKey::Type(self.symbols[*name as usize].clone()));
                        return;
                    }
                    (3, Test::Attr { name, .. }) => {
                        out.push(SubjectKey::Attr(self.symbols[*name as usize].clone()));
                        return;
                    }
                    (4, Test::Structural(Structural::Root)) => {
                        out.push(SubjectKey::Root);
                        return;
                    }
                    _ => {}
                }
            }
        }
        if depth < 4 {
            for test in tests {
                if let Test::Logical { mode: Logical::Is, list } = test {
                    let start = out.len();
                    let links = &self.links[self.lists[*list as usize].links.start as usize..self.lists[*list as usize].links.end as usize];
                    let mut all = true;
                    for &i in links {
                        let complex = &self.complexes[i as usize];
                        let before = out.len();
                        self.compound_keys(complex.steps.end - 1, depth + 1, out);
                        if out[before..].iter().any(|k| matches!(k, SubjectKey::Universal)) {
                            all = false;
                            break;
                        }
                    }
                    if all && out.len() > start {
                        return;
                    }
                    out.truncate(start);
                }
            }
        }
        out.push(SubjectKey::Universal);
    }
}
