use super::*;
use crate::css::selectors::PseudoElement;
use crate::{
    css::selectors,
    dom::{Document, NodeId, NodeType},
};
use smallvec::SmallVec;
use std::num::NonZeroU32;

#[derive(Clone, Copy, Default)]
pub struct JobOptions<'a> {
    pub generation: u64,
    pub quirks: bool,
    /// Immutable state snapshot, including semantic direction. Missing state
    /// is an explicit matcher error if a selector needs it.
    pub states: Option<&'a dyn selectors::StateSource>,
    /// The styles of an earlier revision of the same document, with the
    /// same rules, defaults and states: an element whose own attributes,
    /// siblings and parent's style are unchanged since keeps its style
    /// without matching or computing.
    pub previous: Option<&'a StyleTree>,
    /// Declarations of the animation origin by element (its running CSS
    /// animations sampled now): applied after its inline style, so they
    /// win over normal author declarations and lose to important ones.
    pub animated: Option<&'a crate::dom::NodeMap<Vec<p::Declaration>>>,
    /// Query containers' content-box sizes (CSS px) as the last layout had
    /// them, for `@container` rules; a container not in it answers nothing.
    pub containers: Option<&'a crate::dom::NodeMap<[f64; 2]>>,
    /// Query containers whose size changed since `previous` was styled:
    /// what they hold is matched again (their `@container` rules), the rest
    /// kept as `previous` has it.
    pub resized: &'a [NodeId],
    /// The rules are `previous`'s generation's (the first) but for the
    /// sheets of the shadow trees these hosts hold (scripts adding a
    /// component's styles): `previous` serves all the same, and what those
    /// sheets can reach (each host, its children, its shadow tree and
    /// those nested in it: `:host`, `::slotted`, `::part`) is styled again.
    pub changed_within: Option<(u64, &'a [NodeId])>,
}
#[derive(Clone, Copy, Debug)]
pub struct JobLimits {
    pub nodes: usize,
    pub elements: usize,
    pub slots: usize,
    pub depth: usize,
    pub work: usize,
    pub retained_bytes: usize,
    pub inline_bytes: usize,
    pub inline_declarations: usize,
    /// Optional per-job font request interning. Saturation stops insertion;
    /// zero disables it. Pool capacity counts toward retained_bytes.
    pub font_cache_entries: usize,
    pub css: super::super::Limits,
    pub property: p::Limits,
    pub cascade: cascade::Limits,
    pub matching: selectors::MatchLimits,
    pub style: Limits,
}
impl Default for JobLimits {
    fn default() -> Self {
        Self {
            nodes: 2_000_000,
            elements: 1_000_000,
            slots: 2_000_000,
            depth: 1024,
            work: 4_000_000,
            retained_bytes: 512 * 1024 * 1024,
            inline_bytes: 1024 * 1024,
            inline_declarations: 16384,
            font_cache_entries: 4096,
            css: Default::default(),
            property: Default::default(),
            cascade: Default::default(),
            matching: Default::default(),
            style: Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub visited: usize,
    pub styled: usize,
    /// Of those, styles kept from the previous tree.
    pub reused: usize,
    pub finished: bool,
}
/// Unsupported/unknown declarations remain visible in diagnostics; an eval
/// cannot turn a partially supported stylesheet into a browser pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub stylesheet_issues: usize,
    pub inline_invalid: usize,
    pub inline_unsupported: usize,
    pub presentation_invalid: usize,
    pub presentation_unsupported: usize,
    /// Declarations dropped at computed-value time (see `Dependencies::fallbacks`).
    pub value_fallbacks: usize,
}
#[derive(Clone)]
struct Styled {
    node: NodeId,
    style: Style,
    parent_box: Option<NonZeroU32>,
    /// One past the last entry of this element's subtree (its pseudo-
    /// elements and descendants follow it): the range to copy when the
    /// subtree is unchanged.
    end: u32,
    /// Which pseudo-element this entry styles, if not the element itself.
    pseudo: Option<PseudoElement>,
    /// The rules that matched (ids and specificities), for an element whose
    /// inline style changes later: its cascade without matching again.
    matched: Option<Arc<[(u32, selectors::Specificity)]>>,
}
/// Completed styles for one immutable connected light document. No DOM,
/// stylesheet, parent or state-source borrow survives completion.
#[derive(Clone)]
pub struct StyleTree {
    document: u64,
    revision: u64,
    generation: u64,
    slots: Vec<Option<NonZeroU32>>,
    styles: crate::chunked::Chunked<Styled>,
    /// `::before` and `::after` styles that generate content, by their
    /// element: entries in `styles` without a slot of their own.
    pseudos: std::collections::HashMap<(NodeId, PseudoElement), u32>,
    report: Report,
    /// Made from a previous tree whose every difference is paint-only, over
    /// an unchanged structure: a layout made for that tree serves this one.
    geometry_kept: bool,
    /// The entries (elements, not pseudo-elements) with an animation
    /// named, ascending: found once by a full job, kept by patches.
    animated: Vec<u32>,
    /// Likewise, those with a `backdrop-filter`.
    backdrops: Vec<u32>,
}
/// Whether a style names an animation.
fn names_animation(style: &Style) -> bool {
    matches!(style.value(Property::AnimationName), Value::Animation(a) if a.items().iter().any(|i| matches!(i, crate::css::properties::animation::Item::Name(_))))
}
fn has_backdrop(style: &Style) -> bool {
    matches!(style.value(Property::BackdropFilter), Value::Filters(f) if !f.0.is_empty())
}
/// `i` in or out of a kept list (ascending) as `holds` says.
fn keep(list: &mut Vec<u32>, i: u32, holds: bool) {
    match (list.binary_search(&i), holds) {
        (Err(at), true) => list.insert(at, i),
        (Ok(at), false) => {
            list.remove(at);
        }
        _ => {}
    }
}
/// What a box or text span takes its style from: an element, or one of its
/// pseudo-elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StyleKey {
    pub node: NodeId,
    pub pseudo: Option<PseudoElement>,
}
impl StyleKey {
    pub fn pseudo(node: NodeId, pseudo: PseudoElement) -> Self {
        Self { node, pseudo: Some(pseudo) }
    }
}
impl From<NodeId> for StyleKey {
    fn from(node: NodeId) -> Self {
        Self { node, pseudo: None }
    }
}
impl StyleTree {
    /// The style of a pseudo-element that generates content, if it does.
    pub fn pseudo(&self, node: NodeId, pseudo: PseudoElement) -> Option<&Style> {
        self.pseudos.get(&(node, pseudo)).map(|&i| &self.styles[i as usize].style)
    }
    /// This tree for a document that has not changed since it was styled
    /// (`changes_since` its revision empty), under the same rules: what a
    /// whole job over it would make, each style taken as it is, without
    /// the job (a millisecond and more on GitHub, every animation frame).
    /// None for another document or other rules.
    pub fn unchanged(&self, document: &Document, generation: u64) -> Option<StyleTree> {
        if self.document != document.id() || self.generation != generation || self.styles.is_empty() {
            return None;
        }
        let mut tree = self.clone();
        tree.geometry_kept = true;
        Some(tree)
    }
    /// Its styles not shared with `other` (all of them without one): a
    /// patched copy shares all but the chunks it patched, so what is new
    /// since `other` is found without looking at every style.
    pub fn styles_unshared<'s>(&'s self, other: Option<&'s StyleTree>) -> impl Iterator<Item = (NodeId, &'s Style)> + 's {
        let all: Box<dyn Iterator<Item = usize> + 's> = match other {
            Some(other) => Box::new(self.styles.unshared(&other.styles)),
            None => Box::new(0..self.styles.len()),
        };
        all.map(|i| &self.styles[i]).map(|s| (s.node, &s.style))
    }
    /// Whether every element of `other` has a transform where this tree's
    /// has one, entry for entry: compared only where they don't share
    /// their styles (a patched tree shares all but what it patched).
    pub fn same_transformed(&self, other: &StyleTree) -> bool {
        let has = |s: &Style| matches!(s.value(super::Property::Transform), super::Value::Transforms(_));
        self.styles.len() == other.styles.len()
            && self.styles.unshared(&other.styles).all(|i| {
                let (a, b) = (&self.styles[i], &other.styles[i]);
                a.node == b.node && a.pseudo == b.pseudo && has(&a.style) == has(&b.style)
            })
    }
    /// The elements with an animation named, in the order they were
    /// styled (document order, then any a patch added): a scan of the
    /// styles, not a walk of the document (on GitHub's 5,000 nodes the
    /// walk took most of a millisecond each animation frame).
    pub fn animation_candidates(&self) -> impl Iterator<Item = (NodeId, &Style)> {
        // (Kept, not scanned: every style's animation-name was read each
        // frame, 7% of apple.com's scrolling.)
        self.animated.iter().map(|&i| &self.styles[i as usize]).map(|s| (s.node, &s.style))
    }
    /// The elements with a `backdrop-filter`, kept as the animated ones.
    pub fn backdrop_filtered(&self) -> impl Iterator<Item = (NodeId, &Style)> {
        self.backdrops.iter().map(|&i| &self.styles[i as usize]).map(|s| (s.node, &s.style))
    }
    /// Whether `node` has a pseudo-element style (generated content).
    pub fn has_pseudos(&self, node: NodeId) -> bool {
        !self.pseudos.is_empty() && self.pseudos.keys().any(|(n, _)| *n == node)
    }
    pub fn pseudo_count(&self) -> usize {
        self.pseudos.len()
    }
    pub fn get(&self, document: &Document, node: NodeId, generation: u64) -> Result<Option<&Style>> {
        if self.generation != generation || self.document != document.id() || self.revision != document.revision() {
            return Err(Error::Stale);
        }
        Ok(self.index(node).map(|i| &self.styles[i].style))
    }
    fn index(&self, node: NodeId) -> Option<usize> {
        if node.document() != self.document {
            return None;
        }
        let i = self.slots.get(node.slot() as usize)?.map(|v| v.get() as usize - 1)?;
        (self.styles[i].node == node).then_some(i)
    }
    /// Internal access to a frozen snapshot, after its owner validates the document.
    pub(crate) fn snapshot(&self, key: impl Into<StyleKey>) -> Option<&Style> {
        let key = key.into();
        match key.pseudo {
            None => self.index(key.node).map(|i| &self.styles[i].style),
            Some(p) => self.pseudo(key.node, p),
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// The rules `node` matched, as recorded (ids into the rules of this
    /// tree's generation; none when not recorded).
    pub fn matched(&self, node: NodeId) -> Option<&[(u32, selectors::Specificity)]> {
        self.index(node).and_then(|i| self.styles[i].matched.as_deref())
    }
    /// Whether a layout of the previous tree's document serves this one
    /// (see the field).
    pub fn geometry_kept(&self) -> bool {
        self.geometry_kept
    }
    /// Whether this tree styles the same elements and pseudo-elements as
    /// `old`, in the same order, every difference leaving the box tree as
    /// it is (`Style::keeps_boxes`).
    pub fn keeps_boxes_of(&self, old: &StyleTree) -> bool {
        if self.styles.len() != old.styles.len() || self.document != old.document {
            return false;
        }
        self.styles
            .iter()
            .zip(old.styles.iter())
            .all(|(a, b)| a.node == b.node && a.pseudo == b.pseudo && (a.style.overrides_share(&b.style) || a.style.same_as(&b.style) || a.style.keeps_boxes(&b.style)))
    }
    pub fn document_revision(&self) -> u64 {
        self.revision
    }
    /// The defaults these styles were computed against (the root's; every
    /// style shares them); None for an empty document.
    pub fn defaults(&self) -> Option<Arc<Defaults>> {
        self.styles.first().map(|s| s.style.defaults())
    }
    pub fn report(&self) -> Report {
        self.report
    }
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Style)> {
        self.styles.iter().map(|v| (v.node, &v.style))
    }
    pub fn storage_payload_bytes(&self) -> usize {
        self.styles.capacity() * size_of::<Styled>() + self.slots.capacity() * size_of::<Option<NonZeroU32>>()
    }
}
/// The keys an attribute change touches: the attribute's name, and for
/// id/class the values that came or went.
pub(crate) fn attribute_keys(name: &str, old: Option<&str>, new: Option<&str>) -> Vec<String> {
    let mut keys = vec![format!("[{name}")];
    let (old, new) = (old.unwrap_or(""), new.unwrap_or(""));
    if name == "class" {
        let before: std::collections::HashSet<&str> = old.split_ascii_whitespace().collect();
        let after: std::collections::HashSet<&str> = new.split_ascii_whitespace().collect();
        keys.extend(before.symmetric_difference(&after).map(|c| format!(".{c}")));
    } else if name == "id" && old != new {
        keys.push(format!("#{old}"));
        keys.push(format!("#{new}"));
    }
    keys
}
/// What patching a tree in place came to (see `Job::patch`).
#[derive(Clone, Debug)]
pub struct Patched {
    /// Every difference is paint-only: a layout of the tree before serves.
    pub geometry_kept: bool,
    /// Elements whose style changed.
    pub changed: usize,
    /// Those elements, and the hosts of the pseudo-elements that changed.
    pub nodes: Vec<NodeId>,
    /// Pseudo-elements whose style changed.
    pub pseudos: usize,
    /// Every difference leaves the box tree as it is (`Style::keeps_boxes`).
    pub boxes_kept: bool,
}
/// Worker job: parent-before-child traversal, bounded steps and a readable
/// completed prefix. The caller sends progress/results to the UI and checks
/// its generation before installing them. A failure poisons the whole job.
pub struct Job<'a> {
    document: &'a Document,
    rules: &'a cascade::RuleSet,
    defaults: Arc<Defaults>,
    options: JobOptions<'a>,
    limits: JobLimits,
    cancelled: Option<&'a AtomicBool>,
    next: Option<NodeId>,
    depth: usize,
    root: Option<usize>,
    work: usize,
    progress: Progress,
    failed: bool,
    scratch: cascade::MatchScratch,
    /// The element last matched and its tree scopes (`dom`).
    scopes_of: Option<(NodeId, [Option<u64>; 4])>,
    /// Styles a like sibling may take (style sharing): recent elements
    /// whose matching met nothing a sibling could answer otherwise.
    sharing: Vec<Sharable>,
    tree: StyleTree,
    account: memory::Account,
    fonts: font_pool::FontPool,
    customs: compute::CustomsMemo,
    /// With OLO_STYLE_TIMING set: time in hints and inline parsing, matching,
    /// computing, pseudo-elements, and the candidates and elements counted.
    timing: Option<Timing>,
    /// The keys of the element being styled's ancestors, kept as the walk
    /// descends and climbs, for the matcher's fast reject.
    ancestors: selectors::AncestorFilter,
    /// The elements on the way down, with their keys, to take out again.
    chain: Vec<(NodeId, SmallVec<[u64; 6]>)>,
    /// Incremental restyle: the previous tree and what changed since it
    /// (elements whose attributes changed; nodes whose children changed).
    previous: Option<Incremental<'a>>,
    /// Per styled entry: whether it is the same as in the previous tree,
    /// so its children may keep theirs.
    same: Vec<bool>,
    /// Per style: whether what children inherit is the same as before (true
    /// when `same`; also when the element changed only in what children
    /// never inherit).
    inherited_same: Vec<bool>,
    /// Elements whose subtree is still open (their entry's index), to set
    /// their `end` when the walk leaves them.
    open: Vec<(NodeId, usize)>,
}
struct Incremental<'a> {
    tree: &'a StyleTree,
    nodes: crate::dom::NodeSet,
    /// Elements whose inline style changed and nothing else: their own
    /// values recompute, what selectors match does not.
    style_only: crate::dom::NodeSet,
    children: crate::dom::NodeSet,
    /// Nodes with a change somewhere below them (ancestors-or-self of the
    /// changed): a reused element outside this set keeps its whole subtree.
    below: crate::dom::NodeSet,
    /// So far, every change is paint-only over an unchanged structure.
    geometry_kept: bool,
    /// Why not, when not (diagnostics).
    why_not: Option<String>,
    /// The rules are another generation's: kept styles' recorded rule ids
    /// point into the old rules, so they are not kept.
    rules_changed: bool,
}
/// The elements of `host`'s composed subtree, itself included: its
/// children, its shadow tree, and the trees nested in those.
fn composed_subtree(document: &Document, host: NodeId, nodes: &mut crate::dom::NodeSet) {
    let mut stack = vec![host];
    while let Some(n) = stack.pop() {
        if document.node_type(n).ok() == Some(crate::dom::NodeType::Element) {
            nodes.insert(n);
        }
        if let Some(root) = document.shadow_root(n) {
            stack.push(root);
        }
        if let Ok(children) = document.children(n) {
            stack.extend(children);
        }
    }
}
/// An element's style a later sibling with its parent, name and
/// attributes takes without matching (Servo and Blink share styles so).
struct Sharable {
    parent: Option<usize>,
    node: NodeId,
    /// The tests met on it a sibling must answer the same.
    recorded: Vec<(u32, bool)>,
    style: Style,
    matched: Arc<[(u32, selectors::Specificity)]>,
}
/// How many recent styles are kept for sharing.
const SHARABLE: usize = 16;
/// Whether `a` and `b` are alike to selectors and hints: the same name,
/// namespace and attributes (in any order).
fn alike(document: &Document, a: NodeId, b: NodeId) -> std::result::Result<bool, crate::dom::Error> {
    if document.name(a)? != document.name(b)? || document.namespace(a)? != document.namespace(b)? {
        return Ok(false);
    }
    let left: SmallVec<[(Option<crate::dom::AttributeNamespace>, &str, &str); 8]> = document.attributes_ns(a)?.map(|x| (x.namespace, x.local_name, x.value)).collect();
    let mut count = 0usize;
    for x in document.attributes_ns(b)? {
        count += 1;
        if count > left.len() || !left.iter().any(|&(n, l, v)| n == x.namespace && l == x.local_name && v == x.value) {
            return Ok(false);
        }
    }
    Ok(count == left.len())
}
struct Timing {
    shared: usize,
    hints: f64,
    matching: f64,
    computing: f64,
    /// After computing: comparing with the previous tree and storing.
    storing: f64,
    /// Pseudo-elements: looking for and styling them.
    pseudo: f64,
    candidates: usize,
    elements: usize,
    /// Subtrees copied whole: entries and time.
    copied: usize,
    copy: f64,
    /// When the job was made, and when its construction finished.
    started: std::time::Instant,
    constructed: f64,
    visited: usize,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            shared: 0,
            hints: 0.,
            matching: 0.,
            computing: 0.,
            storing: 0.,
            pseudo: 0.,
            candidates: 0,
            elements: 0,
            copied: 0,
            copy: 0.,
            started: std::time::Instant::now(),
            constructed: 0.,
            visited: 0,
        }
    }
}
impl<'a> Job<'a> {
    pub fn new(document: &'a Document, rules: &'a cascade::RuleSet, defaults: Arc<Defaults>, options: JobOptions<'a>, limits: JobLimits, cancelled: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancelled)?;
        Ok(Self {
            document,
            rules,
            defaults,
            options,
            limits,
            cancelled,
            next: Some(document.root()),
            depth: 0,
            root: None,
            work: 0,
            progress: Progress::default(),
            failed: false,
            scratch: Default::default(),
            scopes_of: None,
            sharing: Vec::new(),
            account: Default::default(),
            fonts: font_pool::FontPool::new(limits.font_cache_entries),
            customs: compute::CustomsMemo::default(),
            timing: std::env::var_os("OLO_STYLE_TIMING").map(|_| {
                compute::TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
                for p in compute::PHASES.iter().chain(&compute::COUNTS) {
                    p.store(0, std::sync::atomic::Ordering::Relaxed);
                }
                Timing { started: std::time::Instant::now(), ..Default::default() }
            }),
            ancestors: Default::default(),
            chain: Vec::new(),
            previous: options.previous.and_then(|tree| {
                // Other rules: only when they differ from the tree's in some
                // shadow trees' sheets alone.
                let changed_within = match options.changed_within {
                    _ if tree.generation == options.generation => None,
                    Some((generation, hosts)) if generation == tree.generation => Some(hosts),
                    other => {
                        if crate::env_on!("OLO_STYLE_TIMING") {
                            eprintln!("  incremental off: rules generation {} -> {}, changed within {:?}", tree.generation, options.generation, other.map(|(g, h)| (g, h.len())));
                        }
                        return None;
                    }
                };
                if tree.document != document.id() {
                    return None;
                }
                let changes = document.changes_since(tree.revision)?;
                let mut nodes = crate::dom::NodeSet::default();
                let mut children = crate::dom::NodeSet::default();
                // With :has() in the rules, an element's style can change
                // with its descendants: the ancestors of a change that could
                // take part in a :has() argument (by the keys the arguments
                // test) are restyled too, a chain, not a subtree; :empty
                // makes a parent follow its children.
                let has_keys = rules.has_keys();
                let uses_has = !has_keys.is_empty();
                let uses_empty = rules.uses_empty();
                let invalidation = rules.invalidation();
                let mut style_only = crate::dom::NodeSet::default();
                // The descendants (and later siblings' subtrees) an
                // attribute change can reach through selectors.
                let invalidate = |n: NodeId, keys: Vec<String>, nodes: &mut crate::dom::NodeSet| {
                    let (down, beside) = invalidation.reach_of(keys.iter().cloned());
                    let before = nodes.len();
                    let diagnose = crate::env_on!("OLO_STYLE_TIMING");
                    let mut walk = |root: NodeId, reach: &selectors::Reach, nodes: &mut crate::dom::NodeSet| {
                        if reach.is_empty() {
                            return;
                        }
                        let mut next = document.first_child(root).ok().flatten();
                        let mut depth = 0usize;
                        while let Some(e) = next {
                            if document.node_type(e).ok() == Some(crate::dom::NodeType::Element) && reach.reaches(document, e) {
                                nodes.insert(e);
                            }
                            let child = document.first_child(e).ok().flatten();
                            if child.is_some() {
                                depth += 1;
                                next = child;
                                continue;
                            }
                            let mut at = e;
                            loop {
                                if let Some(sibling) = document.next_sibling(at).ok().flatten() {
                                    next = Some(sibling);
                                    break;
                                }
                                if depth == 0 {
                                    next = None;
                                    break;
                                }
                                depth -= 1;
                                match document.parent(at).ok().flatten() {
                                    Some(p) if p != root => at = p,
                                    _ => {
                                        next = None;
                                        break;
                                    }
                                }
                            }
                        }
                    };
                    walk(n, &down, nodes);
                    if !beside.is_empty() {
                        let mut sibling = document.next_sibling(n).ok().flatten();
                        while let Some(e) = sibling {
                            if document.node_type(e).ok() == Some(crate::dom::NodeType::Element) {
                                if beside.reaches(document, e) {
                                    nodes.insert(e);
                                }
                                walk(e, &beside, nodes);
                            }
                            sibling = document.next_sibling(e).ok().flatten();
                        }
                    }
                    if diagnose && nodes.len() > before {
                        eprintln!(
                            "  invalidation: {:?} on <{}> reaches {} more (down: universal {}, {} tags, {} classes, {} ids, {} attrs; beside: universal {}, {} classes)",
                            keys,
                            document.name(n).ok().flatten().unwrap_or("?"),
                            nodes.len() - before,
                            down.universal,
                            down.tags.len(),
                            down.classes.len(),
                            down.ids.len(),
                            down.attributes.len(),
                            beside.universal,
                            beside.classes.len()
                        );
                    }
                };
                let element_keys = |n: NodeId| -> Vec<String> {
                    let mut keys = Vec::new();
                    if let Ok(Some(name)) = document.name(n) {
                        keys.push(format!("t:{}", name.to_ascii_lowercase()));
                    }
                    if let Ok(attributes) = document.attributes_ns(n) {
                        for a in attributes {
                            keys.push(format!("[{}", a.local_name.to_ascii_lowercase()));
                            if a.local_name == "id" {
                                keys.push(format!("#{}", a.value));
                            } else if a.local_name == "class" {
                                keys.extend(a.value.split_ascii_whitespace().map(|c| format!(".{c}")));
                            }
                        }
                    }
                    keys
                };
                let dirty_ancestors = |n: NodeId, nodes: &mut crate::dom::NodeSet| {
                    let mut at = document.parent(n).ok().flatten();
                    while let Some(a) = at {
                        if !nodes.insert(a) {
                            break;
                        }
                        at = document.parent(a).ok().flatten();
                    }
                };
                let mut geometry_kept = true;
                if crate::env_on!("OLO_STYLE_TIMING") {
                    let name = |n: NodeId| document.name(n).ok().flatten().unwrap_or("?").to_owned();
                    let listed: Vec<String> = changes
                        .iter()
                        .take(12)
                        .map(|(_, c)| match c {
                            crate::dom::Change::Node(n) => format!("node <{}>", name(*n)),
                            crate::dom::Change::Control(n) => format!("<{}>'s value", name(*n)),
                            crate::dom::Change::Attribute { node: n, name: a, old, new } => {
                                format!(
                                    "<{}> {a}: {:?} -> {:?}",
                                    name(*n),
                                    old.as_deref().map(|s| s.chars().take(40).collect::<String>()),
                                    new.as_deref().map(|s| s.chars().take(40).collect::<String>())
                                )
                            }
                            crate::dom::Change::Style(n) => format!("<{}> style", name(*n)),
                            crate::dom::Change::Text { node: n, .. } => format!("text in <{}>", document.parent(*n).ok().flatten().map(name).unwrap_or_default()),
                            crate::dom::Change::Children { node: n, elements } => format!("children of <{}>{}", name(*n), if *elements { " (elements)" } else { "" }),
                            crate::dom::Change::Unknown => "unknown".to_owned(),
                        })
                        .collect();
                    eprintln!("  changes ({}): {}", changes.len(), listed.join("; "));
                }
                for (_, change) in changes {
                    match change {
                        crate::dom::Change::Node(n) => {
                            nodes.insert(*n);
                            if uses_has && has_keys.touches(document, *n) {
                                dirty_ancestors(*n, &mut nodes);
                            }
                            // Which attribute is unknown: every key it carries.
                            let keys = element_keys(*n);
                            invalidate(*n, keys, &mut nodes);
                        }
                        crate::dom::Change::Attribute { node: n, name, old, new } => {
                            geometry_kept &= !document.attribute_changes_list_numbering(*n, name);
                            nodes.insert(*n);
                            // A host's child asking for another slot: its
                            // host's children change their slots.
                            if name.as_ref() == "slot"
                                && let Some(host) = document.parent(*n).ok().flatten().filter(|h| document.shadow_root(*h).is_some())
                            {
                                children.insert(host);
                            }
                            let keys = attribute_keys(name, old.as_deref(), new.as_deref());
                            // An ancestor's :has() can flip only when an
                            // argument tests one of the keys that changed.
                            let flips = has_keys.attribute_flips(name, old.as_deref(), new.as_deref()) || keys.iter().skip(1).any(|k| has_keys.has_key(k));
                            if uses_has && flips {
                                dirty_ancestors(*n, &mut nodes);
                            }
                            invalidate(*n, keys, &mut nodes);
                        }
                        // Inline style: declared values only; :has()
                        // ancestors are touched only when an argument tests
                        // the attribute, descendants when a selector does.
                        // (A keyless :has() argument tests states or structure,
                        // which an inline style cannot change.)
                        // A control's value or checkedness: its own
                        // states (:checked, :placeholder-shown), what it shows.
                        crate::dom::Change::Control(n) => {
                            geometry_kept = false;
                            nodes.insert(*n);
                            if uses_has {
                                dirty_ancestors(*n, &mut nodes);
                            }
                        }
                        crate::dom::Change::Style(n) => {
                            style_only.insert(*n);
                            if uses_has && has_keys.attributes.contains("style") {
                                nodes.insert(*n);
                                dirty_ancestors(*n, &mut nodes);
                            }
                            invalidate(*n, vec!["[style".to_owned()], &mut nodes);
                        }
                        crate::dom::Change::Children { node: n, elements } => {
                            geometry_kept = false;
                            children.insert(*n);
                            // Text alone (a counter's digits replaced) flips an
                            // ancestor's :has() only through :empty.
                            if uses_has && (*elements || has_keys.empty) {
                                nodes.insert(*n);
                                dirty_ancestors(*n, &mut nodes);
                            } else if uses_empty || uses_has {
                                nodes.insert(*n);
                            }
                        }
                        crate::dom::Change::Text { node: n, emptiness } => {
                            geometry_kept = false;
                            // Only text appearing or going flips `:empty`.
                            if uses_empty
                                && *emptiness
                                && let Some(p) = document.parent(*n).ok().flatten()
                            {
                                nodes.insert(p);
                                if has_keys.empty {
                                    dirty_ancestors(p, &mut nodes);
                                }
                            }
                        }
                        crate::dom::Change::Unknown => return None,
                    }
                }
                if !options.resized.is_empty() {
                    geometry_kept = false;
                    let mut stack: Vec<NodeId> = options.resized.to_vec();
                    while let Some(n) = stack.pop() {
                        let Ok(children) = document.children(n) else { continue };
                        for c in children {
                            if document.node_type(c).ok() == Some(crate::dom::NodeType::Element) {
                                nodes.insert(c);
                                stack.push(c);
                            }
                        }
                    }
                }
                if let Some(hosts) = changed_within {
                    for &host in hosts {
                        composed_subtree(document, host, &mut nodes);
                    }
                    // Their recorded rules are the old rules' ids: matched again.
                    for n in std::mem::take(&mut style_only).iter() {
                        nodes.insert(*n);
                    }
                }
                let why_not = (!geometry_kept).then(|| "the document's structure or text changed".to_owned());
                // A host whose children changed: what its slots show did
                // (a child added, gone, or asking for another slot), so each
                // slot is walked, not copied with what it showed before
                // (Reddit's <alert-controller>, added to <shreddit-app>,
                // was missing from the styles).
                let mut slots = Vec::new();
                for &host in &children {
                    if let Some(root) = document.shadow_root(host) {
                        let mut stack = vec![root];
                        while let Some(n) = stack.pop() {
                            if document.name(n).ok().flatten() == Some("slot") {
                                slots.push(n);
                            }
                            if let Ok(c) = document.children(n) {
                                stack.extend(c);
                            }
                        }
                    }
                }
                let mut below = crate::dom::NodeSet::default();
                for &n in nodes.iter().chain(&children).chain(&style_only).chain(&slots) {
                    let mut at = Some(n);
                    while let Some(a) = at {
                        if !below.insert(a) {
                            break;
                        }
                        // Up the composed tree, as the walk goes: a change in
                        // a shadow tree (a code editor's lines, MDN's) is
                        // below its host and the host's ancestors, else
                        // their subtree was copied whole, its new elements
                        // unstyled (the layout failed, Stale).
                        at = document.composed_parent(a).ok().flatten().or_else(|| document.shadow_host(a));
                    }
                }
                Some(Incremental { tree, nodes, style_only, children, below, geometry_kept, why_not, rules_changed: changed_within.is_some() })
            }),
            same: Vec::new(),
            inherited_same: Vec::new(),
            open: Vec::new(),
            tree: StyleTree {
                document: document.id(),
                revision: document.revision(),
                generation: options.generation,
                slots: Vec::new(),
                styles: crate::chunked::Chunked::new(),
                pseudos: Default::default(),
                geometry_kept: false,
                animated: Vec::new(),
                backdrops: Vec::new(),
                report: Report { stylesheet_issues: rules.issues().len(), ..Default::default() },
            },
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    /// Worker scratch only. The pool drops at finish; published Font objects
    /// remain owned by the completed styles and are included in Memory.
    pub fn font_cache_stats(&self) -> FontCacheStats {
        self.fonts.stats()
    }
    pub fn completed(&self) -> Result<impl Iterator<Item = (NodeId, &Style)>> {
        if self.failed {
            return Err(Error::Failed);
        }
        Ok(self.tree.iter())
    }
    /// Hosts are element-bound snapshots from the same Defaults revision.
    /// No host is retained after a node; node/tree-dependent factors cannot
    /// accidentally use a previous subject's context on a later step.
    pub fn step<H: Host>(&mut self, nodes: usize, mut host: impl FnMut(NodeId) -> H) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.step_inner(nodes, &mut host);
        if result.is_err() {
            self.failed = true;
            self.next = None;
        }
        result
    }
    /// The document as selectors see it, with `node`'s scopes worked out
    /// once for all its matches (its own, its pseudo-elements').
    fn dom(&mut self, node: NodeId) -> Result<selectors::DomTree<'a>> {
        let scopes = match self.scopes_of {
            Some((n, scopes)) if n == node => scopes,
            _ => {
                let scopes = selectors::DomTree::find_scopes(self.document, node).map_err(|e| Error::cascade(cascade::Error::Selectors(e)))?;
                self.scopes_of = Some((node, scopes));
                scopes
            }
        };
        let dom = if let Some(states) = self.options.states { selectors::DomTree::with_states(self.document, states) } else { selectors::DomTree::new(self.document) };
        Ok(dom.with_scopes(node, scopes))
    }
    fn tick(&mut self) -> Result<()> {
        check(self.cancelled)?;
        self.work = self.work.saturating_add(1);
        if self.work > self.limits.work {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn step_inner<H: Host>(&mut self, nodes: usize, host: &mut impl FnMut(NodeId) -> H) -> Result<Progress> {
        check(self.cancelled)?;
        if let Some(t) = self.timing.as_mut()
            && t.constructed == 0.
        {
            t.constructed = t.started.elapsed().as_secs_f64();
        }
        for _ in 0..nodes {
            let Some(node) = self.next else {
                self.progress.finished = true;
                break;
            };
            self.tick()?;
            if self.progress.visited >= self.limits.nodes {
                return Err(Error::Limit);
            }
            self.progress.visited += 1;
            if let Some(t) = self.timing.as_mut() {
                t.visited += 1;
            }
            if self.document.node_type(node).map_err(Error::Dom)? == NodeType::Element {
                // The filter holds the ancestors: pop to this node's parent,
                // style it, then push it for its children.
                let parent = self.document.composed_parent(node).map_err(Error::Dom)?;
                while self.chain.last().is_some_and(|(n, _)| Some(*n) != parent) {
                    let (_, hashes) = self.chain.pop().unwrap();
                    for h in hashes {
                        self.ancestors.remove(h);
                    }
                }
                while self.open.last().is_some_and(|(n, _)| Some(*n) != parent) {
                    let (_, i) = self.open.pop().unwrap();
                    self.tree.styles[i].end = self.tree.styles.len() as u32;
                }
                let (index, skipped) = self.element(node, &host(node))?;
                if skipped {
                    // The subtree came over whole: on to what follows it.
                    self.tree.styles[index].end = self.tree.styles.len() as u32;
                    self.next = self.skip_subtree(node)?;
                    continue;
                }
                self.open.push((node, index));
                if !self.options.quirks {
                    let mut hashes = SmallVec::new();
                    let name = self.document.name(node).map_err(Error::Dom)?.unwrap_or("");
                    let id = self.document.attribute(node, "id").map_err(Error::Dom)?;
                    let classes = self.document.attribute(node, "class").map_err(Error::Dom)?.unwrap_or("");
                    selectors::AncestorFilter::element_hashes(name, id, classes, &mut hashes);
                    for &h in &hashes {
                        self.ancestors.insert(h);
                    }
                    self.chain.push((node, hashes));
                } else {
                    self.chain.push((node, SmallVec::new()));
                }
            }
            self.next = self.successor(node)?;
        }
        self.progress.finished = self.next.is_none();
        Ok(self.progress)
    }
    /// What follows `node` with its descendants left out.
    fn skip_subtree(&mut self, mut node: NodeId) -> Result<Option<NodeId>> {
        while node != self.document.root() {
            self.tick()?;
            if let Some(sibling) = self.document.composed_next_sibling(node).map_err(Error::Dom)? {
                return Ok(Some(sibling));
            }
            node = self.document.composed_parent(node).map_err(Error::Dom)?.ok_or(Error::Invalid)?;
            self.depth = self.depth.checked_sub(1).ok_or(Error::Invalid)?;
        }
        Ok(None)
    }
    /// The next node of the composed tree (shadow trees in their hosts'
    /// places), in tree order.
    fn successor(&mut self, mut node: NodeId) -> Result<Option<NodeId>> {
        if let Some(child) = self.document.composed_first_child(node).map_err(Error::Dom)? {
            if self.depth >= self.limits.depth {
                return Err(Error::Limit);
            }
            self.depth += 1;
            return Ok(Some(child));
        }
        while node != self.document.root() {
            self.tick()?;
            if let Some(sibling) = self.document.composed_next_sibling(node).map_err(Error::Dom)? {
                return Ok(Some(sibling));
            }
            node = self.document.composed_parent(node).map_err(Error::Dom)?.ok_or(Error::Invalid)?;
            self.depth = self.depth.checked_sub(1).ok_or(Error::Invalid)?;
        }
        Ok(None)
    }
    /// HTML's presentational hints (HTML §15.3): attributes that map to
    /// declarations below author styles. Dimensions follow the rules for
    /// parsing dimension values; colours are parsed as CSS colours.
    fn html_hints(&mut self, node: NodeId, hints: &mut Vec<p::Declaration>) -> Result<()> {
        let document = self.document;
        let name = document.name(node).map_err(Error::Dom)?.unwrap_or("");
        let push = |this: &mut Self, property: &str, value: String, hints: &mut Vec<p::Declaration>| -> Result<()> {
            if value.len() > this.limits.inline_bytes || hints.len() >= this.limits.inline_declarations {
                return Err(Error::Limit);
            }
            let syntax = Syntax::parse(Arc::from(value), this.limits.css, this.cancelled).map_err(Error::css)?;
            match p::Declaration::parse(property, syntax.components(), false, this.limits.property, this.cancelled) {
                Ok(declaration) => hints.push(declaration),
                Err(p::Error::Invalid) => this.tree.report.presentation_invalid += 1,
                Err(p::Error::Unsupported(_) | p::Error::UnknownProperty) => this.tree.report.presentation_unsupported += 1,
                Err(error) => return Err(Error::Property(error)),
            }
            Ok(())
        };
        let attribute = |name: &str| document.attribute(node, name).map_err(Error::Dom);
        let sized = matches!(name, "img" | "table" | "td" | "th" | "tr" | "col" | "colgroup" | "iframe" | "video" | "canvas" | "embed" | "object" | "hr" | "input");
        if sized {
            for dimension in ["width", "height"] {
                if name == "input" && dimension == "height" {
                    continue;
                }
                if let Some(value) = attribute(dimension)? {
                    if value.len() > self.limits.inline_bytes {
                        return Err(Error::Limit);
                    }
                    if let Some(value) = dimension_hint(value) {
                        push(self, dimension, value, hints)?;
                    }
                }
            }
        }
        // An iframe's `frameborder="0"` (or "no"): no border (HTML §15.4.4).
        if name == "iframe"
            && let Some(value) = attribute("frameborder")?
            && (value.trim() == "0" || value.trim().eq_ignore_ascii_case("no"))
        {
            push(self, "border-style", "none".to_owned(), hints)?;
        }
        if matches!(name, "body" | "table" | "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "div" | "marquee" | "col" | "colgroup")
            && let Some(value) = attribute("bgcolor")?
        {
            push(self, "background-color", legacy_color(value), hints)?;
        }
        if name == "body"
            && let Some(value) = attribute("text")?
        {
            push(self, "color", legacy_color(value), hints)?;
        }
        if name == "font"
            && let Some(value) = attribute("color")?
        {
            push(self, "color", legacy_color(value), hints)?;
        }
        if name == "font"
            && let Some(value) = attribute("size")?
        {
            // <font size>: 1–7 absolute, +n/−n relative to 3 (HTML §15.3.9).
            let v = value.trim();
            let n: Option<i32> =
                v.strip_prefix('+').map_or_else(|| v.strip_prefix('-').and_then(|d| d.parse::<i32>().ok().map(|d| 3 - d)), |d| d.parse::<i32>().ok().map(|d| 3 + d)).or_else(|| v.parse().ok());
            if let Some(n) = n {
                let keyword = match n.clamp(1, 7) {
                    1 => "x-small",
                    2 => "small",
                    3 => "medium",
                    4 => "large",
                    5 => "x-large",
                    6 => "xx-large",
                    _ => "xxx-large",
                };
                push(self, "font-size", keyword.into(), hints)?;
            }
        }
        if matches!(name, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "caption" | "legend")
            && let Some(value) = attribute("align")?
        {
            let value = value.trim();
            let cell = matches!(name, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot");
            let align = if value.eq_ignore_ascii_case("center") || value.eq_ignore_ascii_case("middle") {
                Some(if cell { "center" } else { "-webkit-center" })
            } else if value.eq_ignore_ascii_case("left") {
                Some(if cell { "left" } else { "-webkit-left" })
            } else if value.eq_ignore_ascii_case("right") {
                Some(if cell { "right" } else { "-webkit-right" })
            } else if value.eq_ignore_ascii_case("justify") {
                Some("justify")
            } else {
                None
            };
            if let Some(align) = align {
                push(self, "text-align", align.into(), hints)?;
            }
        }
        if matches!(name, "table") {
            if let Some(value) = attribute("align")?
                && (value.trim().eq_ignore_ascii_case("center") || value.trim().eq_ignore_ascii_case("left") || value.trim().eq_ignore_ascii_case("right"))
            {
                let v = value.trim().to_ascii_lowercase();
                if v == "center" {
                    push(self, "margin-left", "auto".into(), hints)?;
                    push(self, "margin-right", "auto".into(), hints)?;
                }
            }
            if let Some(value) = attribute("cellspacing")?
                && let Some(v) = dimension_hint(value)
            {
                push(self, "border-spacing", v, hints)?;
            }
        }
        if matches!(name, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot")
            && let Some(value) = attribute("valign")?
        {
            let v = value.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "top" | "middle" | "bottom" | "baseline") {
                push(self, "vertical-align", v, hints)?;
            }
        }
        if matches!(name, "td" | "th") {
            if attribute("nowrap")?.is_some() {
                push(self, "white-space", "nowrap".into(), hints)?;
            }
            // cellpadding on the enclosing table pads every cell.
            let mut ancestor = document.parent(node).map_err(Error::Dom)?;
            let mut depth = 0;
            while let Some(a) = ancestor {
                depth += 1;
                if depth > 64 {
                    break;
                }
                if document.name(a).map_err(Error::Dom)? == Some("table") {
                    if let Some(value) = document.attribute(a, "cellpadding").map_err(Error::Dom)?
                        && let Some(v) = dimension_hint(value)
                    {
                        push(self, "padding", v, hints)?;
                    }
                    break;
                }
                ancestor = document.parent(a).map_err(Error::Dom)?;
            }
        }
        if name == "hr" {
            if let Some(value) = attribute("size")?
                && let Some(v) = dimension_hint(value)
            {
                push(self, "height", v, hints)?;
            }
            if attribute("noshade")?.is_some() {
                push(self, "border-style", "solid".into(), hints)?;
            }
        }
        Ok(())
    }
    fn svg_hints(&mut self, node: NodeId, hints: &mut Vec<p::Declaration>) -> Result<()> {
        let document = self.document;
        let geometry = matches!(document.name(node).map_err(Error::Dom)?, Some("svg" | "rect" | "image" | "foreignObject" | "symbol" | "use"));
        let position = matches!(document.name(node).map_err(Error::Dom)?, Some("svg" | "rect" | "image" | "foreignObject" | "use"));
        let rounded = document.name(node).map_err(Error::Dom)? == Some("rect");
        let animation = matches!(document.name(node).map_err(Error::Dom)?, Some("animate" | "animateMotion" | "animateTransform" | "set"));
        for (name, source) in document.attributes(node).map_err(Error::Dom)? {
            self.tick()?;
            if !(geometry && matches!(name, "width" | "height") || position && matches!(name, "x" | "y") || rounded && matches!(name, "rx" | "ry"))
                && !matches!(
                    name,
                    "fill"
                        | "fill-opacity"
                        | "fill-rule"
                        | "stroke"
                        | "stroke-opacity"
                        | "clip-rule"
                        | "clip-path"
                        | "color"
                        | "opacity"
                        | "display"
                        | "visibility"
                        | "stop-color"
                        | "stop-opacity"
                        | "flood-color"
                        | "flood-opacity"
                        | "font-size"
                        | "font-family"
                        | "font-weight"
                        | "font-style"
                        | "letter-spacing"
                )
                || name == "fill" && animation
            {
                continue;
            }
            if source.len() > self.limits.inline_bytes || hints.len() >= self.limits.inline_declarations {
                return Err(Error::Limit);
            }
            let mut syntax = Syntax::parse(Arc::from(source), self.limits.css, self.cancelled).map_err(Error::css)?;
            if geometry && matches!(name, "width" | "height") || position && matches!(name, "x" | "y") || rounded && matches!(name, "rx" | "ry") || matches!(name, "font-size" | "letter-spacing") {
                let mut parts = syntax.components().filter(|c| c.kind() != crate::css::Kind::Whitespace);
                if parts.next().is_some_and(|c| matches!(c.kind(), crate::css::Kind::Number | crate::css::Kind::Integer)) && parts.next().is_none() {
                    syntax = Syntax::parse(Arc::from(format!("{}px", source.trim())), self.limits.css, self.cancelled).map_err(Error::css)?;
                }
            }
            match p::Declaration::parse(name, syntax.components(), false, self.limits.property, self.cancelled) {
                Ok(declaration) => hints.push(declaration),
                Err(p::Error::Invalid) => self.tree.report.presentation_invalid += 1,
                Err(p::Error::Unsupported(_)) => self.tree.report.presentation_unsupported += 1,
                Err(error) => return Err(Error::Property(error)),
            }
        }
        Ok(())
    }
    fn element(&mut self, node: NodeId, host: &impl Host) -> Result<(usize, bool)> {
        if self.tree.styles.len() >= self.limits.elements || self.tree.styles.len() >= u32::MAX as usize || node.slot() as usize >= self.limits.slots {
            return Err(Error::Limit);
        }
        let parent_node = self.document.composed_parent(node).map_err(Error::Dom)?;
        let parent = parent_node.and_then(|n| self.tree.index(n));
        let parent_box = parent
            .and_then(|i| if matches!(self.tree.styles[i].style.value(Property::Display), Value::Display(Display::Contents)) { self.tree.styles[i].parent_box } else { NonZeroU32::new(i as u32 + 1) });
        // Incremental: the previous style stands when this element's
        // attributes and its siblings are unchanged and its parent's style
        // is the same as before (or it is the root).
        if let Some(inc) = &self.previous
            && !inc.nodes.contains(&node)
            && !inc.style_only.contains(&node)
            && !parent_node.is_some_and(|p| inc.children.contains(&p))
            && parent.is_none_or(|i| self.inherited_same[i])
            // Through `display: contents`, the box its children are laid out in.
            && parent_box.is_none_or(|b| self.inherited_same[b.get() as usize - 1])
            && let Some(i) = inc.tree.index(node)
        {
            let style = inc.tree.styles[i].style.clone();
            let matched = if inc.rules_changed { None } else { inc.tree.styles[i].matched.clone() };
            let index = self.tree.styles.len();
            self.tree.styles.push(Styled { node, style, parent_box, end: 0, pseudo: None, matched });
            self.same.push(true);
            self.inherited_same.push(true);
            if self.root.is_none() {
                self.root = Some(index);
            }
            if self.tree.slots.len() <= node.slot() as usize {
                self.tree.slots.resize(node.slot() as usize + 1, None);
            }
            self.tree.slots[node.slot() as usize] = NonZeroU32::new(index as u32 + 1);
            self.account.add(&self.tree.styles[index].style);
            let mut pseudos = 0usize;
            for pseudo in [PseudoElement::Before, PseudoElement::After, PseudoElement::Placeholder, PseudoElement::Marker] {
                if let Some(&p) = inc.tree.pseudos.get(&(node, pseudo)) {
                    let style = inc.tree.styles[p as usize].style.clone();
                    let at = self.tree.styles.len() as u32;
                    self.account.add(&style);
                    self.tree.styles.push(Styled { node, style, parent_box: NonZeroU32::new(index as u32 + 1), end: at + 1, pseudo: Some(pseudo), matched: None });
                    self.same.push(true);
                    self.inherited_same.push(true);
                    self.tree.pseudos.insert((node, pseudo), at);
                    pseudos += 1;
                }
            }
            self.progress.styled += 1;
            self.progress.reused += 1;
            // Nothing changed below either: the whole subtree comes over,
            // entry by entry (indices shift by the same amount), and the
            // walk goes on past it.
            if !inc.below.contains(&node) {
                let old_end = inc.tree.styles[i].end as usize;
                let first = i + 1 + pseudos;
                if old_end > first && self.tree.styles.len() + (old_end - first) <= self.limits.elements {
                    let copy_clock = self.timing.as_ref().map(|_| std::time::Instant::now());
                    let delta = index as i64 - i as i64;
                    for j in first..old_end {
                        check(self.cancelled)?;
                        self.work = self.work.saturating_add(1);
                        if self.work > self.limits.work {
                            return Err(Error::Limit);
                        }
                        let old = &inc.tree.styles[j];
                        let k = self.tree.styles.len();
                        // Shared with the previous tree: its memory is accounted there.
                        self.tree.styles.push(Styled {
                            node: old.node,
                            style: old.style.clone(),
                            parent_box: old.parent_box.and_then(|p| NonZeroU32::new((i64::from(p.get()) + delta) as u32)),
                            end: (i64::from(old.end) + delta) as u32,
                            pseudo: old.pseudo,
                            matched: if inc.rules_changed { None } else { old.matched.clone() },
                        });
                        self.same.push(true);
                        self.inherited_same.push(true);
                        match old.pseudo {
                            Some(p) => {
                                self.tree.pseudos.insert((old.node, p), k as u32);
                            }
                            None => {
                                if self.tree.slots.len() <= old.node.slot() as usize {
                                    self.tree.slots.resize(old.node.slot() as usize + 1, None);
                                }
                                self.tree.slots[old.node.slot() as usize] = NonZeroU32::new(k as u32 + 1);
                                self.progress.styled += 1;
                                self.progress.reused += 1;
                            }
                        }
                    }
                    if let (Some(t), Some(c)) = (self.timing.as_mut(), copy_clock) {
                        t.copy += c.elapsed().as_secs_f64();
                        t.copied += old_end - first;
                    }
                    return Ok((index, true));
                }
            }
            return Ok((index, false));
        }
        let clock = self.timing.as_ref().map(|_| std::time::Instant::now());
        let previous_matched = self.previous.as_ref().and_then(|inc| {
            if !inc.style_only.contains(&node) || inc.nodes.contains(&node) || parent_node.is_some_and(|p| inc.children.contains(&p)) {
                return None;
            }
            inc.tree.index(node).and_then(|i| inc.tree.styles[i].matched.clone())
        });
        let (style, matched) = self.compute_element(node, parent, parent_box, host, previous_matched)?;
        let mark_compute = clock.map(|c| c.elapsed().as_secs_f64()).unwrap_or(0.);
        let index = self.tree.styles.len();
        self.tree.report.value_fallbacks += usize::from(style.dependencies().fallbacks);
        // Computed afresh: the same as before only if every value is; a
        // difference beyond paint means geometry may have changed.
        let mut style = style;
        let same = self.previous.as_ref().and_then(|inc| inc.tree.index(node)).is_some_and(|i| self.previous.as_ref().unwrap().tree.styles[i].style.same_as(&style));
        // Query selection depends on ancestors' names and containment, even
        // when their dimensions and inherited properties have not changed.
        // Rematch the whole affected subtree, including nested containers
        // that can still query this ancestor by name.
        if self.rules.has_containers()
            && let Some(inc) = self.previous.as_mut()
            && let Some(i) = inc.tree.index(node)
            && [Property::ContainerName, Property::ContainerType].iter().any(|&p| !inc.tree.styles[i].style.value(p).same(style.value(p)))
        {
            composed_subtree(self.document, node, &mut inc.nodes);
        }
        if let Some(inc) = self.previous.as_ref()
            && let Some(i) = inc.tree.index(node)
        {
            // The same as before: the previous style itself (its identity
            // tells painted tiles nothing changed). Different (custom
            // properties, say) but painting the same: the previous values.
            if same {
                style = inc.tree.styles[i].style.clone();
            } else {
                style.share_with(&inc.tree.styles[i].style);
            }
            if !same && crate::env_on!("OLO_STYLE_DIFF") {
                eprintln!(
                    "style differs: <{} class={:?}> {}",
                    self.document.name(node).map_err(Error::Dom)?.unwrap_or("?"),
                    self.document.attribute(node, "class").map_err(Error::Dom)?.unwrap_or("").chars().take(50).collect::<String>(),
                    inc.tree.styles[i].style.difference(&style)
                );
            }
        }
        if let Some(inc) = self.previous.as_mut()
            && inc.geometry_kept
            && !same
        {
            inc.geometry_kept = inc.tree.index(node).is_some_and(|i| inc.tree.styles[i].style.differs_only_in_paint(&style));
            if !inc.geometry_kept {
                inc.why_not = Some(match inc.tree.index(node) {
                    Some(i) => format!("<{}> changed {}", self.document.name(node).map_err(Error::Dom)?.unwrap_or("?"), inc.tree.styles[i].style.difference(&style)),
                    None => format!("<{}> is new", self.document.name(node).map_err(Error::Dom)?.unwrap_or("?")),
                });
            }
        }
        self.same.push(same);
        // Children keep their styles when what they inherit is unchanged;
        // the descendants whose selectors an attribute change can reach
        // are in `nodes` already (the invalidation sets), so this holds
        // for attribute changes too.
        self.inherited_same.push(same || self.previous.as_ref().and_then(|inc| inc.tree.index(node)).is_some_and(|i| self.previous.as_ref().unwrap().tree.styles[i].style.inherited_same(&style)));
        self.tree.styles.push(Styled { node, style, parent_box, end: 0, pseudo: None, matched: Some(matched) });
        if self.root.is_none() {
            self.root = Some(index);
        }
        if self.tree.slots.len() <= node.slot() as usize {
            self.tree.slots.resize(node.slot() as usize + 1, None);
        }
        self.tree.slots[node.slot() as usize] = NonZeroU32::new(index as u32 + 1);
        self.account.add(&self.tree.styles[index].style);
        let mark_stored = clock.map(|c| c.elapsed().as_secs_f64()).unwrap_or(0.);
        if let Some(t) = self.timing.as_mut() {
            t.storing += mark_stored - mark_compute;
        }
        if self.rules.has_pseudo_elements() {
            let element = self.tree.styles[index].style.clone();
            for (pseudo, style) in self.compute_pseudos(node, &element, host)? {
                if self.tree.styles.len() >= self.limits.elements || self.tree.styles.len() >= u32::MAX as usize {
                    return Err(Error::Limit);
                }
                self.tree.report.value_fallbacks += usize::from(style.dependencies().fallbacks);
                self.account.add(&style);
                let at = self.tree.styles.len() as u32;
                // A generated pseudo-element computed afresh: geometry stands
                // only if it was there before and differs in paint alone.
                if let Some(inc) = self.previous.as_mut()
                    && inc.geometry_kept
                {
                    inc.geometry_kept = inc.tree.pseudos.get(&(node, pseudo)).is_some_and(|&p| inc.tree.styles[p as usize].style.differs_only_in_paint(&style));
                }
                self.tree.styles.push(Styled { node, style, parent_box: NonZeroU32::new(index as u32 + 1), end: at + 1, pseudo: Some(pseudo), matched: None });
                self.same.push(false);
                self.inherited_same.push(false);
                self.tree.pseudos.insert((node, pseudo), at);
            }
        }
        if let (Some(t), Some(c)) = (self.timing.as_mut(), clock) {
            t.pseudo += c.elapsed().as_secs_f64() - mark_stored;
        }
        let retained = self
            .account
            .memory
            .accounted_reserved_bytes
            .saturating_add(self.tree.storage_payload_bytes())
            .saturating_sub(self.tree.styles.len() * size_of::<Style>())
            .saturating_add(self.account.scratch_bytes());
        let retained = retained.saturating_add(self.fonts.stats().reserved_bytes);
        if retained > self.limits.retained_bytes {
            return Err(Error::Limit);
        }
        self.progress.styled += 1;
        Ok((index, false))
    }
    /// The previous styles patched in place for a few elements whose own
    /// declarations changed and nothing selectors see elsewhere (an inline
    /// style set by a script, an attribute no selector reaches from): each
    /// is cascaded from the rules it matched (matched again when
    /// `rematch`) and computed anew, its pseudo-elements with it. The
    /// whole tree is never visited or copied. None when the patch cannot
    /// stand (a value children inherit changed, a pseudo-element came or
    /// went, an element without its rules recorded): the tree is then
    /// untouched, and a full job over it is the way.
    pub fn patch<H: Host>(
        document: &'a Document,
        rules: &'a cascade::RuleSet,
        defaults: Arc<Defaults>,
        options: JobOptions<'a>,
        limits: JobLimits,
        cancelled: Option<&'a AtomicBool>,
        tree: &mut StyleTree,
        nodes: &[(NodeId, bool)],
        mut host: impl FnMut(NodeId) -> H,
    ) -> Result<Option<Patched>> {
        if tree.document != document.id() || tree.generation != options.generation || tree.styles.is_empty() || tree.styles[0].pseudo.is_some() {
            return Ok(None);
        }
        let mut job = Self::new(document, rules, defaults, JobOptions { previous: None, ..options }, limits, cancelled)?;
        std::mem::swap(&mut job.tree, tree);
        job.root = Some(0);
        let result = job.patch_inner(nodes, &mut host);
        std::mem::swap(&mut job.tree, tree);
        result
    }
    /// The ancestor filter made of `node`'s ancestors (outside quirks mode,
    /// where the walk keeps none).
    fn fill_ancestors(&mut self, node: NodeId) -> Result<()> {
        self.ancestors = Default::default();
        if self.options.quirks {
            return Ok(());
        }
        let mut hashes: SmallVec<[u64; 8]> = SmallVec::new();
        let mut at = self.document.parent(node).map_err(Error::Dom)?;
        while let Some(a) = at {
            if self.document.node_type(a).map_err(Error::Dom)? == NodeType::Element {
                hashes.clear();
                let name = self.document.name(a).map_err(Error::Dom)?.unwrap_or("");
                let id = self.document.attribute(a, "id").map_err(Error::Dom)?;
                let classes = self.document.attribute(a, "class").map_err(Error::Dom)?.unwrap_or("");
                selectors::AncestorFilter::element_hashes(name, id, classes, &mut hashes);
                for &h in &hashes {
                    self.ancestors.insert(h);
                }
            }
            at = self.document.parent(a).map_err(Error::Dom)?;
        }
        Ok(())
    }
    fn patch_inner<H: Host>(&mut self, nodes: &[(NodeId, bool)], host: &mut impl FnMut(NodeId) -> H) -> Result<Option<Patched>> {
        let mut undo: Vec<(usize, Style, Option<Arc<[(u32, selectors::Specificity)]>>)> = Vec::new();
        let result = self.patch_in_place(nodes, host, &mut undo);
        match result {
            Ok(Ok(patched)) => {
                self.tree.revision = self.document.revision();
                self.tree.geometry_kept = patched.geometry_kept;
                // The animated entries, for those written over.
                for &(i, _, _) in &undo {
                    let element = self.tree.styles[i].pseudo.is_none();
                    let (animates, backdrop) = (element && names_animation(&self.tree.styles[i].style), element && has_backdrop(&self.tree.styles[i].style));
                    keep(&mut self.tree.animated, i as u32, animates);
                    keep(&mut self.tree.backdrops, i as u32, backdrop);
                }
                if let Some(t) = &self.timing {
                    eprintln!(
                        "style patch: {} of {} elements changed ({} written) in {:.2} ms; geometry {}",
                        patched.changed,
                        nodes.len(),
                        undo.len(),
                        t.started.elapsed().as_secs_f64() * 1000.,
                        if patched.geometry_kept { "kept" } else { "changed" }
                    );
                }
                Ok(Some(patched))
            }
            other => {
                // As it was.
                for (i, style, matched) in undo.into_iter().rev() {
                    self.tree.styles[i].style = style;
                    self.tree.styles[i].matched = matched;
                }
                match other {
                    Ok(Err(why)) => {
                        if self.timing.is_some() {
                            eprintln!("style patch: not for {} elements: {why}", nodes.len());
                        }
                        Ok(None)
                    }
                    Err(e) => Err(e),
                    Ok(Ok(_)) => unreachable!(),
                }
            }
        }
    }
    /// The elements computed anew and written over, each with its
    /// pseudo-elements, and the descendants of one that changed what they
    /// inherit (a custom property, a colour) likewise, from the rules
    /// they matched; `undo` holds what was overwritten. Err says why the
    /// patch cannot stand.
    fn patch_in_place<H: Host>(
        &mut self,
        nodes: &[(NodeId, bool)],
        host: &mut impl FnMut(NodeId) -> H,
        undo: &mut Vec<(usize, Style, Option<Arc<[(u32, selectors::Specificity)]>>)>,
    ) -> Result<std::result::Result<Patched, String>> {
        let name = |job: &Self, n: NodeId| job.document.name(n).ok().flatten().unwrap_or("?").to_owned();
        let mut geometry_kept = true;
        let mut changed = 0usize;
        let mut changed_nodes = Vec::new();
        let mut boxes_kept = true;
        let mut pseudos_changed = 0usize;
        let mut done = std::collections::HashSet::new();
        // The elements asked for, then the subtrees of those that changed
        // what they inherit.
        let mut queue: Vec<(usize, bool)> = Vec::new();
        let mut asked = std::collections::HashMap::new();
        for &(node, rematch) in nodes {
            let Some(i) = self.tree.index(node) else { return Ok(Err(format!("<{}> has no style", name(self, node)))) };
            if self.tree.styles[i].pseudo.is_some() {
                return Ok(Err(format!("<{}> is a pseudo-element's", name(self, node))));
            }
            queue.push((i, rematch));
            *asked.entry(i).or_insert(false) |= rematch;
        }
        queue.reverse();
        while let Some((i, rematch)) = queue.pop() {
            self.tick()?;
            if !done.insert(i) {
                continue;
            }
            let node = self.tree.styles[i].node;
            let parent_node = self.document.composed_parent(node).map_err(Error::Dom)?;
            let parent = parent_node.and_then(|n| self.tree.index(n));
            if parent_node.is_some_and(|p| self.document.node_type(p).map_err(Error::Dom).is_ok_and(|t| t == NodeType::Element)) && parent.is_none() {
                return Ok(Err(format!("<{}>'s parent has no style", name(self, node))));
            }
            let parent_box = parent.and_then(|i| {
                if matches!(self.tree.styles[i].style.value(Property::Display), Value::Display(Display::Contents)) { self.tree.styles[i].parent_box } else { NonZeroU32::new(i as u32 + 1) }
            });
            let previous_matched = if rematch { None } else { self.tree.styles[i].matched.clone() };
            if !rematch && previous_matched.is_none() {
                return Ok(Err(format!("<{}> has no rules recorded", name(self, node))));
            }
            let h = host(node);
            // The ancestor filter holds this element's ancestors, as the
            // walk would have it: its selectors (and its pseudo-elements')
            // with a descendant part are judged by it.
            self.fill_ancestors(node)?;
            let (mut style, matched) = self.compute_element(node, parent, parent_box, &h, previous_matched)?;
            let old = &self.tree.styles[i].style;
            if self.rules.has_containers() && [Property::ContainerName, Property::ContainerType].iter().any(|&p| !old.value(p).same(style.value(p))) {
                // Recorded matches below this element are no longer valid;
                // the incremental walk will rematch its composed subtree.
                return Ok(Err(format!("<{}>'s query container changed", name(self, node))));
            }
            let same = old.same_as(&style);
            if same && !rematch {
                continue;
            }
            let mut below = false;
            if !same {
                style.share_with(old);
                below = !old.inherited_same(&style);
                if geometry_kept && !old.differs_only_in_paint(&style) {
                    geometry_kept = false;
                }
                if boxes_kept && !old.keeps_boxes(&style) {
                    boxes_kept = false;
                }
                changed += 1;
                changed_nodes.push(node);
            }
            let pseudos = self.compute_pseudos(node, &style, &h)?;
            // The pseudo-elements must be the ones there before.
            for pseudo in [PseudoElement::Before, PseudoElement::After, PseudoElement::Placeholder, PseudoElement::Marker] {
                if self.tree.pseudos.contains_key(&(node, pseudo)) != pseudos.iter().any(|(p, _)| *p == pseudo) {
                    return Ok(Err(format!("<{}>'s {pseudo:?} came or went", name(self, node))));
                }
            }
            if !same {
                self.tree.report.value_fallbacks += usize::from(style.dependencies().fallbacks);
                let entry = &mut self.tree.styles[i];
                undo.push((i, std::mem::replace(&mut entry.style, style), std::mem::replace(&mut entry.matched, Some(matched))));
            }
            for (p, s) in pseudos {
                let at = self.tree.pseudos[&(node, p)] as usize;
                let entry = &mut self.tree.styles[at];
                if entry.style.same_as(&s) {
                    continue;
                }
                if geometry_kept && !entry.style.differs_only_in_paint(&s) {
                    geometry_kept = false;
                }
                if boxes_kept && !entry.style.keeps_boxes(&s) {
                    boxes_kept = false;
                }
                self.tree.report.value_fallbacks += usize::from(s.dependencies().fallbacks);
                undo.push((at, std::mem::replace(&mut entry.style, s), entry.matched.clone()));
                pseudos_changed += 1;
                // Its host stands for it among the changed: its boxes and
                // texts are within the host's.
                if same && changed_nodes.last() != Some(&node) {
                    changed_nodes.push(node);
                }
            }
            if below {
                // Its descendants, parents before children (the tree's
                // order), from their recorded rules.
                let end = self.tree.styles[i].end as usize;
                let mut descendants = Vec::new();
                for j in (i + 1..end).rev() {
                    if self.tree.styles[j].pseudo.is_none() && !done.contains(&j) {
                        descendants.push((j, asked.get(&j).copied().unwrap_or(false)));
                    }
                }
                queue.extend(descendants);
            }
        }
        Ok(Ok(Patched { geometry_kept, changed, nodes: changed_nodes, pseudos: pseudos_changed, boxes_kept }))
    }
    /// An element's style computed: its hints and inline declarations, its
    /// rules (the ones recorded before cascaded again when only its inline
    /// style changed, else matched), and the computed values.
    /// The query containers around `node`, nearest first, for its
    /// `@container` rules: its ancestors with a `container-type`.
    fn containers_around(&mut self, node: NodeId) -> Result<()> {
        self.scratch.containers.clear();
        if !self.rules.has_containers() {
            return Ok(());
        }
        let mut at = self.document.parent(node).map_err(Error::Dom)?;
        let mut depth = 0;
        while let Some(n) = at {
            depth += 1;
            if depth > self.limits.depth {
                return Err(Error::Limit);
            }
            if let Some(i) = self.tree.index(n) {
                let style = &self.tree.styles[i].style;
                if let Value::Keyword(kind @ (Keyword::InlineSize | Keyword::Size)) = style.value(Property::ContainerType) {
                    let names = match style.value(Property::ContainerName) {
                        Value::Families(f) => f.as_slice().iter().filter_map(|f| if let p::Family::Named(n) = f { Some(n.clone()) } else { None }).collect(),
                        _ => Vec::new(),
                    };
                    let size = self.options.containers.and_then(|c| c.get(&n)).copied();
                    self.scratch.containers.push(cascade::ContainerFrame { node: n, names, size_axis: *kind == Keyword::Size, size });
                }
            }
            at = self.document.parent(n).map_err(Error::Dom)?;
        }
        Ok(())
    }
    fn compute_element(
        &mut self,
        node: NodeId,
        parent: Option<usize>,
        parent_box: Option<NonZeroU32>,
        host: &impl Host,
        previous_matched: Option<Arc<[(u32, selectors::Specificity)]>>,
    ) -> Result<(Style, Arc<[(u32, selectors::Specificity)]>)> {
        // A like sibling's style (same parent, name and attributes, and its
        // match met nothing they could differ in): taken, not matched.
        let parent_index = parent;
        let animated = self.options.animated.is_some_and(|a| a.contains_key(&node));
        let sharing = previous_matched.is_none() && !animated;
        if sharing {
            for i in (0..self.sharing.len()).rev() {
                if self.sharing[i].parent == parent && alike(self.document, self.sharing[i].node, node).map_err(Error::Dom)? && {
                    let dom = self.dom(node)?;
                    self.rules
                        .answers_like(&dom, node, selectors::MatchOptions { quirks: self.options.quirks, ..Default::default() }, &self.sharing[i].recorded, self.limits.matching, self.cancelled)
                        .map_err(Error::cascade)?
                } {
                    if let Some(t) = self.timing.as_mut() {
                        t.shared += 1;
                    }
                    return Ok((self.sharing[i].style.clone(), self.sharing[i].matched.clone()));
                }
            }
        }
        let clock = self.timing.as_ref().map(|_| std::time::Instant::now());
        self.containers_around(node)?;
        let mut hints = Vec::new();
        if self.document.namespace(node).map_err(Error::Dom)? == Some(crate::dom::Namespace::Html) {
            self.html_hints(node, &mut hints)?;
        }
        if self.document.namespace(node).map_err(Error::Dom)? == Some(crate::dom::Namespace::Svg) {
            self.svg_hints(node, &mut hints)?;
        }
        let mut inline = Vec::new();
        if let Some(source) = self.document.attribute(node, "style").map_err(Error::Dom)? {
            if source.len() > self.limits.inline_bytes {
                return Err(Error::Limit);
            }
            let syntax = Syntax::parse(Arc::from(source), self.limits.css, self.cancelled).map_err(Error::css)?;
            let declarations = syntax.declarations(self.cancelled).map_err(Error::css)?;
            if declarations.len() > self.limits.inline_declarations {
                return Err(Error::Limit);
            }
            for declaration in declarations {
                self.tick()?;
                match p::Declaration::compile(&declaration, self.limits.property, self.cancelled) {
                    Ok(declaration) => inline.push(declaration),
                    Err(p::Error::Invalid) => self.tree.report.inline_invalid += 1,
                    Err(p::Error::UnknownProperty | p::Error::Unsupported(_)) => self.tree.report.inline_unsupported += 1,
                    Err(error) => return Err(Error::Property(error)),
                }
            }
        }
        if let Some(animated) = self.options.animated.and_then(|a| a.get(&node)) {
            inline.extend(animated.iter().cloned());
        }
        let dom = self.dom(node)?;
        let parent = parent.map(|i| &self.tree.styles[i].style);
        let root = self.root.map(|i| &self.tree.styles[i].style);
        let layout_parent = parent_box.map(|i| match self.tree.styles[i.get() as usize - 1].style.value(Property::Display) {
            Value::Display(display) => BoxParent { display: *display, axes: self.tree.styles[i.get() as usize - 1].style.axes },
            _ => unreachable!(),
        });
        let mark_hints = clock.map(|c| c.elapsed().as_secs_f64()).unwrap_or(0.);
        if let Some(t) = self.timing.as_mut() {
            t.hints += mark_hints;
        }
        // An element whose inline style changed and nothing selectors see:
        // its rules from before, cascaded with the new declarations.
        let (cascade, matched) = match previous_matched {
            Some(matched) => {
                let cascade = self
                    .rules
                    .cascade_from_matched(&dom, node, &matched, Some(&inline), Some(&hints), parent.map(|p| p as &dyn cascade::Inherited), self.limits.cascade, self.cancelled)
                    .map_err(Error::cascade)?;
                (cascade, matched)
            }
            None => {
                let (cascade, record) = self
                    .rules
                    .match_element_recorded(
                        &dom,
                        node,
                        selectors::MatchOptions { quirks: self.options.quirks, ..Default::default() },
                        Some(&inline),
                        Some(&hints),
                        parent.map(|p| p as &dyn cascade::Inherited),
                        self.limits.cascade,
                        self.limits.matching,
                        &mut self.scratch,
                        Some(&self.ancestors),
                        self.cancelled,
                    )
                    .map_err(Error::cascade)?;
                (cascade, Arc::from(record))
            }
        };
        let mark_match = clock.map(|c| c.elapsed().as_secs_f64()).unwrap_or(0.);
        if let Some(t) = self.timing.as_mut() {
            t.matching += mark_match - mark_hints;
            t.candidates += self.scratch.candidate_count();
            t.elements += 1;
        }
        let style = Style::compute_reusing(
            self.defaults.clone(),
            &cascade,
            parent,
            root,
            self.tree.styles.last().map(|s| &s.style),
            Some(&mut self.fonts),
            Some(&mut self.customs),
            host,
            Options { root: parent.is_none(), layout_parent, ..Default::default() },
            self.limits.style,
            self.cancelled,
        )?;
        // Legacy alignment (<center>, align=) centres blocks and text but stops
        // at table parts, as browsers' -webkit-center does (HTML §15.3.9).
        let style = if matches!(self.document.name(node).map_err(Error::Dom)?, Some("table" | "td" | "th" | "caption" | "tr" | "thead" | "tbody" | "tfoot"))
            && self.document.namespace(node).map_err(Error::Dom)? == Some(crate::dom::Namespace::Html)
            && matches!(style.value(Property::TextAlign), Value::Keyword(Keyword::WebkitCenter | Keyword::WebkitLeft | Keyword::WebkitRight))
            && !self.document.attribute(node, "align").map_err(Error::Dom)?.is_some()
        {
            style.with_value(Property::TextAlign, Value::Keyword(Keyword::Start))
        } else {
            style
        };
        let mark_compute = clock.map(|c| c.elapsed().as_secs_f64()).unwrap_or(0.);
        if let Some(t) = self.timing.as_mut() {
            t.computing += mark_compute - mark_match;
        }
        // Kept for a like sibling when nothing in its match could differ.
        if sharing && !self.scratch.dependent {
            if self.sharing.len() >= SHARABLE {
                self.sharing.remove(0);
            }
            self.sharing.push(Sharable { parent: parent_index, node, recorded: self.scratch.recorded.clone(), style: style.clone(), matched: matched.clone() });
        }
        Ok((style, matched))
    }
    /// The element's `::before` and `::after` that generate content (CSS
    /// Generated Content 3 §2): styled with the element as parent.
    fn compute_pseudos(&mut self, node: NodeId, element: &Style, host: &impl Host) -> Result<Vec<(PseudoElement, Style)>> {
        let mut out = Vec::new();
        if !self.rules.has_pseudo_elements() {
            return Ok(out);
        }
        let dom = self.dom(node)?;
        for pseudo in [PseudoElement::Before, PseudoElement::After] {
            if !self.rules.has_pseudo(pseudo) {
                continue;
            }
            self.tick()?;
            // Only a rule declaring `content` makes the pseudo-element
            // exist: whether one could match is known from the rule index
            // alone (most elements stop here), then those few are tried.
            if !self.rules.may_generate(&dom, node, pseudo, self.limits.cascade, &mut self.scratch, Some(&self.ancestors), self.cancelled).map_err(Error::cascade)? {
                continue;
            }
            let generates = self
                .rules
                .match_element_with_hints(
                    &dom,
                    node,
                    selectors::MatchOptions { quirks: self.options.quirks, pseudo: Some(pseudo), content_only: true, ..Default::default() },
                    None,
                    None,
                    Some(element as &dyn cascade::Inherited),
                    self.limits.cascade,
                    self.limits.matching,
                    &mut self.scratch,
                    Some(&self.ancestors),
                    self.cancelled,
                )
                .map_err(Error::cascade)?
                .declared_properties()
                .any(|p| p == Property::Content);
            if !generates {
                continue;
            }
            let cascade = self
                .rules
                .match_element_with_hints(
                    &dom,
                    node,
                    selectors::MatchOptions { quirks: self.options.quirks, pseudo: Some(pseudo), ..Default::default() },
                    None,
                    None,
                    Some(element as &dyn cascade::Inherited),
                    self.limits.cascade,
                    self.limits.matching,
                    &mut self.scratch,
                    Some(&self.ancestors),
                    self.cancelled,
                )
                .map_err(Error::cascade)?;
            if !cascade.declared_properties().any(|p| p == Property::Content) {
                continue;
            }
            let layout_parent = match element.value(Property::Display) {
                Value::Display(display) => Some(BoxParent { display: *display, axes: element.axes }),
                _ => None,
            };
            let root = self.root.map(|i| &self.tree.styles[i].style);
            let style = Style::compute_reusing(
                self.defaults.clone(),
                &cascade,
                Some(element),
                root,
                None,
                Some(&mut self.fonts),
                Some(&mut self.customs),
                host,
                Options { root: false, layout_parent, ..Default::default() },
                self.limits.style,
                self.cancelled,
            )?;
            if !matches!(style.value(Property::Content), Value::Content(_)) || matches!(style.value(Property::Display), Value::Display(Display::None)) {
                continue;
            }
            out.push((pseudo, style));
        }
        // A text field's or area's placeholder: styled by `::placeholder`
        // rules, no `content` needed (the field shows it while empty).
        if self.rules.has_pseudo(PseudoElement::Placeholder)
            && matches!(self.document.name(node).map_err(Error::Dom)?, Some("input" | "textarea"))
            && self.document.attribute(node, "placeholder").map_err(Error::Dom)?.is_some()
        {
            self.tick()?;
            let cascade = self
                .rules
                .match_element_with_hints(
                    &dom,
                    node,
                    selectors::MatchOptions { quirks: self.options.quirks, pseudo: Some(PseudoElement::Placeholder), ..Default::default() },
                    None,
                    None,
                    Some(element as &dyn cascade::Inherited),
                    self.limits.cascade,
                    self.limits.matching,
                    &mut self.scratch,
                    Some(&self.ancestors),
                    self.cancelled,
                )
                .map_err(Error::cascade)?;
            let root = self.root.map(|i| &self.tree.styles[i].style);
            let style = Style::compute_reusing(
                self.defaults.clone(),
                &cascade,
                Some(element),
                root,
                None,
                Some(&mut self.fonts),
                Some(&mut self.customs),
                host,
                Options { root: false, layout_parent: None, ..Default::default() },
                self.limits.style,
                self.cancelled,
            )?;
            out.push((PseudoElement::Placeholder, style));
        }
        // A list item's marker: styled by `::marker` rules (its colour, its
        // font), no `content` needed. Keep the snapshot for computed-style
        // queries and incremental restyles as well as painting.
        if self.rules.has_pseudo(PseudoElement::Marker) && matches!(element.value(Property::Display), Value::Display(Display::Box { list_item: true, .. })) {
            self.tick()?;
            let cascade = self
                .rules
                .match_element_with_hints(
                    &dom,
                    node,
                    selectors::MatchOptions { quirks: self.options.quirks, pseudo: Some(PseudoElement::Marker), ..Default::default() },
                    None,
                    None,
                    Some(element as &dyn cascade::Inherited),
                    self.limits.cascade,
                    self.limits.matching,
                    &mut self.scratch,
                    Some(&self.ancestors),
                    self.cancelled,
                )
                .map_err(Error::cascade)?;
            if cascade.declared_properties().next().is_some() {
                let root = self.root.map(|i| &self.tree.styles[i].style);
                let style = Style::compute_reusing(
                    self.defaults.clone(),
                    &cascade,
                    Some(element),
                    root,
                    None,
                    Some(&mut self.fonts),
                    Some(&mut self.customs),
                    host,
                    Options { root: false, layout_parent: None, ..Default::default() },
                    self.limits.style,
                    self.cancelled,
                )?;
                out.push((PseudoElement::Marker, style));
            }
        }
        Ok(out)
    }
    pub fn finish(mut self) -> Result<StyleTree> {
        // Every subtree still open ends at the end.
        let len = self.tree.styles.len() as u32;
        for (_, i) in self.open.drain(..) {
            self.tree.styles[i].end = len;
        }
        self.tree.animated = (0..len).filter(|&i| self.tree.styles[i as usize].pseudo.is_none() && names_animation(&self.tree.styles[i as usize].style)).collect();
        self.tree.backdrops = (0..len).filter(|&i| self.tree.styles[i as usize].pseudo.is_none() && has_backdrop(&self.tree.styles[i as usize].style)).collect();
        // The previous tree's pseudo-elements must all be here again, else
        // one was lost (its element restyled without it): geometry changed.
        if let Some(inc) = &self.previous {
            let kept = inc.geometry_kept && inc.tree.pseudos.len() == self.tree.pseudos.len() && inc.tree.styles.len() == self.tree.styles.len();
            self.tree.geometry_kept = kept;
        }
        if let Some(t) = &self.timing {
            eprintln!(
                "style timing: {} elements ({} reused, {} shared), {} candidates ({:.1} per element; {} rejected, {} compounds, {} tests, {} steps): hints {:.1} ms, matching {:.1} ms, computing {:.1} ms, storing {:.1} ms, pseudo {:.1} ms ({} made); font cache {:?}",
                t.elements + self.progress.reused + t.shared,
                self.progress.reused,
                t.shared,
                t.candidates,
                t.candidates as f64 / t.elements.max(1) as f64,
                self.scratch.progress.rejected,
                self.scratch.progress.compounds,
                self.scratch.progress.tests,
                self.scratch.progress.steps,
                t.hints * 1000.,
                t.matching * 1000.,
                t.computing * 1000.,
                t.storing * 1000.,
                t.pseudo * 1000.,
                self.tree.pseudos.len(),
                self.fonts.stats()
            );
            match (&self.options.previous, &self.previous) {
                (Some(tree), None) => eprintln!(
                    "  incremental: off (previous tree document {} revision {} generation {}; this document {} revision {}; log reaches: {})",
                    tree.document,
                    tree.revision,
                    tree.generation,
                    self.document.id(),
                    self.document.revision(),
                    self.document.changes_since(tree.revision).map(|c| c.len().to_string()).unwrap_or_else(|| "no".into())
                ),
                (Some(_), Some(inc)) => eprintln!(
                    "  incremental: {} changed elements, {} changed child lists; geometry {}; :has() keys {:?}",
                    inc.nodes.len(),
                    inc.children.len(),
                    if inc.geometry_kept { "kept".to_owned() } else { format!("redone: {}", inc.why_not.as_deref().unwrap_or("?")) },
                    self.rules.has_keys()
                ),
                _ => {}
            }
            eprintln!(
                "  job: {:.1} ms in all ({:.1} ms before the first step), {} nodes visited, {} entries copied whole in {:.1} ms",
                t.started.elapsed().as_secs_f64() * 1000.,
                t.constructed * 1000.,
                t.visited,
                t.copied,
                t.copy * 1000.
            );
            let ms: Vec<String> = compute::PHASES.iter().map(|p| format!("{:.1}", p.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6)).collect();
            eprintln!(
                "  computing: resolver {} ms, fonts {} ms, declared {} ms, coupled {} ms, customs {} ms ({} elements merged {} inherited entries), reuse {} ms",
                ms[0],
                ms[1],
                ms[2],
                ms[3],
                ms[4],
                compute::COUNTS[0].load(std::sync::atomic::Ordering::Relaxed),
                compute::COUNTS[1].load(std::sync::atomic::Ordering::Relaxed),
                ms[5]
            );
            eprintln!(
                "  customs: {} dictionaries taken from the memo, {} made and kept",
                compute::COUNTS[2].load(std::sync::atomic::Ordering::Relaxed),
                compute::COUNTS[3].load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        if self.failed {
            Err(Error::Failed)
        } else {
            check(self.cancelled)?;
            if self.next.is_some() { Err(Error::Incomplete) } else { Ok(self.tree) }
        }
    }
}

// HTML dimension-value microsyntax, used only as an author presentational
// hint. Author CSS (including an explicit auto) participates in the cascade.
/// Legacy colour values: a `#`-less hex triplet becomes a hex colour.
fn legacy_color(value: &str) -> String {
    let v = value.trim();
    if !v.starts_with('#') && (v.len() == 6 || v.len() == 3) && v.bytes().all(|b| b.is_ascii_hexdigit()) { format!("#{v}") } else { v.to_owned() }
}
fn dimension_hint(input: &str) -> Option<String> {
    let input = input.trim_start_matches(['\t', '\n', '\u{c}', '\r', ' ']);
    let bytes = input.as_bytes();
    let mut at = 0;
    let mut value = 0.;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        value = value * 10. + f64::from(bytes[at] - b'0');
        at += 1;
        if !value.is_finite() {
            return None;
        }
    }
    if at == 0 {
        return None;
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let mut divisor = 1.;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            divisor *= 10.;
            value += f64::from(bytes[at] - b'0') / divisor;
            at += 1;
        }
    }
    Some(format!("{value}{}", if bytes.get(at) == Some(&b'%') { "%" } else { "px" }))
}
