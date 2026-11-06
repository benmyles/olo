use super::*;
use crate::{
    css::selectors,
    dom::{Document, NodeId, NodeType},
};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Default)]
pub struct JobOptions<'a> {
    pub generation: u64,
    pub quirks: bool,
    /// Immutable state snapshot, including semantic direction. Missing state
    /// is an explicit matcher error if a selector needs it.
    pub states: Option<&'a dyn selectors::StateSource>,
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
    pub finished: bool,
}
/// Unsupported/unknown declarations remain visible in diagnostics; an eval
/// cannot turn a partially supported stylesheet into a browser pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub stylesheet_issues: usize,
    pub inline_invalid: usize,
    pub inline_unsupported: usize,
}
struct Styled {
    node: NodeId,
    style: Style,
    parent_box: Option<NonZeroU32>,
}
/// Completed styles for one immutable connected light document. No DOM,
/// stylesheet, parent or state-source borrow survives completion.
pub struct StyleTree {
    document: u64,
    revision: u64,
    generation: u64,
    slots: Vec<Option<NonZeroU32>>,
    styles: Vec<Styled>,
    report: Report,
}
impl StyleTree {
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
    pub(crate) fn snapshot(&self, node: NodeId) -> Option<&Style> {
        self.index(node).map(|i| &self.styles[i].style)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn document_revision(&self) -> u64 {
        self.revision
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
    tree: StyleTree,
    account: memory::Account,
    fonts: font_pool::FontPool,
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
            account: Default::default(),
            fonts: font_pool::FontPool::new(limits.font_cache_entries),
            tree: StyleTree {
                document: document.id(),
                revision: document.revision(),
                generation: options.generation,
                slots: Vec::new(),
                styles: Vec::new(),
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
            if self.document.node_type(node).map_err(Error::Dom)? == NodeType::Element {
                self.element(node, &host(node))?;
            }
            self.next = self.successor(node)?;
        }
        self.progress.finished = self.next.is_none();
        Ok(self.progress)
    }
    fn successor(&mut self, mut node: NodeId) -> Result<Option<NodeId>> {
        if let Some(child) = self.document.first_child(node).map_err(Error::Dom)? {
            if self.depth >= self.limits.depth {
                return Err(Error::Limit);
            }
            self.depth += 1;
            return Ok(Some(child));
        }
        while node != self.document.root() {
            self.tick()?;
            if let Some(sibling) = self.document.next_sibling(node).map_err(Error::Dom)? {
                return Ok(Some(sibling));
            }
            node = self.document.parent(node).map_err(Error::Dom)?.ok_or(Error::Invalid)?;
            self.depth = self.depth.checked_sub(1).ok_or(Error::Invalid)?;
        }
        Ok(None)
    }
    fn element(&mut self, node: NodeId, host: &impl Host) -> Result<()> {
        if self.tree.styles.len() >= self.limits.elements || self.tree.styles.len() >= u32::MAX as usize || node.slot() as usize >= self.limits.slots {
            return Err(Error::Limit);
        }
        let mut hints = Vec::new();
        if self.document.name(node).map_err(Error::Dom)? == Some("img") && self.document.namespace(node).map_err(Error::Dom)? == Some(crate::dom::Namespace::Html) {
            for name in ["width", "height"] {
                let Some(value) = self.document.attribute(node, name).map_err(Error::Dom)? else { continue };
                if value.len() > self.limits.inline_bytes {
                    return Err(Error::Limit);
                }
                if let Some(value) = dimension_hint(value) {
                    let syntax = Syntax::parse(Arc::from(value), self.limits.css, self.cancelled).map_err(Error::css)?;
                    hints.push(p::Declaration::parse(name, syntax.components(), false, self.limits.property, self.cancelled).map_err(Error::Property)?);
                }
            }
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
        let parent = self.document.parent(node).map_err(Error::Dom)?.and_then(|n| self.tree.index(n));
        let parent_box = parent
            .and_then(|i| if matches!(self.tree.styles[i].style.value(Property::Display), Value::Display(Display::Contents)) { self.tree.styles[i].parent_box } else { NonZeroU32::new(i as u32 + 1) });
        let parent = parent.map(|i| &self.tree.styles[i].style);
        let root = self.root.map(|i| &self.tree.styles[i].style);
        let layout_parent = parent_box.map(|i| match self.tree.styles[i.get() as usize - 1].style.value(Property::Display) {
            Value::Display(display) => BoxParent { display: *display, axes: self.tree.styles[i.get() as usize - 1].style.axes },
            _ => unreachable!(),
        });
        let dom = if let Some(states) = self.options.states { selectors::DomTree::with_states(self.document, states) } else { selectors::DomTree::new(self.document) };
        let cascade = self
            .rules
            .match_element_with_hints(
                &dom,
                node,
                selectors::MatchOptions { quirks: self.options.quirks, ..Default::default() },
                Some(&inline),
                Some(&hints),
                parent.map(|p| p as &dyn cascade::Inherited),
                self.limits.cascade,
                self.limits.matching,
                &mut self.scratch,
                self.cancelled,
            )
            .map_err(Error::cascade)?;
        let style = Style::compute_reusing(
            self.defaults.clone(),
            &cascade,
            parent,
            root,
            self.tree.styles.last().map(|s| &s.style),
            Some(&mut self.fonts),
            host,
            Options { root: parent.is_none(), layout_parent, ..Default::default() },
            self.limits.style,
            self.cancelled,
        )?;
        let index = self.tree.styles.len();
        self.tree.styles.push(Styled { node, style, parent_box });
        if self.root.is_none() {
            self.root = Some(index);
        }
        if self.tree.slots.len() <= node.slot() as usize {
            self.tree.slots.resize(node.slot() as usize + 1, None);
        }
        self.tree.slots[node.slot() as usize] = NonZeroU32::new(index as u32 + 1);
        self.account.add(&self.tree.styles[index].style);
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
        Ok(())
    }
    pub fn finish(self) -> Result<StyleTree> {
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
