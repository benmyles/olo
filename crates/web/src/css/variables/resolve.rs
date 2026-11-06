use super::*;
use crate::css::Component;
use std::collections::{HashMap, HashSet};
/// Stable, unique property identity within this element snapshot. Inherited
/// values are already computed by their parent's resolver, not recascaded here.
pub struct Binding<'a> {
    pub id: u64,
    pub value: &'a Value,
    pub animation_tainted: bool,
}
/// Continue a cascade rollback inside the same variable dependency guards.
pub enum Rollback<'a> {
    Binding(Binding<'a>),
    Computed(Computed),
}
pub trait Context {
    fn lookup(&self, name: &str) -> Option<Binding<'_>>;
    fn inherited(&self, _name: &str) -> Option<Computed> {
        None
    }
    /// Revert-family keywords need the actual origin/layer/rule cascade.
    fn revert(&self, _name: &str, _keyword: Wide) -> Result<Computed> {
        Err(Error::RequiresCascade)
    }
    /// Return the next cascaded binding for this property, or an already
    /// defaulted value. IDs must remain unique across all rollback candidates.
    fn rollback(&self, name: &str, _from_id: u64, keyword: Wide) -> Result<Rollback<'_>> {
        self.revert(name, keyword).map(Rollback::Computed)
    }
    /// Increment when any lookup/inheritance/taint/cascade result changes.
    /// Keep the snapshot stable for an operation. Read only on requests.
    fn revision(&self) -> u64 {
        0
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub cached_properties: usize,
    pub cache_hits: usize,
    pub computed_properties: usize,
    pub cached_arena_bytes: usize,
}
enum State {
    Active(usize),
    Done(Computed),
    Cyclic(u64, bool),
}
struct Guard {
    id: u64,
    cyclic: Option<usize>,
    animation_tainted: bool,
}
/// Worker-owned, memoized resolver for one element and immutable style
/// snapshot. No timers/threads; changed revisions discard cached results.
pub struct Resolver<'a, C> {
    context: &'a C,
    environment: Option<&'a Environment>,
    limits: Limits,
    cancelled: Option<&'a AtomicBool>,
    cache: HashMap<u64, State>,
    arenas: HashSet<usize>,
    guards: Vec<Guard>,
    revision: u64,
    work: usize,
    stats: Stats,
}
impl<'a, C: Context> Resolver<'a, C> {
    pub fn new(context: &'a C, limits: Limits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self::new_with_environment(context, None, limits, cancelled)
    }
    pub fn new_with_environment(context: &'a C, environment: Option<&'a Environment>, limits: Limits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self { context, environment, limits, cancelled, cache: HashMap::new(), arenas: HashSet::new(), guards: Vec::new(), revision: context.revision(), work: 0, stats: Stats::default() }
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.arenas.clear();
        self.guards.clear();
        self.stats = Stats::default();
    }
    pub fn stats(&self) -> Stats {
        Stats { cached_properties: self.cache.len(), ..self.stats }
    }
    fn begin(&mut self) -> Result<()> {
        check(self.cancelled)?;
        let revision = self.context.revision();
        if revision != self.revision {
            self.clear();
            self.revision = revision;
        }
        self.work = 0;
        Ok(())
    }
    fn finish(&mut self, result: Result<Computed>) -> Result<Computed> {
        if self.context.revision() != self.revision {
            self.clear();
            return Err(Error::ContextChanged);
        }
        if result.is_err() {
            self.clear();
        }
        result
    }
    /// Resolve a consumer value. Non-animatable consumers must pass false;
    /// animation-tainted references then take their var() fallbacks.
    pub fn resolve(&mut self, value: &Value, allow_animation_tainted: bool) -> Result<Computed> {
        self.begin()?;
        let result = self.sequence(value, 0, allow_animation_tainted);
        self.finish(result)
    }
    /// Compute an unregistered custom property, including CSS-wide keywords.
    pub fn property(&mut self, name: &str) -> Result<Computed> {
        self.begin()?;
        let result = self.lookup(name, 0);
        self.finish(result)
    }
    fn tick(&mut self, n: usize, depth: usize) -> Result<()> {
        check(self.cancelled)?;
        if depth > self.limits.depth.min(256) {
            return Err(Error::DepthLimit);
        }
        self.work = self.work.checked_add(n).ok_or(Error::Limit)?;
        if self.work > self.limits.work { Err(Error::Limit) } else { Ok(()) }
    }
    fn lookup(&mut self, name: &str, depth: usize) -> Result<Computed> {
        self.tick(name.len() + 1, depth)?;
        let Some(binding) = self.context.lookup(name) else {
            let value = self.context.inherited(name).unwrap_or_else(Computed::invalid);
            if value.animation_tainted {
                self.mark_taint();
            }
            return Ok(value);
        };
        self.bound(name, binding, depth)
    }
    fn bound(&mut self, name: &str, binding: Binding<'_>, depth: usize) -> Result<Computed> {
        if binding.animation_tainted {
            self.mark_taint();
        }
        if let Some(state) = self.cache.get(&binding.id) {
            return match state {
                State::Done(value) => {
                    self.stats.cache_hits += 1;
                    let value = value.clone();
                    if value.animation_tainted {
                        self.mark_taint();
                    }
                    Ok(value)
                }
                State::Active(start) => {
                    let tainted = self.guards[*start].animation_tainted;
                    self.mark_cycle(*start);
                    if tainted {
                        self.mark_taint();
                    }
                    Ok(Computed { value: None, animation_tainted: tainted })
                }
                State::Cyclic(root, tainted) => {
                    self.stats.cache_hits += 1;
                    let (mut root, mut tainted) = (*root, *tainted);
                    loop {
                        self.tick(1, depth)?;
                        match self.cache.get(&root) {
                            Some(State::Active(start)) => {
                                tainted |= self.guards[*start].animation_tainted;
                                self.mark_cycle(*start);
                                break;
                            }
                            Some(State::Cyclic(next, root_taint)) => {
                                tainted |= *root_taint;
                                if *next == root {
                                    break;
                                }
                                root = *next;
                            }
                            _ => break,
                        }
                    }
                    if tainted {
                        self.mark_taint();
                    }
                    Ok(Computed { value: None, animation_tainted: tainted })
                }
            };
        }
        if self.cache.len() >= self.limits.properties {
            return Err(Error::Limit);
        }
        self.cache.insert(binding.id, State::Active(self.guards.len()));
        self.guards.push(Guard { id: binding.id, cyclic: None, animation_tainted: binding.animation_tainted });
        let result = self.sequence(binding.value, depth + 1, true);
        // Keep the property guard active through keyword/cascade processing.
        // A lower declaration can itself reference this property or roll back
        // to an earlier binding; neither may escape dependency/cycle checks.
        let result = result.and_then(|mut value| {
            if self.guards.last().unwrap().cyclic.is_some() {
                return Ok(value);
            }
            if let Some(raw) = &value.value
                && let Some(wide) = self.checked_single(raw.components(), depth)?.and_then(wide_component)
            {
                value = match wide {
                    Wide::Initial => Computed::invalid(),
                    Wide::Inherit | Wide::Unset => self.context.inherited(name).unwrap_or_else(Computed::invalid),
                    _ => match self.context.rollback(name, binding.id, wide)? {
                        Rollback::Computed(value) => value,
                        Rollback::Binding(next) => {
                            self.tick(1, depth + 1)?;
                            if matches!(self.cache.get(&next.id), Some(State::Active(_))) {
                                // A cycle made only by rollback keywords defaults
                                // to unset; this is distinct from a var() cycle.
                                self.context.inherited(name).unwrap_or_else(Computed::invalid)
                            } else {
                                self.bound(name, next, depth + 1)?
                            }
                        }
                    },
                };
            }
            Ok(value)
        });
        let guard = self.guards.pop().unwrap();
        debug_assert_eq!(guard.id, binding.id);
        let cyclic_root = guard.cyclic.map(|start| if start < self.guards.len() { self.guards[start].id } else { guard.id });
        let result = result.map(|mut value| {
            if guard.cyclic.is_some() {
                value.value = None;
            }
            value.animation_tainted |= guard.animation_tainted;
            if value.animation_tainted {
                self.mark_taint();
            }
            value
        });
        let result = result.and_then(|value| {
            if let Some(raw) = &value.value {
                let id = Arc::as_ptr(&raw.syntax) as usize;
                if !self.arenas.contains(&id) {
                    let bytes = raw.arena_reserved_bytes();
                    if bytes > self.limits.retained_bytes.saturating_sub(self.stats.cached_arena_bytes) {
                        return Err(Error::Limit);
                    }
                    self.arenas.insert(id);
                    self.stats.cached_arena_bytes += bytes;
                }
            }
            Ok(value)
        });
        match result {
            Ok(value) => {
                self.stats.computed_properties += 1;
                self.cache.insert(binding.id, cyclic_root.map_or_else(|| State::Done(value.clone()), |root| State::Cyclic(root, value.animation_tainted)));
                Ok(value)
            }
            Err(e) => {
                self.cache.remove(&binding.id);
                Err(e)
            }
        }
    }
    fn mark_cycle(&mut self, start: usize) {
        // Every mark covers an active suffix. Its first guard therefore
        // already carries the oldest root of any overlapping cycle.
        let root = self.guards[start].cyclic.unwrap_or(start);
        if self.guards.last().is_some_and(|g| g.cyclic == Some(root)) {
            return;
        }
        for guard in &mut self.guards[root..] {
            guard.cyclic = Some(root);
        }
    }
    fn mark_taint(&mut self) {
        for guard in &mut self.guards {
            guard.animation_tainted = true;
        }
    }
    fn checked_single<'b>(&mut self, values: Components<'b>, depth: usize) -> Result<Option<Component<'b>>> {
        let mut first = None;
        for c in values {
            self.tick(1, depth)?;
            if c.kind() != Kind::Whitespace {
                if first.is_some() {
                    return Ok(None);
                }
                first = Some(c);
            }
        }
        Ok(first)
    }
    fn sequence(&mut self, value: &Value, depth: usize, allow_tainted: bool) -> Result<Computed> {
        self.tick(1, depth)?;
        if !value.needs_substitution() {
            return Ok(Computed { value: Some(value.clone()), animation_tainted: false });
        }
        self.components(value.components(), depth, allow_tainted)
    }
    fn components(&mut self, values: Components<'_>, depth: usize, allow_tainted: bool) -> Result<Computed> {
        self.tick(1, depth)?;
        let mut first = values.clone();
        if let Some(c) = first.next()
            && first.next().is_none()
            && is_reference(c)
        {
            return self.reference(c, depth, allow_tainted);
        }
        let mut out = Assembler::new(self.limits.syntax());
        let Some(tainted) = self.emit(values, &mut out, depth, allow_tainted, &mut true)? else {
            return Ok(Computed::invalid());
        };
        let end = out.len();
        Ok(Computed { value: Some(Value { syntax: Arc::new(out.finish()), start: 0, end }), animation_tainted: tainted })
    }
    fn emit(&mut self, values: Components<'_>, out: &mut Assembler, depth: usize, allow_tainted: bool, writing: &mut bool) -> Result<Option<bool>> {
        self.tick(1, depth)?;
        let mut tainted = false;
        let mut invalid = false;
        for c in values {
            self.tick(1 + (c.span().end - c.span().start) as usize, depth)?;
            if is_reference(c) {
                let value = self.reference(c, depth, allow_tainted)?;
                let Some(raw) = &value.value else {
                    invalid = true;
                    continue;
                };
                tainted |= value.animation_tainted;
                if *writing {
                    for part in raw.components() {
                        self.tick(1 + (part.span().end - part.span().start) as usize, depth)?;
                    }
                    assembled(out.copy(raw.components(), self.cancelled), writing, &mut invalid)?;
                }
            } else if c.is_container() {
                if c.kind() == Kind::Function && substitution(c.text().unwrap()) {
                    return Err(Error::Unsupported("arbitrary substitution function"));
                }
                let at = if *writing { assembled(out.open(c), writing, &mut invalid)? } else { None };
                match self.emit(c.children(), out, depth + 1, allow_tainted, writing)? {
                    Some(child_taint) => tainted |= child_taint,
                    None => invalid = true,
                }
                if let Some(at) = at
                    && *writing
                {
                    assembled(out.close(at, c.kind()), writing, &mut invalid)?;
                }
            } else {
                if *writing {
                    assembled(out.leaf(c), writing, &mut invalid)?;
                }
            }
        }
        Ok((!invalid).then_some(tainted))
    }
    fn reference(&mut self, c: Component<'_>, depth: usize, allow_tainted: bool) -> Result<Computed> {
        self.tick(1 + (c.span().end - c.span().start) as usize, depth)?;
        let args = compile::arguments(c)?;
        if c.text().unwrap().eq_ignore_ascii_case("env") {
            return self.environment_value(args, depth, allow_tainted);
        }
        let first = self.checked_single(args.first.clone(), depth)?;
        let mut result = if let Some(name) = first.and_then(name_component) {
            self.lookup(name, depth + 1)?
        } else if first.is_some_and(|c| !c.is_container()) {
            Computed::invalid()
        } else {
            let first = self.components(args.first, depth + 1, allow_tainted)?;
            if let Some(value) = &first.value
                && let Some(name) = self.checked_single(value.components(), depth)?.and_then(name_component)
            {
                self.lookup(name, depth + 1)?
            } else {
                Computed::invalid()
            }
        };
        if let Some(raw) = &result.value
            && ((raw.end & END) - raw.start) as usize > self.limits.nodes
        {
            result = Computed::invalid();
        }
        if let Some(raw) = &result.value
            && raw.syntax.source().len() > self.limits.bytes
        {
            self.tick(((raw.end & END) - raw.start) as usize, depth)?;
            if raw.source_spelling().len() > self.limits.bytes {
                result = Computed::invalid();
            }
        }
        if !allow_tainted && result.animation_tainted {
            result = Computed::invalid();
        }
        if result.is_invalid()
            && let Some(fallback) = args.fallback
        {
            result = self.components(fallback, depth + 1, allow_tainted)?;
        }
        Ok(result)
    }
    fn environment_value(&mut self, args: compile::Arguments<'_>, depth: usize, allow_tainted: bool) -> Result<Computed> {
        let first = self.components(args.first, depth + 1, allow_tainted)?;
        let Some(value) = &first.value else { return Ok(Computed::invalid()) };
        let key = match environment::key(value.components(), self.cancelled) {
            Ok(key) => key,
            Err(Error::Invalid) => return Ok(Computed::invalid()),
            Err(error) => return Err(error),
        };
        if let Some(value) = key.matchable.then(|| self.environment.and_then(|env| env.get(key.name, &key.indices))).flatten() {
            let nodes = ((value.end & END) - value.start) as usize;
            self.tick(nodes, depth)?;
            if nodes > self.limits.nodes || value.source_spelling().len() > self.limits.bytes {
                return Ok(Computed::invalid());
            }
            return Ok(Computed { value: Some(value.clone()), animation_tainted: first.animation_tainted });
        }
        if let Some(fallback) = args.fallback {
            let mut result = self.components(fallback, depth + 1, allow_tainted)?;
            result.animation_tainted |= first.animation_tainted;
            Ok(result)
        } else {
            Ok(Computed::invalid())
        }
    }
}
fn is_reference(c: Component<'_>) -> bool {
    c.kind() == Kind::Function && ["var", "env"].iter().any(|name| c.text().unwrap().eq_ignore_ascii_case(name))
}
// Output expansion limits produce guaranteed-invalid, so surrounding var()
// can use its fallback. Continue visiting remaining references for cycles;
// cancellation/work/cache quotas remain distinct operational errors.
fn assembled<T>(result: crate::css::Result<T>, writing: &mut bool, invalid: &mut bool) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(crate::css::Error::InputLimit | crate::css::Error::TokenLimit | crate::css::Error::DecodedLimit) => {
            *writing = false;
            *invalid = true;
            Ok(None)
        }
        Err(e) => Err(syntax_error(e)),
    }
}
