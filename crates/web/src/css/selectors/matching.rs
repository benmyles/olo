use super::*;
use crate::dom::{self, Document, NodeId, NodeType};
#[derive(Clone, Copy)]
pub struct ElementName<'a> {
    pub namespace: &'a str,
    pub local_name: &'a str,
}
#[derive(Clone, Copy)]
pub struct Attribute<'a> {
    pub namespace: &'a str,
    pub local_name: &'a str,
    pub value: &'a str,
}
#[derive(Clone, Copy)]
pub enum Content<'a> {
    Element(ElementName<'a>),
    Text(&'a str),
    Document,
    Fragment,
    Other,
}
/// A stable borrowed light tree. Links and names must remain coherent during
/// a match/query; no flattening, implicit shadow crossing or snapshot copying.
/// Methods may reject invalid/stale IDs. All potentially long work is bounded
/// by the matching machine; implementations must not block or mutate the tree.
pub trait Tree {
    type Node: Copy + Eq + std::hash::Hash;
    fn content(&self, node: Self::Node) -> Result<Content<'_>>;
    fn parent(&self, node: Self::Node) -> Result<Option<Self::Node>>;
    fn first_child(&self, node: Self::Node) -> Result<Option<Self::Node>>;
    fn previous_sibling(&self, node: Self::Node) -> Result<Option<Self::Node>>;
    fn next_sibling(&self, node: Self::Node) -> Result<Option<Self::Node>>;
    fn attributes(&self, node: Self::Node) -> Result<impl Iterator<Item = Attribute<'_>>>;
    fn state(&self, _node: Self::Node, _pseudo: Option<PseudoElement>, _state: State) -> Result<bool> {
        Err(Error::StateUnavailable)
    }
    fn direction(&self, _node: Self::Node) -> Result<bool> {
        Err(Error::StateUnavailable)
    }
}
pub trait StateSource {
    fn state(&self, node: NodeId, pseudo: Option<PseudoElement>, state: State) -> Result<bool>;
    /// True is RTL, false LTR; supply the host's resolved semantic direction.
    fn direction(&self, _node: NodeId) -> Result<bool> {
        Err(Error::StateUnavailable)
    }
}
pub struct DomTree<'a> {
    document: &'a Document,
    states: Option<&'a dyn StateSource>,
}
impl<'a> DomTree<'a> {
    pub fn new(document: &'a Document) -> Self {
        Self { document, states: None }
    }
    pub fn with_states(document: &'a Document, states: &'a dyn StateSource) -> Self {
        Self { document, states: Some(states) }
    }
}
impl Tree for DomTree<'_> {
    type Node = NodeId;
    fn content(&self, node: NodeId) -> Result<Content<'_>> {
        Ok(match self.document.node_type(node)? {
            NodeType::Element => Content::Element(ElementName {
                namespace: match self.document.namespace(node)?.unwrap() {
                    dom::Namespace::Html => HTML,
                    dom::Namespace::Svg => "http://www.w3.org/2000/svg",
                    dom::Namespace::MathMl => "http://www.w3.org/1998/Math/MathML",
                },
                local_name: self.document.name(node)?.unwrap(),
            }),
            NodeType::Text => Content::Text(self.document.value(node)?.unwrap()),
            NodeType::Document => Content::Document,
            NodeType::Fragment => Content::Fragment,
            _ => Content::Other,
        })
    }
    fn parent(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(self.document.parent(node)?)
    }
    fn first_child(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(self.document.first_child(node)?)
    }
    fn previous_sibling(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(self.document.previous_sibling(node)?)
    }
    fn next_sibling(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(self.document.next_sibling(node)?)
    }
    fn attributes(&self, node: NodeId) -> Result<impl Iterator<Item = Attribute<'_>>> {
        Ok(self.document.attributes_ns(node)?.map(|a| Attribute { namespace: a.namespace.map_or("", dom::AttributeNamespace::uri), local_name: a.local_name, value: a.value }))
    }
    fn state(&self, node: NodeId, pseudo: Option<PseudoElement>, state: State) -> Result<bool> {
        match self.states {
            Some(states) => states.state(node, pseudo, state),
            // Link-ness is the document's, not interaction state: an `a` or
            // `area` with an href (HTML §4.6.1), without a state source too.
            None if matches!(state, State::AnyLink | State::Link) && pseudo.is_none() => {
                Ok(self.document.namespace(node)? == Some(dom::Namespace::Html) && matches!(self.document.name(node)?, Some("a" | "area")) && self.document.attribute(node, "href")?.is_some())
            }
            None => Err(Error::StateUnavailable),
        }
    }
    fn direction(&self, node: NodeId) -> Result<bool> {
        self.states.ok_or(Error::StateUnavailable)?.direction(node)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct MatchLimits {
    pub work: usize,
    pub backtracking_frames: usize,
    pub call_depth: usize,
    pub results: usize,
    pub index_entries: usize,
}
impl Default for MatchLimits {
    fn default() -> Self {
        Self { work: 16_000_000, backtracking_frames: 4096, call_depth: 128, results: 100_000, index_entries: 100_000 }
    }
}
#[derive(Clone, Copy)]
pub struct MatchOptions<'a, N> {
    pub html_document: bool,
    pub quirks: bool,
    pub scope: Option<N>,
    pub pseudo: Option<PseudoElement>,
    /// With `pseudo`: only rules declaring `content` are candidates (a
    /// cheap test of whether the pseudo-element exists at all).
    pub content_only: bool,
    pub document_language: Option<&'a str>,
    /// Only a future private paint path may enable visited-state matching;
    /// DOM/agent queries must keep this false and never expose history.
    pub visited_style: bool,
}
impl<N> Default for MatchOptions<'_, N> {
    fn default() -> Self {
        Self { html_document: true, quirks: false, scope: None, pseudo: None, content_only: false, document_language: None, visited_style: false }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct MatchProgress {
    pub visited: usize,
    pub matched: usize,
    pub work: usize,
    /// Compounds evaluated, tests evaluated, tree steps taken (parents and
    /// siblings walked): where matching's time goes.
    pub compounds: usize,
    pub tests: usize,
    pub steps: usize,
    /// Branches rejected by the ancestor filter.
    pub rejected: usize,
    pub peak_backtracking_frames: usize,
    pub index_entries: usize,
    pub index_capacity: usize,
    pub peak_pending_indices: usize,
}
#[derive(Clone, Copy)]
enum Axis {
    Single,
    Parent,
    Previous,
}
#[derive(Clone, Copy)]
struct Frame<N> {
    step: u32,
    node: N,
    axis: Axis,
}
#[derive(Clone, Copy)]
struct Context<N> {
    anchor: Option<N>,
    pseudo: Option<PseudoElement>,
    depth: usize,
}
struct Machine<'a, 'k, T: Tree> {
    selectors: &'a Selectors,
    tree: &'a T,
    options: MatchOptions<'a, T::Node>,
    limits: MatchLimits,
    cancelled: Option<&'a AtomicBool>,
    progress: MatchProgress,
    stack: SmallVec<[Frame<T::Node>; 8]>,
    indices: HashMap<(T::Node, u32), (u64, u64)>,
    pending_indices: usize,
    cache_indices: bool,
    keys: &'k mut KeyCache<'a, T::Node>,
    /// The element being matched and its name, looked up once for every
    /// selector tested against it.
    subject: Option<(T::Node, ElementName<'a>)>,
}
/// One element's matcher: every selector tested against the same subject,
/// with the machine, its caches and the subject's name made once.
pub struct Matcher<'a, 'k, T: Tree> {
    machine: Machine<'a, 'k, T>,
    node: T::Node,
}
impl<'a, T: Tree> Matcher<'a, '_, T> {
    /// Whether the selector list `root` matches the subject: its highest
    /// matching specificity, and the work this took.
    pub fn matches(&mut self, root: u32, work_limit: usize) -> Result<(Option<Specificity>, MatchProgress)> {
        self.machine.progress = MatchProgress::default();
        self.machine.limits.work = work_limit;
        self.machine.tick(1)?;
        let result = self.machine.list(root, self.node, Context { anchor: None, pseudo: self.machine.options.pseudo, depth: 0 }, true)?;
        Ok((result, self.machine.progress))
    }
}
/// An element's id and class tokens, hashed, read from its attributes once.
pub struct Keys<'a> {
    id: Option<(u64, &'a str)>,
    classes: SmallVec<[(u64, &'a str); 8]>,
}
/// The keys of the elements one match visits: the subject and the
/// ancestors and siblings its candidates walk, each read once however
/// many selectors test them. Made per element matched, dropped after.
pub struct KeyCache<'a, N> {
    nodes: SmallVec<[N; 16]>,
    keys: SmallVec<[Keys<'a>; 16]>,
    /// The subject's ancestors' keys, when the caller keeps them (the style
    /// walk does): selectors needing an ancestor key not there are
    /// rejected without a walk.
    ancestors: Option<&'a AncestorFilter>,
}
impl<'a, N: Copy + PartialEq> Default for KeyCache<'a, N> {
    fn default() -> Self {
        Self { nodes: SmallVec::new(), keys: SmallVec::new(), ancestors: None }
    }
}
impl<'a, N: Copy + PartialEq> KeyCache<'a, N> {
    pub fn with_ancestors(ancestors: Option<&'a AncestorFilter>) -> Self {
        Self { ancestors, ..Default::default() }
    }
}
/// Past this many elements a match keeps no keys (a selector walking far).
const KEYS: usize = 96;
impl Selectors {
    /// Highest specificity among matching branches, or None. Tree/state
    /// errors are explicit failures, not false matches or ignored branches.
    pub fn matches<T: Tree>(&self, tree: &T, node: T::Node, options: MatchOptions<'_, T::Node>, limits: MatchLimits, cancelled: Option<&AtomicBool>) -> Result<Option<Specificity>> {
        if !matches!(tree.content(node)?, Content::Element(_)) {
            return Ok(None);
        }
        let mut keys = KeyCache::default();
        let mut matcher = self.matcher(tree, node, options, limits, cancelled, &mut keys)?;
        Ok(matcher.matches(self.root, limits.work)?.0)
    }
    /// A matcher for one element, for testing many selector lists against it.
    pub fn matcher<'a, 'k, T: Tree>(
        &'a self,
        tree: &'a T,
        node: T::Node,
        options: MatchOptions<'a, T::Node>,
        limits: MatchLimits,
        cancelled: Option<&'a AtomicBool>,
        keys: &'k mut KeyCache<'a, T::Node>,
    ) -> Result<Matcher<'a, 'k, T>> {
        if options.visited_style {
            return Err(Error::StateUnavailable);
        }
        let Content::Element(name) = tree.content(node)? else { return Err(Error::Invalid) };
        if let Some(scope) = options.scope {
            tree.content(scope)?;
        }
        let machine = Machine {
            selectors: self,
            tree,
            options,
            limits,
            cancelled,
            progress: MatchProgress::default(),
            stack: SmallVec::new(),
            indices: HashMap::new(),
            pending_indices: 0,
            cache_indices: false,
            keys,
            subject: Some((node, name)),
        };
        Ok(Matcher { machine, node })
    }
    /// Stream document-order results without building an unbounded result
    /// vector. Use include_root for an inclusive subtree scan; DOM element
    /// querySelectorAll uses false. This visits the light tree only.
    #[allow(clippy::too_many_arguments)] // Separate tree/root, matching context, bounds, cancellation and streaming sink.
    pub fn query<T: Tree>(
        &self,
        tree: &T,
        root: T::Node,
        include_root: bool,
        options: MatchOptions<'_, T::Node>,
        limits: MatchLimits,
        cancelled: Option<&AtomicBool>,
        mut sink: impl FnMut(T::Node, Specificity) -> Result<()>,
    ) -> Result<MatchProgress> {
        let mut keys = KeyCache::default();
        let mut machine = Machine {
            selectors: self,
            tree,
            options,
            limits,
            cancelled,
            progress: MatchProgress::default(),
            stack: SmallVec::new(),
            indices: HashMap::new(),
            pending_indices: 0,
            cache_indices: !cfg!(feature = "baseline-selector-nth"),
            keys: &mut keys,
            subject: None,
        };
        machine.tick(1)?;
        tree.content(root)?;
        if let Some(scope) = options.scope {
            tree.content(scope)?;
        }
        let mut at = if include_root { Some(root) } else { tree.first_child(root)? };
        while let Some(node) = at {
            machine.tick(1)?;
            machine.progress.visited += 1;
            if matches!(tree.content(node)?, Content::Element(_))
                && let Some(spec) = machine.list(self.root, node, Context { anchor: None, pseudo: options.pseudo, depth: 0 }, true)?
            {
                if machine.progress.matched >= limits.results {
                    return Err(Error::MatchLimit);
                }
                machine.progress.matched += 1;
                sink(node, spec)?;
            }
            at = machine.successor(node, root)?;
        }
        Ok(machine.progress)
    }
}
impl<'a, T: Tree> Machine<'a, '_, T> {
    fn tick(&mut self, n: usize) -> Result<()> {
        check(self.cancelled)?;
        self.progress.work = self.progress.work.checked_add(n).ok_or(Error::MatchLimit)?;
        if self.progress.work > self.limits.work { Err(Error::MatchLimit) } else { Ok(()) }
    }
    fn push(&mut self, frame: Frame<T::Node>) -> Result<()> {
        if self.stack.len() >= self.limits.backtracking_frames {
            return Err(Error::MatchLimit);
        }
        self.stack.push(frame);
        self.progress.peak_backtracking_frames = self.progress.peak_backtracking_frames.max(self.stack.len());
        Ok(())
    }
    fn child_context(&mut self, ctx: Context<T::Node>) -> Result<Context<T::Node>> {
        self.tick(1)?;
        if ctx.depth >= self.limits.call_depth.min(128) {
            return Err(Error::DepthLimit);
        }
        Ok(Context { anchor: ctx.anchor, pseudo: None, depth: ctx.depth + 1 })
    }
    fn list(&mut self, list: u32, node: T::Node, ctx: Context<T::Node>, maximum: bool) -> Result<Option<Specificity>> {
        self.tick(1)?;
        let range = self.selectors.lists[list as usize].links.clone();
        let mut matched = None;
        for at in range {
            let id = self.selectors.links[at as usize];
            if self.complex(id, node, ctx)? {
                let spec = self.selectors.complexes[id as usize].specificity;
                matched = Some(matched.map_or(spec, |s: Specificity| s.max(spec)));
                if !maximum {
                    break;
                }
            }
        }
        Ok(matched)
    }
    fn complex(&mut self, id: u32, node: T::Node, ctx: Context<T::Node>) -> Result<bool> {
        self.tick(1)?;
        let range = self.selectors.complexes[id as usize].steps.clone();
        if self.selectors.complexes[id as usize].pseudo != ctx.pseudo {
            return Ok(false);
        }
        // Fast reject: the filter is the subject's ancestors', so only at
        // the top level (ctx.depth 0, node the subject), not inside :is().
        if ctx.depth == 0
            && let Some(filter) = self.keys.ancestors
        {
            let needed = self.selectors.complexes[id as usize].ancestors.clone();
            if !needed.is_empty() && !self.selectors.ancestor_hashes[needed.start as usize..needed.end as usize].iter().all(|&h| filter.may_contain(h)) {
                self.progress.rejected += 1;
                return Ok(false);
            }
        }
        if range.end - range.start == 1 {
            return self.compound(range.start, node, ctx);
        }
        let base = self.stack.len();
        let result = self.walk_complex(range, node, ctx, base);
        self.stack.truncate(base);
        result
    }
    fn walk_complex(&mut self, range: Range<u32>, node: T::Node, ctx: Context<T::Node>, base: usize) -> Result<bool> {
        self.push(Frame { step: range.end - 1, node, axis: Axis::Single })?;
        while self.stack.len() > base {
            self.tick(1)?;
            let frame = self.stack.pop().unwrap();
            self.progress.steps += 1;
            let next = match frame.axis {
                Axis::Single => None,
                Axis::Parent => self.tree.parent(frame.node)?,
                Axis::Previous => self.previous_element(frame.node)?,
            };
            if let Some(node) = next {
                self.push(Frame { node, ..frame })?;
            }
            if self.compound(frame.step, frame.node, ctx)? {
                if frame.step == range.start {
                    return Ok(true);
                }
                let relation = self.selectors.steps[frame.step as usize].relation;
                let (candidate, axis) = match relation {
                    Relation::Child => (self.tree.parent(frame.node)?, Axis::Single),
                    Relation::Descendant => (self.tree.parent(frame.node)?, Axis::Parent),
                    Relation::Next => (self.previous_element(frame.node)?, Axis::Single),
                    Relation::Later => (self.previous_element(frame.node)?, Axis::Previous),
                    Relation::None => return Err(Error::Invalid),
                };
                if let Some(node) = candidate {
                    self.push(Frame { step: frame.step - 1, node, axis })?;
                }
            }
        }
        Ok(false)
    }
    fn namespace(&self, expected: u32, actual: &str) -> bool {
        expected == ANY || if expected == NONE { actual.is_empty() } else { self.selectors.symbol(expected) == actual }
    }
    fn compound(&mut self, step: u32, node: T::Node, ctx: Context<T::Node>) -> Result<bool> {
        self.tick(1)?;
        self.progress.compounds += 1;
        let step = &self.selectors.steps[step as usize];
        let tests = step.tests.clone();
        let ns = step.namespace;
        let element = match self.subject {
            Some((subject, name)) if subject == node => name,
            _ => match self.tree.content(node)? {
                Content::Element(e) => e,
                _ => return if step.featureless { self.featureless(tests, node, ctx) } else { Ok(false) },
            },
        };
        if !self.namespace(ns, element.namespace) {
            return Ok(false);
        }
        let html = self.options.html_document && element.namespace == HTML;
        let mut pseudo = None;
        for at in tests {
            self.tick(1)?;
            self.progress.tests += 1;
            let matches = match self.selectors.tests[at as usize] {
                Test::Type { ns, name } => self.namespace(ns, element.namespace) && (name == ANY || name_equal(self.selectors.symbol(name), element.local_name, html)),
                Test::Id(id) if !self.options.quirks => {
                    let (hash, symbol) = (self.selectors.symbol_hashes[id as usize], self.selectors.symbol(id));
                    self.keys(node)?.id.is_some_and(|(h, s)| h == hash && s == symbol)
                }
                Test::Class(class) if !self.options.quirks => {
                    let (hash, symbol) = (self.selectors.symbol_hashes[class as usize], self.selectors.symbol(class));
                    self.keys(node)?.classes.iter().any(|&(h, s)| h == hash && s == symbol)
                }
                Test::Id(id) => self.word_attribute(node, "id", self.selectors.symbol(id), false, self.options.quirks)?,
                Test::Class(class) => self.word_attribute(node, "class", self.selectors.symbol(class), true, self.options.quirks)?,
                Test::Attr { ns, name, value, failure, op, case } => self.attribute(node, ns, name, value, failure, op, case, html)?,
                Test::Structural(s) => self.structural(s, node, ctx)?,
                Test::State(state) => {
                    if !self.options.visited_style && state == State::Visited {
                        false
                    } else {
                        self.tree.state(node, pseudo, if !self.options.visited_style && state == State::Link { State::AnyLink } else { state })?
                    }
                }
                Test::Pseudo(p) => {
                    pseudo = Some(p);
                    ctx.pseudo == Some(p)
                }
                Test::Logical { mode, list } => {
                    let child = self.child_context(ctx)?;
                    match mode {
                        Logical::Has => self.has(list, node, child)?,
                        Logical::Is => self.list(list, node, child, false)?.is_some(),
                        Logical::Not => self.list(list, node, child, false)?.is_none(),
                    }
                }
                Test::Nth(id) => {
                    let child = self.child_context(ctx)?;
                    self.nth(id, node, child)?
                }
                Test::Lang { start, end } => self.language(node, start..end)?,
                Test::Dir(rtl) => self.tree.direction(node)? == rtl,
            };
            if !matches {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn featureless(&mut self, tests: Range<u32>, node: T::Node, ctx: Context<T::Node>) -> Result<bool> {
        // Eligibility is compiled separately from boolean matching: a
        // virtual root is never selected accidentally by :not(.class).
        // An implicit default namespace does not block a featureless root.
        for at in tests {
            self.tick(1)?;
            let matched = match self.selectors.tests[at as usize] {
                Test::Structural(Structural::Scope) => self.options.scope == Some(node),
                Test::Structural(Structural::Anchor) => ctx.anchor == Some(node),
                Test::Logical { mode, list } => {
                    let child = self.child_context(ctx)?;
                    match mode {
                        Logical::Is => self.list(list, node, child, false)?.is_some(),
                        Logical::Not => self.list(list, node, child, false)?.is_none(),
                        Logical::Has => self.has(list, node, child)?,
                    }
                }
                _ => false,
            };
            if !matched {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// The node's id and class tokens, read once per match.
    fn keys(&mut self, node: T::Node) -> Result<&Keys<'a>> {
        if let Some(i) = self.keys.nodes.iter().position(|&n| n == node) {
            return Ok(&self.keys.keys[i]);
        }
        let mut keys = Keys { id: None, classes: SmallVec::new() };
        for a in self.tree.attributes(node)? {
            self.tick(1)?;
            if !a.namespace.is_empty() {
                continue;
            }
            if a.local_name == "class" {
                self.tick(a.value.len())?;
                for token in a.value.split(white).filter(|s| !s.is_empty()) {
                    keys.classes.push((token_hash(token), token));
                }
            } else if a.local_name == "id" {
                self.tick(a.value.len())?;
                keys.id = Some((token_hash(a.value), a.value));
            }
        }
        if self.keys.nodes.len() >= KEYS {
            // Keep the subject (first) and the newest: drop the oldest other.
            self.keys.nodes.remove(1);
            self.keys.keys.remove(1);
        }
        self.keys.nodes.push(node);
        self.keys.keys.push(keys);
        Ok(self.keys.keys.last().unwrap())
    }
    fn word_attribute(&mut self, node: T::Node, name: &str, word: &str, split: bool, insensitive: bool) -> Result<bool> {
        for a in self.tree.attributes(node)? {
            self.tick(1)?;
            if a.namespace.is_empty() && a.local_name == name {
                self.tick(a.value.len() + word.len())?;
                return Ok(if split { a.value.split(white).any(|s| !s.is_empty() && equal(s, word, insensitive)) } else { equal(a.value, word, insensitive) });
            }
        }
        Ok(false)
    }
    #[allow(clippy::too_many_arguments)]
    fn attribute(&mut self, node: T::Node, ns: u32, name: u32, value: u32, failure: u32, op: AttrOp, case: Case, html: bool) -> Result<bool> {
        let wanted = self.selectors.symbol(name);
        for a in self.tree.attributes(node)? {
            self.tick(1)?;
            if !self.namespace(ns, a.namespace) || !name_equal(wanted, a.local_name, html && a.namespace.is_empty()) {
                continue;
            }
            if matches!(op, AttrOp::Exists) {
                return Ok(true);
            }
            let value = self.selectors.symbol(value);
            let insensitive = match case {
                Case::Ascii => true,
                Case::Sensitive => false,
                Case::Default => html && a.namespace.is_empty() && html_insensitive(a.local_name),
            };
            self.tick(a.value.len().saturating_add(value.len()))?;
            let matched = match op {
                AttrOp::Exists => true,
                AttrOp::Equal => equal(a.value, value, insensitive),
                AttrOp::Includes => !value.is_empty() && !value.chars().any(white) && a.value.split(white).any(|s| equal(s, value, insensitive)),
                AttrOp::Dash => equal(a.value, value, insensitive) || (a.value.len() > value.len() && a.value.as_bytes()[value.len()] == b'-' && equal(&a.value[..value.len()], value, insensitive)),
                AttrOp::Prefix => !value.is_empty() && a.value.get(..value.len()).is_some_and(|s| equal(s, value, insensitive)),
                AttrOp::Suffix => !value.is_empty() && a.value.len() >= value.len() && a.value.get(a.value.len() - value.len()..).is_some_and(|s| equal(s, value, insensitive)),
                AttrOp::Contains => !value.is_empty() && if insensitive { self.contains_ascii(a.value, value, failure)? } else { memchr::memmem::find(a.value.as_bytes(), value.as_bytes()).is_some() },
            };
            if matched {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn contains_ascii(&mut self, haystack: &str, needle: &str, failure: u32) -> Result<bool> {
        if failure == ANY {
            return Err(Error::Invalid);
        }
        let mut matched = 0;
        let needle = needle.as_bytes();
        for byte in haystack.bytes() {
            self.tick(1)?;
            while matched > 0 && !byte.eq_ignore_ascii_case(&needle[matched]) {
                matched = self.selectors.failures[failure as usize + matched - 1] as usize;
                self.tick(1)?;
            }
            if byte.eq_ignore_ascii_case(&needle[matched]) {
                matched += 1;
                if matched == needle.len() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn root(&mut self, node: T::Node) -> Result<bool> {
        self.tick(1)?;
        Ok(match self.tree.parent(node)? {
            None => true,
            Some(p) => matches!(self.tree.content(p)?, Content::Document),
        })
    }
    fn structural(&mut self, s: Structural, node: T::Node, ctx: Context<T::Node>) -> Result<bool> {
        match s {
            Structural::Root => self.root(node),
            Structural::Scope => {
                if let Some(scope) = self.options.scope {
                    Ok(node == scope)
                } else {
                    self.root(node)
                }
            }
            Structural::Anchor => Ok(ctx.anchor == Some(node)),
            Structural::Empty => {
                let mut child = self.tree.first_child(node)?;
                while let Some(c) = child {
                    self.tick(1)?;
                    match self.tree.content(c)? {
                        Content::Element(_) => return Ok(false),
                        Content::Text(s) if !s.is_empty() => return Ok(false),
                        _ => {}
                    }
                    child = self.tree.next_sibling(c)?;
                }
                Ok(true)
            }
            Structural::First => Ok(self.previous_element(node)?.is_none()),
            Structural::Last => Ok(self.next_element(node)?.is_none()),
            Structural::Only => Ok(self.previous_element(node)?.is_none() && self.next_element(node)?.is_none()),
            Structural::FirstType => self.only_type(node, false),
            Structural::LastType => self.only_type(node, true),
            Structural::OnlyType => Ok(self.only_type(node, false)? && self.only_type(node, true)?),
        }
    }
    fn previous_element(&mut self, node: T::Node) -> Result<Option<T::Node>> {
        let mut at = self.tree.previous_sibling(node)?;
        while let Some(n) = at {
            self.tick(1)?;
            if matches!(self.tree.content(n)?, Content::Element(_)) {
                return Ok(at);
            }
            at = self.tree.previous_sibling(n)?;
        }
        Ok(None)
    }
    fn next_element(&mut self, node: T::Node) -> Result<Option<T::Node>> {
        let mut at = self.tree.next_sibling(node)?;
        while let Some(n) = at {
            self.tick(1)?;
            if matches!(self.tree.content(n)?, Content::Element(_)) {
                return Ok(at);
            }
            at = self.tree.next_sibling(n)?;
        }
        Ok(None)
    }
    fn same_type(&self, a: T::Node, b: T::Node) -> Result<bool> {
        Ok(match (self.tree.content(a)?, self.tree.content(b)?) {
            (Content::Element(a), Content::Element(b)) => a.namespace == b.namespace && a.local_name == b.local_name,
            _ => false,
        })
    }
    fn only_type(&mut self, node: T::Node, last: bool) -> Result<bool> {
        let mut at = if last { self.next_element(node)? } else { self.previous_element(node)? };
        while let Some(n) = at {
            self.tick(1)?;
            if self.same_type(node, n)? {
                return Ok(false);
            }
            at = if last { self.next_element(n)? } else { self.previous_element(n)? };
        }
        Ok(true)
    }
    fn nth(&mut self, id: u32, node: T::Node, ctx: Context<T::Node>) -> Result<bool> {
        let nth = &self.selectors.nth[id as usize];
        let formula = nth.formula;
        let list = nth.list;
        let typed = nth.of_type;
        let last = nth.last;
        if formula.a <= 0 && formula.b <= 0 {
            return Ok(false);
        }
        if self.cache_indices {
            let tag = if typed { NONE } else { list };
            if let Some(&(index, total)) = self.indices.get(&(node, tag)) {
                return Ok(formula.matches(if index == 0 {
                    0
                } else if last {
                    total - index + 1
                } else {
                    index
                }));
            }
            self.build_indices(node, tag, ctx)?;
            let &(index, total) = self.indices.get(&(node, tag)).ok_or(Error::InvalidNode)?;
            return Ok(formula.matches(if index == 0 {
                0
            } else if last {
                total - index + 1
            } else {
                index
            }));
        }
        if list != ANY && self.list(list, node, ctx, false)?.is_none() {
            return Ok(false);
        }
        let mut count = 1u64;
        let mut at = if last { self.next_element(node)? } else { self.previous_element(node)? };
        while let Some(n) = at {
            self.tick(1)?;
            if (!typed || self.same_type(node, n)?) && (list == ANY || self.list(list, n, ctx, false)?.is_some()) {
                count = count.checked_add(1).ok_or(Error::MatchLimit)?;
            }
            at = if last { self.next_element(n)? } else { self.previous_element(n)? };
        }
        Ok(formula.matches(count))
    }
    fn build_indices(&mut self, node: T::Node, tag: u32, ctx: Context<T::Node>) -> Result<()> {
        // Borrow immutable names from the tree, without copying strings or
        // making per-type vectors. A second pass sets totals for reverse nth.
        let tree = self.tree;
        let mut types = HashMap::<(&'a str, &'a str), u64>::new();
        let mut pending = SmallVec::<[(T::Node, u64); 32]>::new();
        let mut count = 0u64;
        let mut at = match tree.parent(node)? {
            Some(parent) => tree.first_child(parent)?,
            None => Some(node),
        };
        while let Some(n) = at {
            self.tick(1)?;
            if let Content::Element(element) = tree.content(n)? {
                if self.indices.len().saturating_add(self.pending_indices) >= self.limits.index_entries {
                    return Err(Error::MatchLimit);
                }
                let index = if tag == NONE {
                    self.tick(element.namespace.len() + element.local_name.len())?;
                    let count = types.entry((element.namespace, element.local_name)).or_default();
                    *count += 1;
                    *count
                } else if tag == ANY || self.list(tag, n, ctx, false)?.is_some() {
                    count += 1;
                    count
                } else {
                    0
                };
                // A nested filter may have populated its own cache while
                // this group's indices were pending; charge both together.
                if self.indices.len().saturating_add(self.pending_indices) >= self.limits.index_entries {
                    return Err(Error::MatchLimit);
                }
                self.pending_indices += 1;
                self.progress.peak_pending_indices = self.progress.peak_pending_indices.max(self.pending_indices);
                pending.push((n, index));
            }
            at = tree.next_sibling(n)?;
        }
        for (n, index) in pending {
            self.tick(1)?;
            let total = if tag == NONE {
                let Content::Element(element) = tree.content(n)? else {
                    return Err(Error::InvalidNode);
                };
                types[&(element.namespace, element.local_name)]
            } else {
                count
            };
            self.pending_indices -= 1;
            self.indices.insert((n, tag), (index, total));
        }
        self.progress.index_entries = self.indices.len();
        self.progress.index_capacity = self.indices.capacity();
        Ok(())
    }
    fn has(&mut self, list: u32, anchor: T::Node, mut ctx: Context<T::Node>) -> Result<bool> {
        ctx.anchor = Some(anchor);
        let links = self.selectors.lists[list as usize].links.clone();
        for at in links {
            let id = self.selectors.links[at as usize];
            let complex = &self.selectors.complexes[id as usize];
            let single = complex.steps.end - complex.steps.start == 2;
            let relation = complex.relative;
            if matches!(relation, Relation::Next | Relation::Later) {
                let mut sibling = self.next_element(anchor)?;
                while let Some(root) = sibling {
                    self.tick(1)?;
                    if single {
                        if self.complex(id, root, ctx)? {
                            return Ok(true);
                        }
                    } else {
                        let mut node = Some(root);
                        while let Some(n) = node {
                            self.tick(1)?;
                            if self.complex(id, n, ctx)? {
                                return Ok(true);
                            }
                            node = self.successor(n, root)?;
                        }
                    }
                    if single && matches!(relation, Relation::Next) {
                        break;
                    }
                    sibling = self.next_element(root)?;
                }
            } else {
                let mut node = self.tree.first_child(anchor)?;
                while let Some(n) = node {
                    self.tick(1)?;
                    if matches!(self.tree.content(n)?, Content::Element(_)) && self.complex(id, n, ctx)? {
                        return Ok(true);
                    }
                    node = if single && matches!(relation, Relation::Child) { self.tree.next_sibling(n)? } else { self.successor(n, anchor)? };
                }
            }
        }
        Ok(false)
    }
    fn successor(&mut self, node: T::Node, boundary: T::Node) -> Result<Option<T::Node>> {
        self.tick(1)?;
        if let Some(child) = self.tree.first_child(node)? {
            return Ok(Some(child));
        }
        let mut at = node;
        loop {
            self.tick(1)?;
            if at == boundary {
                return Ok(None);
            }
            if let Some(sibling) = self.tree.next_sibling(at)? {
                return Ok(Some(sibling));
            }
            let Some(parent) = self.tree.parent(at)? else {
                return Ok(None);
            };
            at = parent;
        }
    }
    fn language(&mut self, node: T::Node, ranges: Range<u32>) -> Result<bool> {
        let mut at = Some(node);
        let mut language = None;
        while let Some(n) = at {
            self.tick(1)?;
            if let Content::Element(e) = self.tree.content(n)? {
                let plain = matches!(e.namespace, HTML | "http://www.w3.org/2000/svg");
                for a in self.tree.attributes(n)? {
                    self.tick(1)?;
                    if a.local_name == "lang" {
                        if a.namespace == "http://www.w3.org/XML/1998/namespace" {
                            language = Some(a.value);
                            break;
                        }
                        if plain && a.namespace.is_empty() {
                            language = Some(a.value);
                        }
                    }
                }
                if language.is_some() {
                    break;
                }
            }
            at = self.tree.parent(n)?;
        }
        let language = language.or(self.options.document_language).unwrap_or("");
        self.tick(language.len())?;
        for id in ranges {
            let range = self.selectors.symbol(self.selectors.languages[id as usize]);
            self.tick(range.len() + language.len())?;
            if language_range(range, language) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
fn equal(a: &str, b: &str, insensitive: bool) -> bool {
    if insensitive { a.eq_ignore_ascii_case(b) } else { a == b }
}
fn name_equal(selector: &str, actual: &str, html: bool) -> bool {
    if html { selector.bytes().map(|b| b.to_ascii_lowercase()).eq(actual.bytes()) } else { selector == actual }
}
fn html_insensitive(name: &str) -> bool {
    matches!(
        name,
        "accept"
            | "accept-charset"
            | "align"
            | "alink"
            | "axis"
            | "bgcolor"
            | "charset"
            | "checked"
            | "clear"
            | "codetype"
            | "color"
            | "compact"
            | "declare"
            | "defer"
            | "dir"
            | "direction"
            | "disabled"
            | "enctype"
            | "face"
            | "frame"
            | "hreflang"
            | "http-equiv"
            | "lang"
            | "language"
            | "link"
            | "media"
            | "method"
            | "multiple"
            | "nohref"
            | "noresize"
            | "noshade"
            | "nowrap"
            | "readonly"
            | "rel"
            | "rev"
            | "rules"
            | "scope"
            | "scrolling"
            | "selected"
            | "shape"
            | "target"
            | "text"
            | "type"
            | "valign"
            | "valuetype"
            | "vlink"
    )
}
fn language_range(range: &str, language: &str) -> bool {
    if range.is_empty() {
        return language.is_empty();
    }
    if language.is_empty() {
        return false;
    }
    let mut requested = range.split('-');
    let mut actual = language.split('-');
    let first = requested.next().unwrap();
    let Some(tag) = actual.next() else {
        return false;
    };
    if first != "*" && !first.eq_ignore_ascii_case(tag) {
        return false;
    }
    for wanted in requested {
        if wanted == "*" {
            continue;
        }
        loop {
            let Some(tag) = actual.next() else {
                return false;
            };
            if wanted.eq_ignore_ascii_case(tag) {
                break;
            }
            if tag.len() == 1 {
                return false;
            }
        }
    }
    true
}
