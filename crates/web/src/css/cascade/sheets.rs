use super::*;
use crate::css::{Components, Kind, Statement, StatementKind, Syntax};
use selectors::{Selectors, SubjectKey};
use std::collections::HashMap;
/// Unsupported/invalid source remains observable to conformance/eval callers.
/// An issue is never evidence that all original sheet semantics were applied.
#[derive(Clone, Debug)]
pub enum Issue {
    Invalid,
    /// A declaration the engine dropped, and its property name.
    Property(p::Error, Box<str>),
    /// A selector the engine dropped, and its source (bounded).
    Selector(selectors::Error, Box<str>),
    AtRule(Arc<str>),
    Nesting,
}
/// A rule's declaration block compiled, with what was dropped from it.
#[derive(Clone)]
pub(crate) struct CompiledBlock {
    declarations: Arc<[p::Declaration]>,
    issues: Arc<[Issue]>,
    /// Its declarations (those dropped too), for the compile's limits.
    count: usize,
}
struct Rule {
    selector: u32,
    declarations: Arc<[p::Declaration]>,
    origin: Origin,
    layer: u32,
    order: u64,
    /// The `@container` condition it is under (index into `containers`),
    /// or NO_CONTAINER.
    container: u32,
    /// The shadow tree whose sheets it is from (`selectors::scope_key`),
    /// or None for the document's (and the user agent's, which every tree
    /// takes); with whether it addresses that tree's host (`:host`) and
    /// elements slotted into it (`::slotted()`).
    scope: Option<u64>,
    host: bool,
    slotted: bool,
    /// It addresses parts of shadow trees hosted in its own (`::part()`).
    part: bool,
}
const NO_CONTAINER: u32 = u32::MAX;
impl Rule {
    /// Whether it applies to an element taking rules from `scopes`
    /// (`selectors::Tree::scopes`), and at what shadow encapsulation (CSS
    /// Cascade 5 §6.1, context; for normal declarations the lower wins):
    /// an outer tree's `::part()` rules at 0; its own tree's rules and the
    /// user agent's at 1; its shadow tree's `:host` and `::slotted()` rules
    /// at 2. The document's author rules don't reach into shadow trees
    /// otherwise.
    fn context(&self, scopes: &[Option<u64>; 4]) -> Option<u16> {
        if self.part && scopes[0].is_some() && self.scope == scopes[3] {
            return Some(0);
        }
        if self.origin == Origin::UserAgent || self.scope == scopes[0] {
            return Some(1);
        }
        (self.scope.is_some() && (self.host && self.scope == scopes[1] || self.slotted && self.scope == scopes[2])).then_some(2)
    }
}
/// An `@container` rule's condition (CSS Conditional 5 §2): the name of
/// the container it asks (any container when none), its query, and the
/// `@container` it is itself inside (all must hold).
pub struct ContainerCondition {
    pub name: Option<Arc<str>>,
    pub query: crate::css::Syntax,
    pub parent: u32,
    /// Whether the query reads the block axis (a `size` container only answers it).
    pub block_axis: bool,
}
/// A container an element's queries may ask: the element, its names,
/// whether it answers block-axis queries (`container-type: size`), and its
/// content box's size (CSS px) as the last layout had it, if known.
#[derive(Clone, Debug)]
pub struct ContainerFrame {
    pub node: crate::dom::NodeId,
    pub names: Vec<Arc<str>>,
    pub size_axis: bool,
    pub size: Option<[f64; 2]>,
}
struct Layer {
    children: Vec<u32>,
}
/// Rules by their subject's key: an element's candidates are the rules
/// keyed by its id, classes and type, plus the universal ones.
#[derive(Default)]
struct Buckets {
    ids: HashMap<Arc<str>, Vec<u32>>,
    classes: HashMap<Arc<str>, Vec<u32>>,
    types: HashMap<Arc<str>, Vec<u32>>,
    attributes: HashMap<Arc<str>, Vec<u32>>,
    root: Vec<u32>,
    universal: Vec<u32>,
}
impl Buckets {
    fn add(&mut self, key: SubjectKey, rule: u32) {
        let (map, key) = match key {
            SubjectKey::Universal => {
                if self.universal.last() != Some(&rule) {
                    self.universal.push(rule);
                }
                return;
            }
            SubjectKey::Root => {
                if self.root.last() != Some(&rule) {
                    self.root.push(rule);
                }
                return;
            }
            SubjectKey::Id(k) => (&mut self.ids, k),
            SubjectKey::Class(k) => (&mut self.classes, k),
            SubjectKey::Type(k) => (&mut self.types, k),
            SubjectKey::Attr(k) => (&mut self.attributes, k),
        };
        let list = map.entry(fold_arc(key)).or_default();
        if list.last() != Some(&rule) {
            list.push(rule);
        }
    }
}
#[derive(Default)]
struct Index {
    element: Scoped,
    /// Rules with a branch addressing a pseudo-element, by it: the only
    /// candidates when styling one, and never candidates for an element.
    pseudo: HashMap<selectors::PseudoElement, Scoped>,
    /// Of those, the rules declaring `content`: a pseudo-element exists only
    /// when one of them matches, so they are tried first, alone.
    pseudo_content: HashMap<selectors::PseudoElement, Scoped>,
}
/// Buckets by the tree a rule is from (`Rule::scope`; the user agent's
/// rules under `UA_SCOPE`): an element gathers candidates only from the
/// trees whose rules can reach it (its own, the user agent's, those it
/// hosts or is slotted into), not every shadow tree's copy of a
/// component's sheet (MDN's buttons: 1076 `.button` rules each).
type Scoped = HashMap<Option<u64>, Buckets>;
const UA_SCOPE: Option<u64> = Some(u64::MAX);
fn bucket_scope(rule: &Rule) -> Option<u64> {
    if rule.origin == Origin::UserAgent { UA_SCOPE } else { rule.scope }
}
/// Immutable ordered stylesheet set. Layer order is shared across sheets of
/// each origin, with named/nested/anonymous layers and implicit outer layers.
pub struct RuleSet {
    selectors: Selectors,
    rules: Vec<Rule>,
    index: Index,
    issues: Vec<Issue>,
    has_keys: selectors::HasKeys,
    invalidation: selectors::Invalidation,
    uses_empty: bool,
    /// `@keyframes` by name (the last of a name).
    keyframes: crate::css::animation::Library,
    /// Unique to this compilation: styles made with another rule set
    /// (their recorded rule ids point into it) are not reused with this.
    generation: u64,
    containers: Vec<ContainerCondition>,
}
static GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
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
    keyframes: crate::css::animation::Library,
    /// `url()` images of `background-image` declarations, with the URL of
    /// the sheet each came from, in order; a page fetches them.
    backgrounds: Vec<(Arc<str>, Option<Arc<str>>)>,
    /// The URL of the sheet being added, for its `@font-face` sources.
    base: Option<Arc<str>>,
    failed: bool,
    containers: Vec<ContainerCondition>,
    /// The `@container` condition the rules being read are under.
    container: u32,
    /// The shadow tree the sheet being added belongs to (see `Rule::scope`).
    scope: Option<u64>,
    scoped: bool,
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
            keyframes: Default::default(),
            backgrounds: Vec::new(),
            base: None,
            failed: false,
            containers: Vec::new(),
            container: NO_CONTAINER,
            scope: None,
            scoped: false,
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
    /// The `background-image` URLs met so far and the sheet URL each
    /// resolves against, in order, under true conditions only.
    pub fn background_urls(&self) -> &[(Arc<str>, Option<Arc<str>>)] {
        &self.backgrounds
    }
    const BACKGROUNDS: usize = 1024;
    fn note_backgrounds(&mut self, d: &p::Declaration) {
        let p::Declaration::Known { values, .. } = d else { return };
        for a in values.iter() {
            if let p::Value::Images(images) = &a.value {
                for image in images.0.iter().flatten() {
                    if let p::Image::Url(url) = image
                        && self.backgrounds.len() < Self::BACKGROUNDS
                        && !self.backgrounds.iter().any(|(u, b)| u == url && *b == self.base)
                    {
                        self.backgrounds.push((url.clone(), self.base.clone()));
                    }
                }
            }
        }
    }
    pub fn add_sheet(&mut self, syntax: &Syntax, origin: Origin, cancelled: Option<&AtomicBool>) -> Result<()> {
        self.add_sheet_from(syntax, origin, None, cancelled)
    }
    /// A sheet of a shadow tree (`scope`, its `selectors::scope_key`):
    /// its rules apply in that tree, to its host through `:host` and to
    /// what is slotted into it through `::slotted()` (CSS Scoping 1 §3).
    pub fn add_scoped_sheet_from(&mut self, syntax: &Syntax, origin: Origin, base: Option<Arc<str>>, scope: Option<u64>, cancelled: Option<&AtomicBool>) -> Result<()> {
        self.scope = scope;
        self.scoped |= scope.is_some();
        let result = self.add_sheet_from(syntax, origin, base, cancelled);
        self.scope = None;
        result
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
                            let mut parts = rule.value().filter(|c| c.kind() != Kind::Whitespace);
                            let text = match (parts.next(), parts.last()) {
                                (Some(first), Some(last)) => &syntax.source()[first.span().start as usize..last.span().end as usize],
                                (Some(only), None) => &syntax.source()[only.span().start as usize..only.span().end as usize],
                                _ => "",
                            };
                            let end = text.char_indices().nth(80).map_or(text.len(), |(i, _)| i);
                            self.issue(Issue::Selector(e, text[..end].into()))?;
                            continue;
                        }
                        Err(e) => return Err(Error::Selectors(e)),
                    };
                    let block = Self::block(&rule, self.limits, cancelled)?;
                    self.tick(block.count + block.issues.len(), cancelled)?;
                    self.declarations += block.count;
                    if self.declarations > self.limits.declarations {
                        return Err(Error::Limit);
                    }
                    for issue in block.issues.iter() {
                        self.issue(issue.clone())?;
                    }
                    for d in block.declarations.iter() {
                        self.note_backgrounds(d);
                    }
                    let declarations = block.declarations;
                    if self.rules.len() >= self.limits.rules.min(u32::MAX as usize) {
                        return Err(Error::Limit);
                    }
                    let order = self.rules.len() as u64;
                    let (host, slotted, part) = self.selector_compiler.shadow_context(selector);
                    let (host, slotted) = if self.scope.is_some() { (host, slotted) } else { (false, false) };
                    self.rules.push(Rule { selector, declarations, origin, layer: parent, order, container: self.container, scope: self.scope, host, slotted, part });
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
                // `@container [<name>] <condition>`: its rules hold where the
                // element's container meets it, decided as elements match.
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("container")) => {
                    let parts: Vec<_> = rule.value().filter(|c| c.kind() != Kind::Whitespace).collect();
                    let named = parts.first().is_some_and(|c| c.kind() == Kind::Ident && !c.text().is_some_and(|t| t.eq_ignore_ascii_case("not")));
                    let name = if named { parts[0].text().map(Arc::from) } else { None };
                    let rest = &parts[usize::from(named)..];
                    let (Some(first), Some(last)) = (rest.first(), rest.last()) else {
                        self.issue(Issue::Invalid)?;
                        continue;
                    };
                    let text = &syntax.source()[first.span().start as usize..last.span().end as usize];
                    let lower = text.to_ascii_lowercase();
                    if lower.contains("style(") || lower.contains("scroll-state(") {
                        self.issue(Issue::AtRule(Arc::from("container style()")))?;
                        continue;
                    }
                    let Ok(query) = crate::css::Syntax::parse(Arc::from(text), Default::default(), cancelled) else {
                        self.issue(Issue::Invalid)?;
                        continue;
                    };
                    if self.containers.len() >= 65536 || rule.block().is_none() {
                        self.issue(Issue::Invalid)?;
                        continue;
                    }
                    let block_axis = ["height", "block-size", "aspect-ratio", "orientation"].iter().any(|f| lower.contains(f));
                    self.containers.push(ContainerCondition { name, query, parent: self.container, block_axis });
                    let outer = std::mem::replace(&mut self.container, self.containers.len() as u32 - 1);
                    let walked = self.walk(syntax, rule.block_contents(cancelled).map_err(syntax_error)?, origin, parent, depth + 1, options, cancelled);
                    self.container = outer;
                    walked?;
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
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("keyframes") || s.eq_ignore_ascii_case("-webkit-keyframes")) => {
                    match crate::css::animation::parse_keyframes(syntax, &rule, cancelled) {
                        Some((name, frames)) => {
                            self.keyframes.insert(name, std::sync::Arc::new(frames));
                        }
                        None => self.issue(Issue::Invalid)?,
                    }
                }
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("charset")) => {}
                // Loaded with the sheet that imports it (page::imported).
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("import")) => {}
                StatementKind::AtRule if rule.name().is_some_and(|s| s.eq_ignore_ascii_case("namespace")) => self.issue(Issue::Invalid)?,
                StatementKind::AtRule => self.issue(Issue::AtRule(Arc::from(rule.name().unwrap_or(""))))?,
                _ => self.issue(Issue::Invalid)?,
            }
        }
        Ok(())
    }
    /// A style rule's declarations, compiled: as the sheet keeps them from
    /// an earlier compile, else compiled now and kept there.
    fn block(rule: &Statement<'_>, limits: CompileLimits, cancelled: Option<&AtomicBool>) -> Result<CompiledBlock> {
        let key = rule.block_start();
        if let Some(key) = key
            && let Some(block) = rule.sheet().blocks.lock().unwrap().get(&key)
        {
            return Ok(block.clone());
        }
        let mut declarations = Vec::new();
        let mut issues = Vec::new();
        let mut count = 0usize;
        for d in rule.block_contents(cancelled).map_err(syntax_error)? {
            if d.kind() != StatementKind::Declaration {
                issues.push(Issue::Nesting);
                continue;
            }
            count += 1;
            if count > limits.declarations {
                return Err(Error::Limit);
            }
            match p::Declaration::compile(&d, limits.properties, cancelled) {
                Ok(d) => declarations.push(d),
                Err(e @ (p::Error::Invalid | p::Error::UnknownProperty | p::Error::Unsupported(_))) => issues.push(Issue::Property(e, d.name().unwrap_or("").into())),
                Err(e) => return Err(Error::Property(e)),
            }
        }
        let block = CompiledBlock { declarations: declarations.into(), issues: issues.into(), count };
        if let Some(key) = key {
            rule.sheet().blocks.lock().unwrap().insert(key, block.clone());
        }
        Ok(block)
    }
    /// Compile a sheet's declaration blocks ahead (as it arrives, on a
    /// thread of the loader's): every style rule's, under whatever
    /// condition, kept with the sheet for the compiles that take it.
    pub fn precompile(syntax: &Syntax, cancelled: Option<&AtomicBool>) -> Result<()> {
        fn walk(rules: Vec<Statement<'_>>, depth: usize, cancelled: Option<&AtomicBool>) -> Result<()> {
            if depth > 32 {
                return Ok(());
            }
            for rule in rules {
                match rule.kind() {
                    StatementKind::QualifiedRule => {
                        SheetBuilder::block(&rule, CompileLimits::default(), cancelled)?;
                    }
                    StatementKind::AtRule if rule.block().is_some() && rule.name().is_some_and(|n| ["media", "supports", "layer", "container"].iter().any(|k| n.eq_ignore_ascii_case(k))) => {
                        walk(rule.block_contents(cancelled).map_err(syntax_error)?, depth + 1, cancelled)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        walk(syntax.stylesheet(cancelled).map_err(syntax_error)?, 0, cancelled)
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
                let content = rule.declarations.iter().any(|d| matches!(d, p::Declaration::Known { values, .. } if values.iter().any(|a| a.property == p::Property::Content)));
                let scope = bucket_scope(rule);
                for (key, pseudo) in selectors.subject_keys(rule.selector) {
                    match pseudo {
                        Some(pseudo) => {
                            if content {
                                index.pseudo_content.entry(pseudo).or_default().entry(scope).or_default().add(key.clone(), i as u32);
                            }
                            index.pseudo.entry(pseudo).or_default().entry(scope).or_default().add(key, i as u32)
                        }
                        None => index.element.entry(scope).or_default().add(key, i as u32),
                    }
                }
            }
        }
        if crate::env_on!("OLO_STYLE_TIMING") {
            let biggest = |b: &Buckets| b.classes.values().chain(b.types.values()).map(Vec::len).max().unwrap_or(0);
            let mut kinds: HashMap<String, usize> = HashMap::new();
            let all = || index.element.values();
            for &i in all().flat_map(|b| b.universal.iter()) {
                *kinds.entry(selectors.subject_summary(self.rules[i as usize].selector)).or_default() += 1;
            }
            let mut kinds: Vec<_> = kinds.into_iter().collect();
            kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            eprintln!("universal rules by subject: {kinds:?}");
            let mut buckets: Vec<(String, usize)> =
                all().flat_map(|b| b.classes.iter().map(|(k, v)| (format!(".{k:?}"), v.len())).chain(b.types.iter().map(|(k, v)| (format!("{k:?}"), v.len())))).collect();
            buckets.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            eprintln!("biggest buckets: {:?}", &buckets[..buckets.len().min(6)]);
            eprintln!(
                "rule index: {} rules; element: {} universal, {} ids, {} classes, {} types, biggest bucket {}; pseudo: {:?}",
                self.rules.len(),
                all().map(|b| b.universal.len()).sum::<usize>(),
                all().map(|b| b.ids.len()).sum::<usize>(),
                all().map(|b| b.classes.len()).sum::<usize>(),
                all().map(|b| b.types.len()).sum::<usize>(),
                all().map(biggest).max().unwrap_or(0),
                index
                    .pseudo
                    .iter()
                    .map(|(p, s)| (*p, s.values().map(|b| b.universal.len()).sum::<usize>(), s.values().map(|b| b.classes.len() + b.types.len() + b.ids.len()).sum::<usize>()))
                    .collect::<Vec<_>>()
            );
        }
        let has_keys = selectors.has_keys();
        let invalidation = selectors.invalidation();
        let uses_empty = selectors.uses_empty();
        let generation = GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(RuleSet { selectors, rules: self.rules, index, issues: self.issues, has_keys, invalidation, uses_empty, keyframes: self.keyframes, generation, containers: self.containers })
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
    /// The containers around the element being matched, nearest first
    /// (set by the caller when the rules have `@container` conditions).
    pub containers: Vec<ContainerFrame>,
    /// Container conditions decided, by condition and container.
    decided: HashMap<(u32, crate::dom::NodeId), bool>,
    /// Summed over every match made with this scratch.
    pub progress: selectors::MatchProgress,
    /// Whether the last element's match met a test a like sibling could
    /// answer otherwise (see `selectors::Matcher::dependent`): else a
    /// sibling with its parent, name and attributes may take its style.
    pub dependent: bool,
    /// The tests met on it a like sibling must answer the same (see
    /// `RuleSet::answers_like`).
    pub recorded: Vec<(u32, bool)>,
}
impl MatchScratch {
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}
impl RuleSet {
    /// Whether any rule is under an `@container` condition (then callers
    /// give `MatchScratch::containers` and lay out again when a
    /// container's size changes what holds).
    pub fn has_containers(&self) -> bool {
        !self.containers.is_empty()
    }
    /// Whether condition `id` (and those it is inside) holds for an element
    /// with `frames` around it: each asks the nearest container with its
    /// name (any, when unnamed) that can answer it; one with no known size,
    /// or none at all, holds nowhere.
    fn container_holds(&self, id: u32, frames: &[ContainerFrame], decided: &mut HashMap<(u32, crate::dom::NodeId), bool>) -> bool {
        let mut at = id;
        let mut steps = 0;
        while at != NO_CONTAINER {
            steps += 1;
            let Some(c) = self.containers.get(at as usize) else { return false };
            if steps > 64 {
                return false;
            }
            let Some(frame) = frames.iter().find(|f| (!c.block_axis || f.size_axis) && c.name.as_ref().is_none_or(|n| f.names.iter().any(|m| m == n))) else { return false };
            let holds = *decided.entry((at, frame.node)).or_insert_with(|| match frame.size {
                Some([w, h]) => {
                    let media = crate::css::media::Media { width: w, height: h, ..crate::css::media::Media::screen(w, h, 1., false) };
                    crate::css::media::evaluate_condition(c.query.components(), &media)
                }
                None => false,
            });
            if !holds {
                return false;
            }
            at = c.parent;
        }
        true
    }
    /// The `@keyframes` rules, by name.
    pub fn keyframes(&self) -> &crate::css::animation::Library {
        &self.keyframes
    }
    pub fn state_mask(&self) -> u64 {
        self.selectors.state_mask()
    }
    pub fn has_pseudo_elements(&self) -> bool {
        self.selectors.has_pseudo_elements()
    }
    pub fn depends_on_descendants(&self) -> bool {
        self.selectors.depends_on_descendants()
    }
    pub fn has_keys(&self) -> &selectors::HasKeys {
        &self.has_keys
    }
    /// See the field: the generation for styles' `JobOptions`.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn invalidation(&self) -> &selectors::Invalidation {
        &self.invalidation
    }
    pub fn uses_empty(&self) -> bool {
        self.uses_empty
    }
    /// Whether any rule addresses this pseudo-element.
    pub fn has_pseudo(&self, pseudo: selectors::PseudoElement) -> bool {
        self.index.pseudo.contains_key(&pseudo)
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
        ancestors: Option<&selectors::AncestorFilter>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Cascade<'a>> {
        self.match_element_with_hints(tree, node, options, inline, None, parent, limits, match_limits, scratch, ancestors, cancelled)
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
        ancestors: Option<&selectors::AncestorFilter>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Cascade<'a>> {
        self.match_inner(tree, node, options, inline, hints, parent, limits, match_limits, scratch, ancestors, cancelled, None)
    }
    /// As `match_element_with_hints`, also answering which rules matched
    /// (ids and specificities, in candidate order) for `cascade_from_matched`.
    #[allow(clippy::too_many_arguments)]
    pub fn match_element_recorded<'a, T: selectors::Tree>(
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
        ancestors: Option<&selectors::AncestorFilter>,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<(Cascade<'a>, Vec<(u32, selectors::Specificity)>)> {
        let mut record = Vec::new();
        let cascade = self.match_inner(tree, node, options, inline, hints, parent, limits, match_limits, scratch, ancestors, cancelled, Some(&mut record))?;
        Ok((cascade, record))
    }
    /// Whether `node` answers the tests a like sibling's match met on it
    /// (`MatchScratch::recorded`) as the sibling did: it then matches the
    /// same rules, and takes the sibling's style (style sharing).
    pub fn answers_like<T: selectors::Tree>(
        &self,
        tree: &T,
        node: T::Node,
        options: selectors::MatchOptions<'_, T::Node>,
        tests: &[(u32, bool)],
        match_limits: selectors::MatchLimits,
        cancelled: Option<&AtomicBool>,
    ) -> Result<bool> {
        if tests.is_empty() {
            return Ok(true);
        }
        let mut keys = selectors::KeyCache::default();
        let mut matcher = self.selectors.matcher(tree, node, options, match_limits, cancelled, &mut keys).map_err(Error::Selectors)?;
        matcher.answers(tests).map_err(Error::Selectors)
    }
    /// The cascade of an element whose matched rules are known from before
    /// (its inline style changed, nothing selectors see): no matching.
    #[allow(clippy::too_many_arguments)]
    pub fn cascade_from_matched<'a, T: selectors::Tree>(
        &'a self,
        tree: &T,
        node: T::Node,
        matched: &[(u32, selectors::Specificity)],
        inline: Option<&'a [p::Declaration]>,
        hints: Option<&'a [p::Declaration]>,
        parent: Option<&'a dyn Inherited>,
        limits: Limits,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Cascade<'a>> {
        check(cancelled)?;
        let has_parent =
            if let Some(parent) = tree.parent(node).map_err(Error::Selectors)? { matches!(tree.content(parent).map_err(Error::Selectors)?, selectors::Content::Element(_)) } else { false };
        let mut builder = Builder::new(limits, cancelled);
        let scopes = tree.scopes(node).map_err(Error::Selectors)?;
        for &(id, specificity) in matched {
            let rule = self.rules.get(id as usize).ok_or(Error::Invalid)?;
            let encapsulation = rule.context(&scopes).unwrap_or(1);
            for (declaration, d) in rule.declarations.iter().enumerate() {
                builder.add(
                    d,
                    Priority { origin: rule.origin, encapsulation, layer: rule.layer, specificity, order: rule.order, declaration: declaration as u32, rule: u64::from(id) + 1, ..Default::default() },
                )?;
            }
        }
        if let Some(inline) = inline {
            for (i, d) in inline.iter().enumerate() {
                builder.add(d, Priority { inline: true, order: u64::MAX, rule: u64::MAX, declaration: i as u32, ..Default::default() })?;
            }
        }
        if let Some(hints) = hints {
            for (i, d) in hints.iter().enumerate() {
                builder.add(d, Priority { origin: Origin::PresentationalHint, rule: u64::MAX - 1, declaration: i as u32, ..Default::default() })?;
            }
        }
        // A shadow tree's top elements inherit from its host (the composed
        // parent, which the caller passes), their DOM parent a fragment.
        let fragment_child =
            parent.is_some() && matches!(tree.parent(node).map_err(Error::Selectors)?.map(|p| tree.content(p)).transpose().map_err(Error::Selectors)?, Some(selectors::Content::Fragment));
        builder.finish(has_parent || fragment_child, parent)
    }
    /// The rules that could match `node` (by its tree scopes, name, id,
    /// classes and attributes), into `scratch.candidates`.
    #[allow(clippy::too_many_arguments)]
    fn collect_candidates<T: selectors::Tree>(
        &self,
        tree: &T,
        node: T::Node,
        name: &selectors::ElementName<'_>,
        options: selectors::MatchOptions<'_, T::Node>,
        limits: Limits,
        scratch: &mut MatchScratch,
        cancelled: Option<&AtomicBool>,
    ) -> Result<()> {
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
            static EMPTY: std::sync::LazyLock<Scoped> = std::sync::LazyLock::new(Scoped::default);
            let scoped = match options.pseudo {
                Some(pseudo) if options.content_only => self.index.pseudo_content.get(&pseudo).unwrap_or(&EMPTY),
                Some(pseudo) => self.index.pseudo.get(&pseudo).unwrap_or(&EMPTY),
                None => &self.index.element,
            };
            // The trees whose rules can reach it (see `RuleSet::context`).
            let scopes = tree.scopes(node).map_err(Error::Selectors)?;
            let mut keys: smallvec::SmallVec<[Option<u64>; 5]> = smallvec::smallvec![UA_SCOPE, scopes[0]];
            for key in [scopes[1], scopes[2]].into_iter().flatten().map(Some).chain(scopes[0].is_some().then_some(scopes[3])) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            for buckets in keys.iter().filter_map(|k| scoped.get(k)) {
                add(Some(&buckets.universal), &mut work)?;
                add(buckets.types.get(folded(name.local_name).as_ref()), &mut work)?;
                if !buckets.root.is_empty() {
                    let parent = tree.parent(node).map_err(Error::Selectors)?;
                    let is_root = match parent {
                        Some(p) => !matches!(tree.content(p).map_err(Error::Selectors)?, selectors::Content::Element(_)),
                        None => true,
                    };
                    if is_root {
                        add(Some(&buckets.root), &mut work)?;
                    }
                }
                for a in tree.attributes(node).map_err(Error::Selectors)? {
                    check(cancelled)?;
                    work = work.saturating_add(a.value.len() + a.local_name.len() + 1);
                    if work > limits.work {
                        return Err(Error::Limit);
                    }
                    if !a.namespace.is_empty() {
                        continue;
                    }
                    if !buckets.attributes.is_empty() {
                        add(buckets.attributes.get(folded(a.local_name).as_ref()), &mut work)?;
                    }
                    if a.local_name.eq_ignore_ascii_case("id") {
                        add(buckets.ids.get(folded(a.value).as_ref()), &mut work)?;
                    } else if a.local_name.eq_ignore_ascii_case("class") {
                        for class in a.value.split([' ', '\t', '\n', '\r', '\x0c']).filter(|s| !s.is_empty()) {
                            add(buckets.classes.get(folded(class).as_ref()), &mut work)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// Whether a rule declaring `content` for `pseudo` could match `node`:
    /// when none could (most elements), the pseudo-element is not there,
    /// known without matching (MDN: 213 made of 14,282 looked for).
    #[allow(clippy::too_many_arguments)]
    pub fn may_generate<T: selectors::Tree>(
        &self,
        tree: &T,
        node: T::Node,
        pseudo: selectors::PseudoElement,
        limits: Limits,
        scratch: &mut MatchScratch,
        ancestors: Option<&selectors::AncestorFilter>,
        cancelled: Option<&AtomicBool>,
    ) -> Result<bool> {
        if !self.index.pseudo_content.contains_key(&pseudo) {
            return Ok(false);
        }
        let selectors::Content::Element(name) = tree.content(node).map_err(Error::Selectors)? else {
            return Err(Error::Invalid);
        };
        let options = selectors::MatchOptions { pseudo: Some(pseudo), content_only: true, ..Default::default() };
        self.collect_candidates(tree, node, &name, options, limits, scratch, cancelled)?;
        // A candidate needing an ancestor key the element's ancestors lack
        // (`:is(.breadcrumbs li)::after` for each of MDN's sidebar items)
        // is out at once.
        Ok(match ancestors {
            Some(filter) => scratch.candidates.iter().any(|&id| self.rules.get(id as usize).is_none_or(|rule| self.selectors.may_match(rule.selector, Some(pseudo), filter))),
            None => !scratch.candidates.is_empty(),
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn match_inner<'a, T: selectors::Tree>(
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
        ancestors: Option<&selectors::AncestorFilter>,
        cancelled: Option<&'a AtomicBool>,
        mut record: Option<&mut Vec<(u32, selectors::Specificity)>>,
    ) -> Result<Cascade<'a>> {
        check(cancelled)?;
        if options.visited_style {
            return Err(Error::Unsupported("private visited paint path"));
        }
        let selectors::Content::Element(name) = tree.content(node).map_err(Error::Selectors)? else {
            return Err(Error::Invalid);
        };
        self.collect_candidates(tree, node, &name, options, limits, scratch, cancelled)?;
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
        let mut keys = selectors::KeyCache::with_ancestors(ancestors);
        let mut matcher = self.selectors.matcher(tree, node, options, match_limits, cancelled, &mut keys).map_err(Error::Selectors)?;
        let scopes = tree.scopes(node).map_err(Error::Selectors)?;
        for &id in &scratch.candidates {
            check(cancelled)?;
            let rule = &self.rules[id as usize];
            let Some(encapsulation) = rule.context(&scopes) else { continue };
            matcher.set_rule_scope(rule.scope);
            let (matched, progress) = matcher.matches(rule.selector, match_limits.work.min(limits.work.saturating_sub(work))).map_err(Error::Selectors)?;
            if progress.steps > 40 && crate::env_on!("OLO_STYLE_TIMING") {
                eprintln!("    costly match: <{}> {} steps for {}", name.local_name, progress.steps, self.selectors.describe(rule.selector));
            }
            scratch.progress.compounds += progress.compounds;
            scratch.progress.tests += progress.tests;
            scratch.progress.steps += progress.steps;
            scratch.progress.rejected += progress.rejected;
            work = work.checked_add(progress.work).ok_or(Error::Limit)?;
            let matched = matched.filter(|_| rule.container == NO_CONTAINER || self.container_holds(rule.container, &scratch.containers, &mut scratch.decided));
            if let Some(specificity) = matched {
                if let Some(r) = record.as_deref_mut() {
                    r.push((id, specificity));
                }
                for (declaration, d) in rule.declarations.iter().enumerate() {
                    builder.add(
                        d,
                        Priority { origin: rule.origin, encapsulation, layer: rule.layer, specificity, order: rule.order, declaration: declaration as u32, rule: id as u64 + 1, ..Default::default() },
                    )?;
                }
            }
        }
        scratch.dependent = matcher.dependent();
        scratch.recorded.clear();
        scratch.recorded.extend_from_slice(matcher.recorded());
        drop(matcher);
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
        // A shadow tree's top elements inherit from its host (the composed
        // parent, which the caller passes), their DOM parent a fragment.
        let fragment_child =
            parent.is_some() && matches!(tree.parent(node).map_err(Error::Selectors)?.map(|p| tree.content(p)).transpose().map_err(Error::Selectors)?, Some(selectors::Content::Fragment));
        builder.finish(has_parent || fragment_child, parent)
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
            index_entries: self.index.element.values().map(|b| b.universal.len()).sum(),
            index_vector_bytes: self.index.element.values().map(|b| b.universal.capacity() * 4).sum(),
            index_key_value_capacity_bytes: 0,
        };
        for rule in &self.rules {
            m.declarations += rule.declarations.len();
            m.declaration_array_bytes += rule.declarations.len() * size_of::<p::Declaration>();
        }
        m.selector_reserved_bytes = self.selectors.memory().accounted_reserved_bytes;
        for buckets in self.index.element.values().chain(self.index.pseudo.values().flat_map(|s| s.values())) {
            for map in [&buckets.ids, &buckets.classes, &buckets.types] {
                m.index_key_value_capacity_bytes += map.capacity() * size_of::<(Arc<str>, Vec<u32>)>();
                for list in map.values() {
                    m.index_entries += list.len();
                    m.index_vector_bytes += list.capacity() * 4;
                }
            }
        }
        m
    }
}
/// The cascade layers `syntax` names, in the order it first names them, as
/// one `@layer a, a.b, c;` statement: what an earlier copy of a sheet
/// linked again later contributes (its rules lose to the later copy's,
/// but its place sets the layers' order). Layers named inside conditional
/// rules or anonymous layers are left to the later copy. None when it
/// names none.
pub fn layer_statement(syntax: &Syntax, cancelled: Option<&AtomicBool>) -> Option<String> {
    fn names(values: Components<'_>) -> Vec<String> {
        let mut out = Vec::new();
        let mut name = String::new();
        for c in values {
            match c.kind() {
                Kind::Whitespace => {}
                Kind::Ident => name.push_str(c.text().unwrap_or("")),
                Kind::Comma => out.push(std::mem::take(&mut name)),
                _ if c.character() == Some('.') => name.push('.'),
                _ => return Vec::new(),
            }
        }
        out.push(name);
        out.retain(|n| !n.is_empty() && !n.starts_with('.') && !n.ends_with('.') && !n.contains(".."));
        out
    }
    fn walk(rules: Vec<Statement<'_>>, prefix: &str, out: &mut Vec<String>, depth: usize, cancelled: Option<&AtomicBool>) {
        if depth > 32 {
            return;
        }
        for rule in rules {
            if rule.kind() != StatementKind::AtRule || !rule.name().is_some_and(|n| n.eq_ignore_ascii_case("layer")) {
                continue;
            }
            let named = names(rule.value());
            for n in &named {
                let full = format!("{prefix}{n}");
                if !out.contains(&full) {
                    out.push(full);
                }
            }
            if rule.block().is_some()
                && let [one] = named.as_slice()
                && let Ok(inner) = rule.block_contents(cancelled)
            {
                walk(inner, &format!("{prefix}{one}."), out, depth + 1, cancelled);
            }
        }
    }
    let mut out = Vec::new();
    walk(syntax.stylesheet(cancelled).ok()?, "", &mut out, 0, cancelled);
    (!out.is_empty()).then(|| format!("@layer {};", out.join(",")))
}
