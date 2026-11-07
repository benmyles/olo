use super::*;
use crate::css::selectors::PseudoElement;
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
    pub presentation_invalid: usize,
    pub presentation_unsupported: usize,
    /// Declarations dropped at computed-value time (see `Dependencies::fallbacks`).
    pub value_fallbacks: usize,
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
    /// `::before` and `::after` styles that generate content, by their
    /// element: entries in `styles` without a slot of their own.
    pseudos: std::collections::HashMap<(NodeId, PseudoElement), u32>,
    report: Report,
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
                pseudos: Default::default(),
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
                )
                || name == "fill" && animation
            {
                continue;
            }
            if source.len() > self.limits.inline_bytes || hints.len() >= self.limits.inline_declarations {
                return Err(Error::Limit);
            }
            let mut syntax = Syntax::parse(Arc::from(source), self.limits.css, self.cancelled).map_err(Error::css)?;
            if geometry && matches!(name, "width" | "height") || position && matches!(name, "x" | "y") || rounded && matches!(name, "rx" | "ry") {
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
    fn element(&mut self, node: NodeId, host: &impl Host) -> Result<()> {
        if self.tree.styles.len() >= self.limits.elements || self.tree.styles.len() >= u32::MAX as usize || node.slot() as usize >= self.limits.slots {
            return Err(Error::Limit);
        }
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
        let index = self.tree.styles.len();
        self.tree.report.value_fallbacks += usize::from(style.dependencies().fallbacks);
        self.tree.styles.push(Styled { node, style, parent_box });
        if self.root.is_none() {
            self.root = Some(index);
        }
        if self.tree.slots.len() <= node.slot() as usize {
            self.tree.slots.resize(node.slot() as usize + 1, None);
        }
        self.tree.slots[node.slot() as usize] = NonZeroU32::new(index as u32 + 1);
        self.account.add(&self.tree.styles[index].style);
        // ::before and ::after that generate content (CSS Generated Content
        // 3 §2): styled with the element as parent, kept beside its style.
        if self.rules.has_pseudo_elements() {
            for pseudo in [PseudoElement::Before, PseudoElement::After] {
                self.tick()?;
                let element = &self.tree.styles[index].style;
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
                    host,
                    Options { root: false, layout_parent, ..Default::default() },
                    self.limits.style,
                    self.cancelled,
                )?;
                if !matches!(style.value(Property::Content), Value::Content(_)) || matches!(style.value(Property::Display), Value::Display(Display::None)) {
                    continue;
                }
                if self.tree.styles.len() >= self.limits.elements || self.tree.styles.len() >= u32::MAX as usize {
                    return Err(Error::Limit);
                }
                self.tree.report.value_fallbacks += usize::from(style.dependencies().fallbacks);
                self.account.add(&style);
                let at = self.tree.styles.len() as u32;
                self.tree.styles.push(Styled { node, style, parent_box: NonZeroU32::new(index as u32 + 1) });
                self.tree.pseudos.insert((node, pseudo), at);
            }
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
