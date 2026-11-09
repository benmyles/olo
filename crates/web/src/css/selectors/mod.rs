//! Original selector compilation and tree matching. No layout, styling,
//! Web IDL, live event state or renderer integration is implied by these APIs.
mod anb;
pub use anb::AnB;
mod compile;
pub(crate) use compile::RuleCompiler;
mod matching;
use super::Components;
pub use matching::{Attribute, Content, DomTree, ElementName, KeyCache, MatchLimits, MatchOptions, MatchProgress, Matcher, StateSource, Tree, dir_attribute, scope_key};
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
impl State {
    /// The attributes whose change can flip the state (a link's href, a
    /// control's checked or disabled); none for interaction states.
    /// One the person's input sets (hover, press, focus, the URL's target).
    pub fn is_interaction(self) -> bool {
        matches!(self, State::Hover | State::Active | State::Focus | State::FocusWithin | State::FocusVisible | State::Target)
    }
    /// Its invalidation key (see `Invalidation`), for an interaction state.
    pub fn key(self) -> &'static str {
        match self {
            State::Hover => ":hover",
            State::Active => ":active",
            State::Focus => ":focus",
            State::FocusWithin => ":focus-within",
            State::FocusVisible => ":focus-visible",
            State::Target => ":target",
            _ => ":state",
        }
    }
    pub fn reflected_attributes(self) -> &'static [&'static str] {
        match self {
            State::AnyLink | State::Link | State::Visited | State::LocalLink => &["href"],
            State::Enabled | State::Disabled => &["disabled"],
            State::Checked | State::Unchecked | State::Indeterminate | State::Default => &["checked", "selected", "type"],
            State::Required | State::Optional => &["required"],
            State::Valid | State::Invalid | State::UserValid | State::UserInvalid | State::InRange | State::OutOfRange => &["value", "min", "max", "pattern", "required", "type"],
            State::ReadOnly | State::ReadWrite => &["readonly", "disabled", "contenteditable"],
            State::PlaceholderShown => &["placeholder", "value"],
            State::Modal => &["open"],
            State::Target | State::Hover | State::Active | State::Focus | State::FocusWithin | State::FocusVisible | State::Defined => &[],
            _ => &[],
        }
    }
}
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
    /// `slot::slotted(x)`: from the slotted element to its slot.
    Slotted,
    /// `host::part(name)`: from an element in a shadow tree to its host.
    Part,
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
    Type {
        ns: u32,
        name: u32,
    },
    Id(u32),
    Class(u32),
    Attr {
        ns: u32,
        name: u32,
        value: u32,
        failure: u32,
        op: AttrOp,
        case: Case,
    },
    Structural(Structural),
    State(State),
    Pseudo(PseudoElement),
    Logical {
        mode: Logical,
        list: u32,
    },
    Nth(u32),
    Lang {
        start: u32,
        end: u32,
    },
    Dir(bool),
    /// `:host`, or `:host(<list>)` (the list's index; NONE for none): the
    /// shadow host, in its shadow tree's rules.
    Host(u32),
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
    /// Whether any selector depends on an element's descendants (`:has()`,
    /// `:empty`): then a change below an element can change its style.
    pub fn depends_on_descendants(&self) -> bool {
        self.tests.iter().any(|t| matches!(t, Test::Logical { mode: Logical::Has, .. } | Test::Structural(Structural::Empty)))
    }
    pub fn uses_empty(&self) -> bool {
        self.tests.iter().any(|t| matches!(t, Test::Structural(Structural::Empty)))
    }
    /// What the `:has()` arguments test their subjects by, all together: a
    /// changed element with none of these keys cannot change any `:has()`
    /// result, so its ancestors keep their styles. Keys inside `:not()`
    /// and `:is()` count (a class coming or going flips `:not(.x)` too);
    /// states that reflect attributes (`:checked`, `:disabled`, `:link`)
    /// count by those attributes; `:empty` makes text matter.
    pub fn has_keys(&self) -> HasKeys {
        let mut keys = HasKeys::default();
        for test in &self.tests {
            let Test::Logical { mode: Logical::Has, list } = test else { continue };
            keys.any = true;
            self.has_keys_of_list(*list, &mut keys);
        }
        if keys.empty || keys.interaction {
            let mut subject = Vec::new();
            for (i, step) in self.steps.iter().enumerate() {
                let (mut empty, mut interaction, mut siblings) = (false, false, false);
                for t in &self.tests[step.tests.start as usize..step.tests.end as usize] {
                    let Test::Logical { mode: Logical::Has, list } = t else { continue };
                    let mut inner = HasKeys::default();
                    self.has_keys_of_list(*list, &mut inner);
                    empty |= inner.empty;
                    interaction |= inner.interaction;
                    siblings |= inner.interaction && inner.siblings;
                }
                if !empty && !interaction {
                    continue;
                }
                subject.clear();
                self.compound_keys(i as u32, 0, &mut subject);
                for (on, reach) in [(empty, &mut keys.empty_subjects), (interaction, &mut keys.interaction_subjects)] {
                    if on {
                        reach.universal |= subject.is_empty();
                        reach.add(&subject);
                    }
                }
                keys.interaction_siblings |= siblings;
            }
        }
        keys
    }
    fn has_keys_of_list(&self, list: u32, keys: &mut HasKeys) {
        let links = &self.links[self.lists[list as usize].links.start as usize..self.lists[list as usize].links.end as usize];
        for &i in links {
            let complex = &self.complexes[i as usize];
            // Every compound of the argument: any element on its path.
            for step in &self.steps[complex.steps.start as usize..complex.steps.end as usize] {
                keys.siblings |= matches!(step.relation, Relation::Next | Relation::Later);
                let tests = &self.tests[step.tests.start as usize..step.tests.end as usize];
                for t in tests {
                    match t {
                        Test::Type { name, .. } if *name != ANY => {
                            keys.tags.insert(self.symbols[*name as usize].to_ascii_lowercase());
                        }
                        Test::Id(n) => {
                            keys.ids.insert(self.symbols[*n as usize].to_string());
                        }
                        Test::Class(n) => {
                            keys.classes.insert(self.symbols[*n as usize].to_string());
                        }
                        Test::Attr { name, .. } => {
                            keys.attributes.insert(self.symbols[*name as usize].to_ascii_lowercase());
                        }
                        Test::Logical { list, .. } => self.has_keys_of_list(*list, keys),
                        Test::Structural(Structural::Empty) => keys.empty = true,
                        Test::State(state) => {
                            for a in state.reflected_attributes() {
                                keys.attributes.insert((*a).to_owned());
                            }
                            keys.interaction |= state.is_interaction();
                        }
                        Test::Lang { .. } => {
                            keys.attributes.insert("lang".to_owned());
                        }
                        Test::Dir(_) => {
                            keys.attributes.insert("dir".to_owned());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    /// The invalidation sets of every selector (see `Invalidation`).
    pub fn invalidation(&self) -> Invalidation {
        let mut out = Invalidation::default();
        for test in &self.tests {
            if let Test::Attr { name, .. } = test {
                out.attributes.insert(self.symbols[*name as usize].to_string());
            }
        }
        let mut subject = Vec::new();
        for (i, complex) in self.complexes.iter().enumerate() {
            if complex.steps.end <= complex.steps.start + 1 {
                continue;
            }
            // A :has() argument (anchored at its subject) is not a selector
            // of its own: changes in it are followed through `has_keys`.
            let first = &self.steps[complex.steps.start as usize];
            if self.tests[first.tests.start as usize..first.tests.end as usize].iter().any(|t| matches!(t, Test::Structural(Structural::Anchor))) {
                continue;
            }
            subject.clear();
            self.compound_keys(complex.steps.end - 1, 0, &mut subject);
            let mut reach = Reach::default();
            reach.add(&subject);
            // Walking from the subject leftwards: once a sibling combinator
            // is crossed, changes to the left reach the subject as a later
            // sibling's descendant.
            let mut sibling = false;
            for step_index in (complex.steps.start..complex.steps.end - 1).rev() {
                let relation = self.steps[step_index as usize + 1].relation;
                sibling |= matches!(relation, Relation::Next | Relation::Later);
                let mut keys: Vec<String> = Vec::new();
                let mut keyless = false;
                let mut unknown = false;
                self.compound_invalidation_keys(step_index, 0, &mut keys, &mut keyless, &mut unknown);
                let (map, any) = if sibling { (&mut out.siblings, &mut out.any_siblings) } else { (&mut out.descendants, &mut out.any_descendants) };
                if keyless {
                    if crate::env_on!("OLO_STYLE_TIMING") {
                        let step = &self.steps[step_index as usize];
                        let kinds: Vec<String> = self.tests[step.tests.start as usize..step.tests.end as usize]
                            .iter()
                            .map(|t| match t {
                                Test::Structural(s) => format!("{s:?}"),
                                Test::State(s) => format!(":{s:?}"),
                                Test::Pseudo(p) => format!("::{p:?}"),
                                Test::Logical { mode, .. } => format!("{mode:?}()"),
                                Test::Nth(_) => "nth".into(),
                                Test::Lang { .. } => "lang".into(),
                                Test::Dir(_) => "dir".into(),
                                Test::Host(_) => ":host".into(),
                                Test::Type { name, .. } if *name == ANY => "*".into(),
                                Test::Type { .. } => "type".into(),
                                Test::Id(_) => "id".into(),
                                Test::Class(_) => "class".into(),
                                Test::Attr { .. } => "attr".into(),
                            })
                            .collect();
                        // (Of the selector: its complex, by number; a step
                        // is not a list `subject_summary` could read.)
                        eprintln!("  keyless compound [{}] in complex {}", kinds.join("+"), i);
                    }
                }
                // A compound that tests no attribute (`*`, `*:hover`)
                // matches whatever an attribute change does: such a change
                // reaches nothing through it (GitHub's `data-focused`
                // flips restyled the page whole, 6-15 ms each, through a
                // `* > …`). One nested too deep to read may test any.
                if unknown {
                    any.extend(&reach);
                }
                for k in keys {
                    map.entry(k).or_default().extend(&reach);
                }
            }
        }
        out
    }
    /// Every key an element must carry for this compound to match it
    /// (any one changing can change the match), with the attributes its
    /// states reflect; `keyless` when the compound has none (or a logical
    /// branch has none), `unknown` when a branch is nested too deep to read.
    fn compound_invalidation_keys(&self, step: u32, depth: usize, out: &mut Vec<String>, keyless: &mut bool, unknown: &mut bool) {
        let step = &self.steps[step as usize];
        let tests = &self.tests[step.tests.start as usize..step.tests.end as usize];
        let mut keyed = false;
        for test in tests {
            match test {
                Test::Type { name, .. } if *name != ANY => {
                    out.push(format!("t:{}", self.symbols[*name as usize].to_ascii_lowercase()));
                    keyed = true;
                }
                Test::Id(n) => {
                    out.push(format!("#{}", self.symbols[*n as usize]));
                    keyed = true;
                }
                Test::Class(n) => {
                    out.push(format!(".{}", self.symbols[*n as usize]));
                    keyed = true;
                }
                Test::Attr { name, .. } => {
                    out.push(format!("[{}", self.symbols[*name as usize].to_ascii_lowercase()));
                    keyed = true;
                }
                // :has() follows its own keys (the subtree); :is/:where/:not
                // branches contribute theirs.
                Test::Logical { mode: Logical::Has, .. } => {
                    // What a `:has()` flipping on an element reaches (see
                    // `page::interaction_rematch`).
                    out.push(":has".to_owned());
                    keyed = true;
                }
                Test::Logical { list, .. } if depth < 4 => {
                    let links = &self.links[self.lists[*list as usize].links.start as usize..self.lists[*list as usize].links.end as usize];
                    for &i in links {
                        let complex = &self.complexes[i as usize];
                        for inner in complex.steps.start..complex.steps.end {
                            self.compound_invalidation_keys(inner, depth + 1, out, keyless, unknown);
                        }
                    }
                    keyed = true;
                }
                Test::Lang { .. } => {
                    out.push("[lang".to_owned());
                    keyed = true;
                }
                // `:host([open]) .x`: the host's keys reach into its tree.
                Test::Host(list) => {
                    if *list != NONE && depth < 4 {
                        let links = &self.links[self.lists[*list as usize].links.start as usize..self.lists[*list as usize].links.end as usize];
                        for &i in links {
                            let complex = &self.complexes[i as usize];
                            for inner in complex.steps.start..complex.steps.end {
                                self.compound_invalidation_keys(inner, depth + 1, out, keyless, unknown);
                            }
                        }
                    }
                    keyed = true;
                }
                // States an attribute sets (`:checked`, `:disabled`) and
                // directionality follow those attributes too.
                Test::State(state) => {
                    out.extend(state.reflected_attributes().iter().map(|a| format!("[{a}")));
                    // An interaction state's own key: a hover or focus
                    // reaches through it (`.card:hover .title`).
                    if state.is_interaction() {
                        out.push(state.key().to_owned());
                    }
                }
                Test::Dir(_) => out.push("[dir".to_owned()),
                Test::Logical { .. } => *unknown = true,
                Test::Structural(_) | Test::Nth(_) | Test::Pseudo(_) => {}
                Test::Type { .. } => {}
            }
        }
        // States follow interaction and structure the tree (sibling and
        // child-list changes), handled apart; a compound with nothing else
        // (`:hover`, `:first-child`) is not keyless. One with nothing at
        // all (`*`) is.
        let no_keys_needed = !keyed && !tests.is_empty() && tests.iter().all(|t| matches!(t, Test::State(_) | Test::Structural(_) | Test::Nth(_) | Test::Dir(_) | Test::Pseudo(_)));
        if !keyed && !no_keys_needed {
            *keyless = true;
        }
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
/// The keys `:has()` arguments test by (see `Selectors::has_keys`). An
/// element coming or going can flip any argument (the tags say which
/// elements, but a subtree removed is not there to ask), so a child list
/// change with an element in it restyles the ancestors regardless.
#[derive(Default, Debug, Clone)]
pub struct HasKeys {
    /// Any `:has()` at all.
    pub any: bool,
    pub tags: std::collections::HashSet<String>,
    pub ids: std::collections::HashSet<String>,
    pub classes: std::collections::HashSet<String>,
    pub attributes: std::collections::HashSet<String>,
    /// An argument tests `:empty`: text coming or going matters.
    pub empty: bool,
    /// The elements a compound with a `:has()` whose argument tests
    /// `:empty` can match (by its keys): only those can flip when text
    /// comes or goes inside them.
    pub empty_subjects: Reach,
    /// An argument tests an interaction state (`:hover`, `:focus`, …).
    pub interaction: bool,
    /// An argument has a sibling combinator (`:has(+ …)`, `:has(~ …)`).
    pub siblings: bool,
    /// The elements a compound with a `:has()` whose argument tests an
    /// interaction state can match: of an element whose state changed,
    /// only those ancestors can flip.
    pub interaction_subjects: Reach,
    /// Such an argument has a sibling combinator: elements beside the
    /// changed one can flip too (not followed: everything is matched again).
    pub interaction_siblings: bool,
}
/// The subjects a key's change can reach: elements with one of these tags,
/// ids, classes or attribute names (or every element, when universal).
#[derive(Default, Clone, Debug)]
pub struct Reach {
    pub tags: std::collections::HashSet<String>,
    pub ids: std::collections::HashSet<String>,
    pub classes: std::collections::HashSet<String>,
    pub attributes: std::collections::HashSet<String>,
    pub universal: bool,
}
impl Reach {
    pub fn is_empty(&self) -> bool {
        !self.universal && self.tags.is_empty() && self.ids.is_empty() && self.classes.is_empty() && self.attributes.is_empty()
    }
    fn add(&mut self, keys: &[SubjectKey]) {
        for k in keys {
            match k {
                SubjectKey::Universal | SubjectKey::Root => self.universal = true,
                SubjectKey::Id(v) => {
                    self.ids.insert(v.to_string());
                }
                SubjectKey::Class(v) => {
                    self.classes.insert(v.to_string());
                }
                SubjectKey::Type(v) => {
                    self.tags.insert(v.to_ascii_lowercase());
                }
                SubjectKey::Attr(v) => {
                    self.attributes.insert(v.to_ascii_lowercase());
                }
            }
        }
    }
    pub fn extend(&mut self, other: &Reach) {
        self.universal |= other.universal;
        self.tags.extend(other.tags.iter().cloned());
        self.ids.extend(other.ids.iter().cloned());
        self.classes.extend(other.classes.iter().cloned());
        self.attributes.extend(other.attributes.iter().cloned());
    }
    /// Whether the element (its tag, id, classes, attribute names) is reached.
    pub fn reaches(&self, doc: &crate::dom::Document, node: crate::dom::NodeId) -> bool {
        if self.universal {
            return true;
        }
        if !self.tags.is_empty()
            && let Ok(Some(name)) = doc.name(node)
            && self.tags.contains(&name.to_ascii_lowercase())
        {
            return true;
        }
        if self.ids.is_empty() && self.classes.is_empty() && self.attributes.is_empty() {
            return false;
        }
        let Ok(attributes) = doc.attributes_ns(node) else { return true };
        for a in attributes {
            if !self.attributes.is_empty() && self.attributes.contains(&a.local_name.to_ascii_lowercase()) {
                return true;
            }
            if a.local_name == "id" && self.ids.contains(a.value) {
                return true;
            }
            if a.local_name == "class" && !self.classes.is_empty() && a.value.split_ascii_whitespace().any(|c| self.classes.contains(c)) {
                return true;
            }
        }
        false
    }
}
/// What a change to an element's tag, id, class or attribute can affect
/// elsewhere: for each key found in a compound other than the subject's,
/// the subjects it reaches among the element's descendants (descendant
/// and child combinators) or its later siblings and their descendants
/// (sibling combinators). A keyless compound (`:hover > .x`, `:not(.a) .b`)
/// reaches its subjects from any change.
#[derive(Default, Clone, Debug)]
pub struct Invalidation {
    pub descendants: std::collections::HashMap<String, Reach>,
    pub siblings: std::collections::HashMap<String, Reach>,
    pub any_descendants: Reach,
    pub any_siblings: Reach,
    /// Every attribute any selector tests (in any compound, inside `:not()`
    /// and `:is()` too): one no selector tests changes no match, keyless
    /// compounds or not.
    pub attributes: std::collections::HashSet<String>,
}
impl Invalidation {
    /// Whether any selector tests this attribute.
    pub fn tests_attribute(&self, name: &str) -> bool {
        self.attributes.contains(name)
    }
    /// Keys are written "t:tag", "#id", ".class", "[attr".
    pub fn reach_of(&self, keys: impl Iterator<Item = String>) -> (Reach, Reach) {
        let (mut d, mut s) = (self.any_descendants.clone(), self.any_siblings.clone());
        for k in keys {
            if let Some(r) = self.descendants.get(&k) {
                d.extend(r);
            }
            if let Some(r) = self.siblings.get(&k) {
                s.extend(r);
            }
        }
        (d, s)
    }
}
impl HasKeys {
    /// Whether a key that changed ("[attr", ".class", "#id") is one an
    /// argument tests: only then can an ancestor's :has() flip.
    pub fn has_key(&self, key: &str) -> bool {
        key.strip_prefix('.').is_some_and(|c| self.classes.contains(c))
            || key.strip_prefix('#').is_some_and(|i| self.ids.contains(i))
            || key.strip_prefix('[').is_some_and(|a| self.attributes.contains(a))
    }
    /// Whether an attribute's change can flip an argument: the attribute
    /// is tested, and (for one a state reflects by presence alone, like
    /// a placeholder) it came or went.
    pub fn attribute_flips(&self, name: &str, old: Option<&str>, new: Option<&str>) -> bool {
        if !self.attributes.contains(name) {
            return false;
        }
        match name {
            "placeholder" => old.is_none_or(str::is_empty) != new.is_none_or(str::is_empty),
            _ => true,
        }
    }
    pub fn is_empty(&self) -> bool {
        !self.any
    }
    /// Whether this element (by its tag, id, classes and attribute names)
    /// could take part in a `:has()` argument's match.
    /// Whether an element whose attributes changed (which is unknown)
    /// could take part in an argument: it carries a key, or an argument
    /// tests an attribute it may have lost.
    pub fn touches(&self, doc: &crate::dom::Document, node: crate::dom::NodeId) -> bool {
        if !self.attributes.is_empty() {
            return true;
        }
        if let Ok(Some(name)) = doc.name(node)
            && self.tags.contains(&name.to_ascii_lowercase())
        {
            return true;
        }
        let Ok(attributes) = doc.attributes_ns(node) else { return true };
        for a in attributes {
            if self.attributes.contains(&a.local_name.to_ascii_lowercase()) {
                return true;
            }
            if a.local_name == "id" && self.ids.contains(a.value) {
                return true;
            }
            if a.local_name == "class" && a.value.split(white).any(|c| self.classes.contains(c)) {
                return true;
            }
        }
        false
    }
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
    /// A selector list's complexes written out roughly (names, combinators),
    /// for the timing log's costly matches.
    pub(crate) fn describe(&self, root: u32) -> String {
        self.links[self.lists[root as usize].links.start as usize..self.lists[root as usize].links.end as usize]
            .iter()
            .take(4)
            .map(|&i| {
                let complex = &self.complexes[i as usize];
                let mut out = String::new();
                for at in complex.steps.clone() {
                    let step = &self.steps[at as usize];
                    for t in &self.tests[step.tests.start as usize..step.tests.end as usize] {
                        out.push_str(&match t {
                            Test::Type { name, .. } => {
                                if *name == ANY {
                                    "*".to_owned()
                                } else {
                                    self.symbol(*name).to_owned()
                                }
                            }
                            Test::Id(id) => format!("#{}", self.symbol(*id)),
                            Test::Class(c) => format!(".{}", self.symbol(*c)),
                            Test::Attr { name, .. } => format!("[{}]", self.symbol(*name)),
                            Test::Structural(s) => format!(":{s:?}"),
                            Test::State(s) => format!(":{s:?}"),
                            Test::Pseudo(p) => format!("::{p:?}"),
                            Test::Logical { mode, list } => format!(":{mode:?}({})", self.describe(*list)),
                            Test::Nth(_) => ":nth".into(),
                            Test::Lang { .. } => ":lang".into(),
                            Test::Dir(_) => ":dir".into(),
                            Test::Host(_) => ":host".into(),
                        });
                    }
                    if at + 1 < complex.steps.end {
                        out.push_str(match self.steps[at as usize].relation {
                            Relation::Child => " > ",
                            Relation::Descendant => " ",
                            Relation::Next => " + ",
                            Relation::Later => " ~ ",
                            _ => " ? ",
                        });
                    }
                }
                out
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
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
                        Test::Host(_) => ":host".into(),
                    })
                    .collect::<Vec<_>>()
                    .join("+")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
    /// Whether a branch of list `root` addressing `pseudo` could match an
    /// element whose ancestors are `filter`'s: the matcher's fast reject,
    /// from the compiled selectors alone, without making a matcher.
    pub(crate) fn may_match(&self, root: u32, pseudo: Option<PseudoElement>, filter: &AncestorFilter) -> bool {
        let Some(list) = self.lists.get(root as usize) else { return true };
        self.links[list.links.start as usize..list.links.end as usize].iter().any(|&i| {
            let complex = &self.complexes[i as usize];
            complex.pseudo == pseudo && self.ancestor_hashes[complex.ancestors.start as usize..complex.ancestors.end as usize].iter().all(|&h| filter.may_contain(h))
        })
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
