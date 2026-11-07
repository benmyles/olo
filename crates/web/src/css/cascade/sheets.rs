use super::*;
use crate::css::{Components, Kind, Statement, StatementKind, Syntax};
use selectors::{Selectors, SubjectKey};
use std::collections::HashMap;
/// Unsupported/invalid source remains observable to conformance/eval callers.
/// An issue is never evidence that all original sheet semantics were applied.
#[derive(Clone, Debug)]
pub enum Issue {
    Invalid,
    Property(p::Error),
    Selector(selectors::Error),
    AtRule(Arc<str>),
    Nesting,
}
struct Rule {
    selector: u32,
    declarations: Box<[p::Declaration]>,
    origin: Origin,
    layer: u32,
    order: u64,
}
struct Layer {
    children: Vec<u32>,
}
#[derive(Default)]
struct Index {
    ids: HashMap<Arc<str>, Vec<u32>>,
    classes: HashMap<Arc<str>, Vec<u32>>,
    types: HashMap<Arc<str>, Vec<u32>>,
    universal: Vec<u32>,
}
/// Immutable ordered stylesheet set. Layer order is shared across sheets of
/// each origin, with named/nested/anonymous layers and implicit outer layers.
pub struct RuleSet {
    selectors: Selectors,
    rules: Vec<Rule>,
    index: Index,
    issues: Vec<Issue>,
}
#[derive(Clone, Copy)]
pub struct CompileLimits {
    pub rules: usize,
    pub declarations: usize,
    pub layers: usize,
    pub issues: usize,
    pub work: usize,
    pub depth: usize,
    pub properties: p::Limits,
    pub selectors: selectors::CompileLimits,
}
impl Default for CompileLimits {
    fn default() -> Self {
        Self { rules: 100000, declarations: 1_000_000, layers: 65536, issues: 65536, work: 16_000_000, depth: 64, properties: Default::default(), selectors: Default::default() }
    }
}
pub struct SheetBuilder {
    rules: Vec<Rule>,
    issues: Vec<Issue>,
    layers: Vec<Layer>,
    names: HashMap<(u32, Arc<str>), u32>,
    limits: CompileLimits,
    work: usize,
    declarations: usize,
    selector_compiler: selectors::RuleCompiler,
    /// What `@media` rules are evaluated against; without one they are issues.
    media: Option<crate::css::media::Media>,
    media_dependent: bool,
    faces: Vec<crate::css::fonts::FontFace>,
    /// The URL of the sheet being added, for its `@font-face` sources.
    base: Option<Arc<str>>,
    failed: bool,
}
impl SheetBuilder {
    pub fn new(limits: CompileLimits) -> Self {
        Self {
            rules: Vec::new(),
            issues: Vec::new(),
            layers: (0..6).map(|_| Layer { children: Vec::new() }).collect(),
            names: HashMap::new(),
            limits,
            work: 0,
            declarations: 0,
            selector_compiler: selectors::RuleCompiler::new(),
            media: None,
            media_dependent: false,
            faces: Vec::new(),
            base: None,
            failed: false,
        }
    }
    fn issue(&mut self, issue: Issue) -> Result<()> {
        if self.issues.len() >= self.limits.issues {
            return Err(Error::Limit);
        }
        self.issues.push(issue);
        Ok(())
    }
    fn tick(&mut self, n: usize, cancelled: Option<&AtomicBool>) -> Result<()> {
        check(cancelled)?;
        self.work = self.work.checked_add(n).ok_or(Error::Limit)?;
        if self.work > self.limits.work { Err(Error::Limit) } else { Ok(()) }
    }
    /// Evaluate `@media` and `@supports` rules for this medium. Rules under a
    /// false condition are left out; `media_dependent` says whether any
    /// `@media` rule was met, so a changed medium knows to recompile.
    pub fn with_media(mut self, media: crate::css::media::Media) -> Self {
        self.media = Some(media);
        self
    }
    pub fn media_dependent(&self) -> bool {
        self.media_dependent
    }
    /// The `@font-face` rules met so far, in order, under true conditions only.
    pub fn font_faces(&self) -> &[crate::css::fonts::FontFace] {
        &self.faces
    }
    pub fn add_sheet(&mut self, syntax: &Syntax, origin: Origin, cancelled: Option<&AtomicBool>) -> Result<()> {
        self.add_sheet_from(syntax, origin, None, cancelled)
    }
    /// A sheet fetched from `base`: its relative `url()`s resolve against it.
    pub fn add_sheet_from(&mut self, syntax: &Syntax, origin: Origin, base: Option<Arc<str>>, cancelled: Option<&AtomicBool>) -> Result<()> {
        if self.failed {
            return Err(Error::Invalid);
        }
        self.base = base;
        let result = self.sheet(syntax, origin, cancelled);
        self.base = None;
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn sheet(&mut self, syntax: &Syntax, origin: Origin, cancelled: Option<&AtomicBool>) -> Result<()> {
        if matches!(origin, Origin::Animation | Origin::Transition) {
            return Err(Error::Unsupported("keyframes/transition stylesheet compilation"));
        }
        self.tick(syntax.source().len(), cancelled)?;
        let rules = syntax.stylesheet(cancelled).map_err(syntax_error)?;
        let mut namespaces = Vec::<(Arc<str>, Arc<str>)>::new();
        let mut default_namespace = None;
        let mut kept = Vec::new();
        let mut phase = 0;
        for rule in rules {
            self.tick(1, cancelled)?;
            let name = rule.name().unwrap_or("");
            if rule.kind() == StatementKind::AtRule && name.eq_ignore_ascii_case("namespace") {
                if phase > 2 || rule.block().is_some() {
                    self.issue(Issue::Invalid)?;
                    continue;
                }
                match namespace(rule.value()) {
                    Some((prefix, uri)) => {
                        phase = 2;
                        if let Some(prefix) = prefix {
                            namespaces.push((Arc::from(prefix), Arc::from(uri)));
                        } else {
                            default_namespace = Some(Arc::from(uri));
                        }
                    }
                    None => self.issue(Issue::Invalid)?,
                }
            } else {
                if rule.kind() == StatementKind::AtRule && name.eq_ignore_ascii_case("charset") {
                } else if rule.kind() == StatementKind::AtRule && name.eq_ignore_ascii_case("import") && phase < 2 {
                    phase = 1;
                } else if rule.kind() == StatementKind::AtRule && name.eq_ignore_ascii_case("layer") && rule.block().is_none() && phase == 0 {
                } else {
                    phase = 3;
                }
                kept.push(rule);
            }
        }
        let borrowed: Vec<_> = namespaces.iter().map(|(a, b)| (a.as_ref(), b.as_ref())).collect();
        let options = selectors::CompileOptions { namespaces: &borrowed, default_namespace: default_namespace.as_deref() };
        self.walk(syntax, kept, origin, origin as u32, 0, options, cancelled)
    }
    fn layer(&mut self, parent: u32, name: Option<Arc<str>>, cancelled: Option<&AtomicBool>) -> Result<u32> {
        self.tick(1 + name.as_ref().map_or(0, |s| s.len()), cancelled)?;
        if let Some(name) = &name
            && let Some(&id) = self.names.get(&(parent, name.clone()))
        {
            return Ok(id);
        }
        if self.layers.len() >= self.limits.layers.min(u32::MAX as usize) {
            return Err(Error::Limit);
        }
        let id = self.layers.len() as u32;
        self.layers.push(Layer { children: Vec::new() });
        self.layers[parent as usize].children.push(id);
        if let Some(name) = name {
            self.names.insert((parent, name), id);
        }
        Ok(id)
    }
    fn layer_names(&mut self, values: Components<'_>, parent: u32, allow_many: bool, cancelled: Option<&AtomicBool>) -> Result<Vec<u32>> {
        // Validate the entire grammar before declaring any named layer.
        let mut paths = Vec::new();
        let mut path = Vec::<Arc<str>>::new();
        let (mut need_ident, mut dot, mut white) = (true, false, false);
        for c in values {
            self.tick(1 + (c.span().end - c.span().start) as usize, cancelled)?;
            if c.kind() == Kind::Whitespace {
                if !need_ident || dot {
                    white = true;
                }
                continue;
            }
            if c.kind() == Kind::Ident && need_ident && !(dot && white) {
                let name = c.text().unwrap();
                if ["initial", "inherit", "unset", "revert", "revert-layer", "revert-rule"].iter().any(|s| name.eq_ignore_ascii_case(s)) {
                    return Err(Error::Invalid);
                }
                path.push(Arc::from(name));
                need_ident = false;
                dot = false;
                white = false;
            } else if c.character() == Some('.') && !need_ident && !white {
                need_ident = true;
                dot = true;
            } else if c.kind() == Kind::Comma && !need_ident && allow_many {
                paths.push(std::mem::take(&mut path));
                need_ident = true;
                dot = false;
                white = false;
            } else {
                return Err(Error::Invalid);
            }
        }
        if need_ident {
            return Err(Error::Invalid);
        }
        paths.push(path);
        let mut layers = Vec::new();
        for path in paths {
            let mut current = parent;
            for name in path {
                current = self.layer(current, Some(name), cancelled)?;
            }
            layers.push(current);
        }
        Ok(layers)
    }
    #[allow(clippy::too_many_arguments)] // The sheet, its rules and their cascade context.
    fn walk(&mut self, syntax: &Syntax, rules: Vec<Statement<'_>>, origin: Origin, parent: u32, depth: usize, options: selectors::CompileOptions<'_>, cancelled: Option<&AtomicBool>) -> Result<()> {
        if depth > self.limits.depth.min(128) {
            return Err(Error::Limit);
        }
        for rule in rules {
            self.tick(1, cancelled)?;
            match rule.kind() {
                StatementKind::QualifiedRule => {
                    let selector = match self.selector_compiler.add(rule.value(), options, self.limits.selectors, cancelled) {
                        Ok(s) => s,
                        Err(selectors::Error::Invalid) => {
                            self.issue(Issue::Invalid)?;
                            continue;
                        }
                        Err(e @ selectors::Error::Unsupported(_)) => {
                            self.issue(Issue::Selector(e))?;
                            continue;
                        }
                        Err(e) => return Err(Error::Selectors(e)),
                    };
                    let mut declarations = Vec::new();
                    for d in rule.block_contents(cancelled).map_err(syntax_error)? {
                        self.tick(1, cancelled)?;
                        if d.kind() != StatementKind::Declaration {
                            self.issue(Issue::Nesting)?;
                            continue;
                        }
                        self.declarations += 1;
                        if self.declarations > self.limits.declarations {
                            return Err(Error::Limit);
                        }
                        match p::Declaration::compile(&d, self.limits.properties, cancelled) {
                            Ok(d) => declarations.push(d),
                            Err(e @ (p::Error::Invalid | p::Error::UnknownProperty | p::Error::Unsupported(_))) => self.issue(Issue::Property(e))?,
                            Err(e) => return Err(Error::Property(e)),
                        }
                    }
                    if self.rules.len() >= self.limits.rules.min(u32::MAX as usize) {
                        return Err(Error::Limit);
                    }
                    let order = self.rules.len() as u64;
                    self.rules.push(Rule { selector, declarations: declarations.into_boxed_slice(), origin, layer: parent, order });
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("layer")) => {
                    let empty = rule.value().all(|c| c.kind() == Kind::Whitespace);
                    let layers = if empty && rule.block().is_some() {
                        vec![self.layer(parent, None, cancelled)?]
                    } else {
                        match self.layer_names(rule.value(), parent, rule.block().is_none(), cancelled) {
                            Ok(l) => l,
                            Err(Error::Invalid) => {
                                self.issue(Issue::Invalid)?;
                                continue;
                            }
                            Err(e) => return Err(e),
                        }
                    };
                    if rule.block().is_some() {
                        self.walk(syntax, rule.block_contents(cancelled).map_err(syntax_error)?, origin, layers[0], depth + 1, options, cancelled)?;
                    }
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("media")) && self.media.is_some() => {
                    self.media_dependent = true;
                    if let Some(media) = &self.media
                        && crate::css::media::evaluate(rule.value(), media, cancelled)
                        && rule.block().is_some()
                    {
                        self.walk(syntax, rule.block_contents(cancelled).map_err(syntax_error)?, origin, parent, depth + 1, options, cancelled)?;
                    }
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("supports")) => {
                    if crate::css::media::supports(rule.value(), self.limits.properties, cancelled) && rule.block().is_some() {
                        self.walk(syntax, rule.block_contents(cancelled).map_err(syntax_error)?, origin, parent, depth + 1, options, cancelled)?;
                    }
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("font-face")) => {
                    if self.faces.len() >= 256 {
                        self.issue(Issue::AtRule(Arc::from("font-face")))?;
                    } else if let Some(mut face) = crate::css::fonts::parse(syntax, &rule, cancelled) {
                        face.base = self.base.clone();
                        self.faces.push(face);
                    }
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("charset")) => {}
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("namespace")) => self.issue(Issue::Invalid)?,
                StatementKind::AtRule => self.issue(Issue::AtRule(Arc::from(rule.name().unwrap_or(""))))?,
                _ => self.issue(Issue::Invalid)?,
            }
        }
        Ok(())
    }
    pub fn finish(mut self, cancelled: Option<&AtomicBool>) -> Result<RuleSet> {
        if self.failed {
            return Err(Error::Invalid);
        }
        let mut ranks = vec![u32::MAX; self.layers.len()];
        let mut rank = 0u32;
        let mut stack = Vec::new();
        for root in 0..6 {
            stack.push((root, false));
        }
        while let Some((id, visited)) = stack.pop() {
            self.tick(1, cancelled)?;
            if visited {
                ranks[id] = rank;
                rank = rank.checked_add(1).ok_or(Error::Limit)?;
            } else {
                stack.push((id, true));
                for &child in self.layers[id].children.iter().rev() {
                    stack.push((child as usize, false));
                }
            }
        }
        let selectors = self.selector_compiler.finish();
        let mut index = Index::default();
        for (i, rule) in self.rules.iter_mut().enumerate() {
            check(cancelled)?;
            rule.layer = if rule.layer < 6 { u32::MAX } else { ranks[rule.layer as usize] };
            if !cfg!(feature = "baseline-cascade-index") {
                for key in selectors.subject_keys(rule.selector) {
                    let (map, key) = match key {
                        SubjectKey::Universal => {
                            index.universal.push(i as u32);
                            continue;
                        }
                        SubjectKey::Id(k) => (&mut index.ids, k),
                        SubjectKey::Class(k) => (&mut index.classes, k),
                        SubjectKey::Type(k) => (&mut index.types, k),
                    };
                    let key = fold_arc(key);
                    map.entry(key).or_default().push(i as u32);
                }
            }
        }
        Ok(RuleSet { selectors, rules: self.rules, index, issues: self.issues })
    }
}
fn fold_arc(s: Arc<str>) -> Arc<str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) { Arc::from(s.to_ascii_lowercase()) } else { s }
}
fn folded(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) { std::borrow::Cow::Owned(s.to_ascii_lowercase()) } else { std::borrow::Cow::Borrowed(s) }
}
#[derive(Default)]
pub struct MatchScratch {
    seen: Vec<u32>,
    generation: u32,
    candidates: Vec<u32>,
}
impl MatchScratch {
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}
impl RuleSet {
    pub fn state_mask(&self) -> u64 {
        self.selectors.state_mask()
    }
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
    #[allow(clippy::too_many_arguments)] // Explicit tree/node/matching snapshot, inline/parent state, worker bounds and scratch.
    pub fn match_element<'a, T: selectors::Tree>(
        &'a self,
        tree: &T,
        node: T::Node,
        options: selectors::MatchOptions<'_, T::Node>,
        inline: Option<&'a [p::Declaration]>,
        parent: Option<&'a dyn Inherited>,
        limits: Limits,
        match_limits: selectors::MatchLimits,
        scratch: &mut MatchScratch,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Cascade<'a>> {
        self.match_element_with_hints(tree, node, options, inline, None, parent, limits, match_limits, scratch, cancelled)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn match_element_with_hints<'a, T: selectors::Tree>(
        &'a self,
        tree: &T,
        node: T::Node,
        options: selectors::MatchOptions<'_, T::Node>,
        inline: Option<&'a [p::Declaration]>,
        hints: Option<&'a [p::Declaration]>,
        parent: Option<&'a dyn Inherited>,
        limits: Limits,
        match_limits: selectors::MatchLimits,
        scratch: &mut MatchScratch,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Cascade<'a>> {
        check(cancelled)?;
        if options.visited_style {
            return Err(Error::Unsupported("private visited paint path"));
        }
        let selectors::Content::Element(name) = tree.content(node).map_err(Error::Selectors)? else {
            return Err(Error::Invalid);
        };
        scratch.candidates.clear();
        if cfg!(feature = "baseline-cascade-index") {
            scratch.candidates.extend(0..self.rules.len() as u32);
        } else {
            if scratch.seen.len() < self.rules.len() {
                scratch.seen.resize(self.rules.len(), 0);
            }
            scratch.generation = scratch.generation.wrapping_add(1);
            if scratch.generation == 0 {
                scratch.seen.fill(0);
                scratch.generation = 1;
            }
            let mut work = 0usize;
            let mut add = |list: Option<&Vec<u32>>, work: &mut usize| -> Result<()> {
                if let Some(list) = list {
                    for &id in list {
                        check(cancelled)?;
                        *work = work.saturating_add(1);
                        if *work > limits.work {
                            return Err(Error::Limit);
                        }
                        if scratch.seen[id as usize] != scratch.generation {
                            scratch.seen[id as usize] = scratch.generation;
                            scratch.candidates.push(id);
                        }
                    }
                }
                Ok(())
            };
            add(Some(&self.index.universal), &mut work)?;
            add(self.index.types.get(folded(name.local_name).as_ref()), &mut work)?;
            for a in tree.attributes(node).map_err(Error::Selectors)? {
                check(cancelled)?;
                work = work.saturating_add(a.value.len() + a.local_name.len() + 1);
                if work > limits.work {
                    return Err(Error::Limit);
                }
                if !a.namespace.is_empty() {
                    continue;
                }
                if a.local_name.eq_ignore_ascii_case("id") {
                    add(self.index.ids.get(folded(a.value).as_ref()), &mut work)?;
                } else if a.local_name.eq_ignore_ascii_case("class") {
                    for class in a.value.split([' ', '\t', '\n', '\r', '\x0c']).filter(|s| !s.is_empty()) {
                        add(self.index.classes.get(folded(class).as_ref()), &mut work)?;
                    }
                }
            }
        }
        if scratch.candidates.len() > limits.candidates {
            return Err(Error::Limit);
        }
        let has_parent = if options.pseudo.is_some() {
            true
        } else if let Some(parent) = tree.parent(node).map_err(Error::Selectors)? {
            matches!(tree.content(parent).map_err(Error::Selectors)?, selectors::Content::Element(_))
        } else {
            false
        };
        let mut builder = Builder::new(limits, cancelled);
        let mut work = 0usize;
        for &id in &scratch.candidates {
            check(cancelled)?;
            let rule = &self.rules[id as usize];
            let bounded = selectors::MatchLimits { work: match_limits.work.min(limits.work.saturating_sub(work)), ..match_limits };
            let (matched, progress) = self.selectors.matches_root_accounted(rule.selector, tree, node, options, bounded, cancelled).map_err(Error::Selectors)?;
            work = work.checked_add(progress).ok_or(Error::Limit)?;
            if let Some(specificity) = matched {
                for (declaration, d) in rule.declarations.iter().enumerate() {
                    builder.add(d, Priority { origin: rule.origin, layer: rule.layer, specificity, order: rule.order, declaration: declaration as u32, rule: id as u64 + 1, ..Default::default() })?;
                }
            }
        }
        if options.pseudo.is_none()
            && let Some(inline) = inline
        {
            for (i, d) in inline.iter().enumerate() {
                builder.add(d, Priority { inline: true, order: u64::MAX, rule: u64::MAX, declaration: i as u32, ..Default::default() })?;
            }
        }
        if options.pseudo.is_none()
            && let Some(hints) = hints
        {
            for (i, d) in hints.iter().enumerate() {
                builder.add(d, Priority { origin: Origin::PresentationalHint, rule: u64::MAX - 1, declaration: i as u32, ..Default::default() })?;
            }
        }
        builder.finish(has_parent, parent)
    }
}

fn syntax_error(e: crate::css::Error) -> Error {
    if e == crate::css::Error::Cancelled { Error::Cancelled } else { Error::Limit }
}
fn namespace(values: Components<'_>) -> Option<(Option<&str>, &str)> {
    let mut parts = values.filter(|c| c.kind() != Kind::Whitespace);
    let first = parts.next()?;
    let (prefix, uri) = if first.kind() == Kind::Ident { (Some(first.text()?), parts.next()?) } else { (None, first) };
    if parts.next().is_some() {
        return None;
    }
    let uri = if matches!(uri.kind(), Kind::String | Kind::UnclosedString | Kind::Url | Kind::UnclosedUrl) {
        uri.text()?
    } else if uri.kind() == Kind::Function && uri.text()?.eq_ignore_ascii_case("url") {
        let mut parts = uri.children().filter(|c| c.kind() != Kind::Whitespace);
        let s = parts.next()?;
        if parts.next().is_some() || !matches!(s.kind(), Kind::String | Kind::UnclosedString) {
            return None;
        }
        s.text()?
    } else {
        return None;
    };
    Some((prefix, uri))
}

#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub rules: usize,
    pub declarations: usize,
    pub rule_array_bytes: usize,
    pub declaration_array_bytes: usize,
    pub selector_reserved_bytes: usize,
    pub index_entries: usize,
    pub index_vector_bytes: usize,
    pub index_key_value_capacity_bytes: usize,
}
impl RuleSet {
    pub fn memory(&self) -> Memory {
        let mut m = Memory {
            rules: self.rules.len(),
            declarations: 0,
            rule_array_bytes: self.rules.capacity() * size_of::<Rule>(),
            declaration_array_bytes: 0,
            selector_reserved_bytes: 0,
            index_entries: self.index.universal.len(),
            index_vector_bytes: self.index.universal.capacity() * 4,
            index_key_value_capacity_bytes: 0,
        };
        for rule in &self.rules {
            m.declarations += rule.declarations.len();
            m.declaration_array_bytes += rule.declarations.len() * size_of::<p::Declaration>();
        }
        m.selector_reserved_bytes = self.selectors.memory().accounted_reserved_bytes;
        for map in [&self.index.ids, &self.index.classes, &self.index.types] {
            m.index_key_value_capacity_bytes += map.capacity() * size_of::<(Arc<str>, Vec<u32>)>();
            for list in map.values() {
                m.index_entries += list.len();
                m.index_vector_bytes += list.capacity() * 4;
            }
        }
        m
    }
}
