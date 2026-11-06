//! Original stylesheet cascade. Defaulted declarations are not computed/used
//! font or layout values. All compilation/matching/resolution runs on workers.
mod axes;
mod priority;
pub use axes::{Axes, Direction, TextOrientation, WritingMode};
mod sheets;
use super::{properties as p, selectors, variables};
pub use priority::{Origin, Priority};
pub use sheets::{CompileLimits, Issue, MatchScratch, Memory, RuleSet, SheetBuilder};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub candidates: usize,
    pub work: usize,
    pub depth: usize,
    pub typed_retained_bytes: usize,
    pub properties: p::Limits,
    pub variables: variables::Limits,
}
impl Default for Limits {
    fn default() -> Self {
        Self { candidates: 65536, work: 4_000_000, depth: 128, typed_retained_bytes: 16 * 1024 * 1024, properties: Default::default(), variables: Default::default() }
    }
}
#[derive(Debug)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    Limit,
    Cancelled,
    Selectors(selectors::Error),
    Property(p::Error),
    Variables(variables::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS cascade {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
/// A defaulted declaration. Inheritance must copy the parent's computed value,
/// never re-evaluate an inherited specified expression in the child's context.
#[derive(Clone, Debug)]
pub enum Defaulted {
    Specified(p::Value),
    Initial,
    Inherit,
}
/// Paired cascade result. For Inherit, read `source` on the parent using
/// the parent's axes, then copy that already-computed value. A logical source
/// must not be replaced by its physical counterpart in the child's axes.
#[derive(Clone, Debug)]
pub struct Projected {
    pub value: Defaulted,
    pub source: p::Property,
}
trait Output {
    const PROJECT: bool;
    fn finish(value: Defaulted, source: p::Property) -> Self;
}
impl Output for Defaulted {
    const PROJECT: bool = false;
    fn finish(value: Defaulted, _: p::Property) -> Self {
        value
    }
}
impl Output for Projected {
    const PROJECT: bool = true;
    fn finish(value: Defaulted, source: p::Property) -> Self {
        Self { value, source }
    }
}
#[derive(Clone, Copy)]
enum Data<'a> {
    Known(p::Property, &'a p::Value),
    Custom(&'a str, &'a variables::Value),
}
#[derive(Clone, Copy)]
struct Entry<'a> {
    data: Data<'a>,
    priority: Priority,
    animation_tainted: bool,
}
/// Already computed, inherited custom properties from a coherent parent style.
/// Host callbacks must be cheap and immutable for this snapshot.
pub trait Inherited {
    fn custom(&self, name: &str) -> Option<variables::Computed>;
}
#[derive(Clone, Copy)]
struct KnownRange {
    property: p::Property,
    start: u32,
    end: u32,
}
/// Frozen candidates for one element. Typed programs/token arenas remain shared
/// with their stylesheet. A snapshot belongs to one node/tree/style generation.
pub struct Cascade<'a> {
    entries: Vec<Entry<'a>>,
    known: smallvec::SmallVec<[KnownRange; 8]>,
    custom: BTreeMap<&'a str, std::ops::Range<usize>>,
    parent: Option<&'a dyn Inherited>,
    has_parent: bool,
    limits: Limits,
    cancelled: Option<&'a AtomicBool>,
    work: std::cell::Cell<usize>,
}
/// Build from matched rules (or explicit host-origin/animation candidates).
/// Rule IDs are unique within this element; source/declaration order is global.
pub struct Builder<'a> {
    entries: Vec<Entry<'a>>,
    limits: Limits,
    cancelled: Option<&'a AtomicBool>,
}
impl<'a> Builder<'a> {
    pub fn new(limits: Limits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self { entries: Vec::new(), limits, cancelled }
    }
    pub fn add(&mut self, declaration: &'a p::Declaration, priority: Priority) -> Result<()> {
        self.add_tainted(declaration, priority, false)
    }
    pub fn add_tainted(&mut self, declaration: &'a p::Declaration, mut priority: Priority, animation_tainted: bool) -> Result<()> {
        check(self.cancelled)?;
        let animation_tainted = animation_tainted || priority.origin == Origin::Animation;
        priority.important = declaration.important();
        priority.validate()?;
        let count = match declaration {
            p::Declaration::Known { values, .. } => values.len(),
            p::Declaration::Custom { .. } => 1,
        };
        if count > self.limits.candidates.min(u32::MAX as usize).saturating_sub(self.entries.len()) {
            return Err(Error::Limit);
        }
        match declaration {
            p::Declaration::Known { values, .. } => {
                for a in values {
                    self.entries.push(Entry { data: Data::Known(a.property, &a.value), priority, animation_tainted });
                }
            }
            p::Declaration::Custom { name, value, .. } => self.entries.push(Entry { data: Data::Custom(name, value), priority, animation_tainted }),
        }
        Ok(())
    }
    pub fn finish(mut self, has_parent: bool, parent: Option<&'a dyn Inherited>) -> Result<Cascade<'a>> {
        check(self.cancelled)?;
        // Bottom-up merging checks cancellation/work between comparisons;
        // no partially sorted/published element state can escape on failure.
        let mut work = 0usize;
        let mut spare = if self.entries.len() > 1 { self.entries.clone() } else { Vec::new() };
        let mut width = 1usize;
        while width < self.entries.len() {
            let mut start = 0;
            while start < self.entries.len() {
                let mid = start.saturating_add(width).min(self.entries.len());
                let end = mid.saturating_add(width).min(self.entries.len());
                let (mut left, mut right) = (start, mid);
                for slot in &mut spare[start..end] {
                    check(self.cancelled)?;
                    let cost = if left < mid && right < end { 1 + group(self.entries[left].data).1.len().min(group(self.entries[right].data).1.len()) } else { 1 };
                    work = work.checked_add(cost).ok_or(Error::Limit)?;
                    if work > self.limits.work {
                        return Err(Error::Limit);
                    }
                    let take_left = right == end || left < mid && entry_cmp(&self.entries[left], &self.entries[right]) != std::cmp::Ordering::Greater;
                    if take_left {
                        *slot = self.entries[left];
                        left += 1;
                    } else {
                        *slot = self.entries[right];
                        right += 1;
                    }
                }
                start = end;
            }
            std::mem::swap(&mut self.entries, &mut spare);
            width = width.saturating_mul(2);
        }
        let mut known = smallvec::SmallVec::new();
        let mut custom = BTreeMap::new();
        let mut start = 0;
        while start < self.entries.len() {
            check(self.cancelled)?;
            let mut end = start + 1;
            while end < self.entries.len() && group(self.entries[end].data) == group(self.entries[start].data) {
                end += 1;
            }
            match self.entries[start].data {
                Data::Known(property, _) => known.push(KnownRange { property, start: start as u32, end: end as u32 }),
                Data::Custom(name, _) => {
                    custom.insert(name, start..end);
                }
            }
            start = end;
        }
        Ok(Cascade { entries: self.entries, known, custom, parent, has_parent, limits: self.limits, cancelled: self.cancelled, work: std::cell::Cell::new(0) })
    }
}
fn entry_cmp(a: &Entry<'_>, b: &Entry<'_>) -> std::cmp::Ordering {
    group(a.data).cmp(&group(b.data)).then_with(|| b.priority.cmp(&a.priority))
}
fn group(data: Data<'_>) -> (u16, &str) {
    match data {
        Data::Known(p, _) => (p as u16, ""),
        Data::Custom(name, _) => (u16::MAX, name),
    }
}
impl<'a> Cascade<'a> {
    fn known_range(&self, property: p::Property) -> std::ops::Range<usize> {
        self.known.binary_search_by_key(&(property as u16), |r| r.property as u16).ok().map_or(0..0, |i| self.known[i].start as usize..self.known[i].end as usize)
    }
    fn tick(&self) -> Result<()> {
        check(self.cancelled)?;
        let n = self.work.get().saturating_add(1);
        self.work.set(n);
        if n > self.limits.work { Err(Error::Limit) } else { Ok(()) }
    }
    pub fn resolver(&'a self) -> Resolver<'a> {
        Resolver { cascade: self, variables: variables::Resolver::new(self, self.limits.variables, self.cancelled), pending: std::collections::HashMap::new(), pending_bytes: 0 }
    }
    pub fn candidate_count(&self) -> usize {
        self.entries.len()
    }
    pub fn has_parent(&self) -> bool {
        self.has_parent
    }
    pub fn declared_properties(&self) -> impl Iterator<Item = p::Property> + '_ {
        self.known.iter().map(|r| r.property)
    }
    /// Candidate/range payload capacities; borrowed typed programs and
    /// BTreeMap/control/allocator bookkeeping are excluded.
    pub fn reserved_payload_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry<'_>>() + self.known.capacity() * size_of::<KnownRange>()
    }

    pub fn custom_names(&self) -> impl Iterator<Item = &str> {
        self.custom.keys().copied()
    }
    fn range(&self, index: usize) -> std::ops::Range<usize> {
        match self.entries[index].data {
            Data::Known(p, _) => self.known_range(p),
            Data::Custom(name, _) => self.custom[name].clone(),
        }
    }
    fn rollback(&self, from: usize, wide: variables::Wide) -> Result<Option<usize>> {
        let priority = self.entries[from].priority;
        if priority.encapsulation != 0 {
            return Err(Error::Unsupported("encapsulation rollback host"));
        }
        for i in self.range(from) {
            self.tick()?;
            if self.entries[i].priority.eligible_after(priority, wide)? {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
    fn highest(&self, physical: Option<usize>, logical: Option<usize>) -> Option<usize> {
        match (physical, logical) {
            // A declaration such as all expands both. Physical longhands win
            // equal priority, as required for all: inherit across changed axes.
            (Some(p), Some(l)) if self.entries[l].priority > self.entries[p].priority => Some(l),
            (Some(p), _) => Some(p),
            (None, l) => l,
        }
    }
    fn rollback_pair(&self, from: usize, wide: variables::Wide, physical: std::ops::Range<usize>, logical: std::ops::Range<usize>) -> Result<Option<usize>> {
        let priority = self.entries[from].priority;
        if priority.encapsulation != 0 {
            return Err(Error::Unsupported("encapsulation rollback host"));
        }
        let mut candidates = [None; 2];
        for (slot, range) in candidates.iter_mut().zip([physical, logical]) {
            for i in range {
                self.tick()?;
                if self.entries[i].priority.eligible_after(priority, wide)? {
                    *slot = Some(i);
                    break;
                }
            }
        }
        Ok(self.highest(candidates[0], candidates[1]))
    }
    fn defaulted(&self, property: p::Property, wide: variables::Wide) -> Defaulted {
        if self.has_parent && (wide == variables::Wide::Inherit || wide == variables::Wide::Unset && property.inherited()) { Defaulted::Inherit } else { Defaulted::Initial }
    }
}
/// Per-element worker resolution. Retains variable results and shared typed
/// shorthand expansion; query only changed properties, then retain computed
/// host results. No clock, polling or per-frame work is performed here.
pub struct Resolver<'a> {
    cascade: &'a Cascade<'a>,
    variables: variables::Resolver<'a, Cascade<'a>>,
    pending: std::collections::HashMap<(usize, bool), p::Assignments>,
    pending_bytes: usize,
}
impl Resolver<'_> {
    pub fn property(&mut self, property: p::Property, allow_animation_tainted: bool) -> Result<Defaulted> {
        self.resolve::<Defaulted>(property, Axes::default(), allow_animation_tainted)
    }
    /// Cascade physical/logical equivalents together after resolving axes.
    /// Both aliases share a result. Retain `source` for correct inheritance.
    pub fn projected(&mut self, property: p::Property, axes: Axes, allow_animation_tainted: bool) -> Result<Projected> {
        self.resolve::<Projected>(property, axes, allow_animation_tainted)
    }
    fn resolve<O: Output>(&mut self, property: p::Property, axes: Axes, allow_animation_tainted: bool) -> Result<O> {
        self.cascade.work.set(0);
        self.cascade.tick()?;
        let pair = if O::PROJECT { axes.pair(property) } else { None };
        let primary = pair.map_or(property, |p| p[0]);
        let range = self.cascade.known_range(primary);
        let other = pair.map_or(0..0, |p| self.cascade.known_range(p[1]));
        let mut at = self.cascade.highest((!range.is_empty()).then_some(range.start), (!other.is_empty()).then_some(other.start));
        let mut seen = smallvec::SmallVec::<[usize; 8]>::new();
        while let Some(i) = at {
            self.cascade.tick()?;
            if seen.contains(&i) {
                return Ok(O::finish(self.cascade.defaulted(primary, variables::Wide::Unset), primary));
            }
            if seen.len() >= self.cascade.limits.depth.min(256) {
                return Err(Error::Limit);
            }
            seen.push(i);
            let Data::Known(declared, raw) = self.cascade.entries[i].data else { unreachable!() };
            let value = if let p::Value::Pending(pending) = raw {
                let key = (Arc::as_ptr(pending) as usize, allow_animation_tainted);
                if !self.pending.contains_key(&key) {
                    let values = pending.resolve(&mut self.variables, allow_animation_tainted, self.cascade.limits.properties, self.cascade.cancelled).map_err(Error::Property)?;
                    // Conservative retained accounting: shared programs may be
                    // counted more than once. Hash/allocator controls are count-
                    // bounded separately; this is not total process memory.
                    let mut bytes = size_of::<((usize, bool), p::Assignments)>();
                    if values.spilled() {
                        bytes = bytes.saturating_add(values.capacity() * size_of::<p::Assignment>());
                    }
                    for a in &values {
                        bytes = bytes.saturating_add(a.value.reserved_bytes());
                    }
                    if bytes > self.cascade.limits.typed_retained_bytes.saturating_sub(self.pending_bytes) {
                        return Err(Error::Limit);
                    }
                    self.pending_bytes += bytes;
                    self.pending.insert(key, values);
                }
                &self.pending[&key].iter().find(|a| a.property == declared).ok_or(Error::Invalid)?.value
            } else {
                raw
            };
            match value {
                p::Value::Wide(wide @ (variables::Wide::Revert | variables::Wide::RevertLayer | variables::Wide::RevertRule)) => {
                    at = if pair.is_some() { self.cascade.rollback_pair(i, *wide, range.clone(), other.clone())? } else { self.cascade.rollback(i, *wide)? };
                }
                p::Value::Wide(wide) => return Ok(O::finish(self.cascade.defaulted(declared, *wide), declared)),
                _ => return Ok(O::finish(Defaulted::Specified(value.clone()), declared)),
            }
        }
        Ok(O::finish(self.cascade.defaulted(primary, variables::Wide::Unset), primary))
    }
    pub fn custom(&mut self, name: &str) -> Result<variables::Computed> {
        self.cascade.work.set(0);
        self.cascade.tick()?;
        self.variables.property(name).map_err(Error::Variables)
    }
    pub fn pending_cached(&self) -> usize {
        self.pending.len()
    }
}
impl variables::Context for Cascade<'_> {
    fn lookup(&self, name: &str) -> Option<variables::Binding<'_>> {
        let i = self.custom.get(name)?.start;
        let Data::Custom(_, value) = self.entries[i].data else { unreachable!() };
        Some(variables::Binding { id: i as u64 + 1, value, animation_tainted: self.entries[i].animation_tainted })
    }
    fn inherited(&self, name: &str) -> Option<variables::Computed> {
        self.parent.and_then(|p| p.custom(name))
    }
    fn rollback(&self, name: &str, from_id: u64, wide: variables::Wide) -> variables::Result<variables::Rollback<'_>> {
        let i = usize::try_from(from_id.checked_sub(1).ok_or(variables::Error::RequiresCascade)?).map_err(|_| variables::Error::RequiresCascade)?;
        if i >= self.entries.len() || !matches!(self.entries[i].data,Data::Custom(n,_) if n==name) {
            return Err(variables::Error::RequiresCascade);
        }
        let next = self.rollback(i, wide).map_err(|e| match e {
            Error::Cancelled => variables::Error::Cancelled,
            Error::Limit => variables::Error::Limit,
            _ => variables::Error::RequiresCascade,
        })?;
        Ok(if let Some(i) = next {
            let Data::Custom(_, value) = self.entries[i].data else { unreachable!() };
            variables::Rollback::Binding(variables::Binding { id: i as u64 + 1, value, animation_tainted: self.entries[i].animation_tainted })
        } else {
            variables::Rollback::Computed(self.inherited(name).unwrap_or_else(variables::Computed::invalid))
        })
    }
}
