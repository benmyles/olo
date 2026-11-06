//! Immutable document-wide, already computed CSS environment values. An empty
//! value is defined; an absent name/index tuple alone selects an env fallback.
use super::*;
use smallvec::SmallVec;
use std::{collections::HashSet, sync::OnceLock};

const MAX_INDICES: usize = 16;
struct Entry {
    name: Arc<str>,
    indices: Box<[u32]>,
    value: Value,
}
pub struct Environment {
    entries: Box<[Entry]>,
    bytes: usize,
}
impl Environment {
    /// Names are decoded CSS identifiers (references can use CSS escapes).
    /// Values must be computed before publication. Rebuild the snapshot, and
    /// change the style host generation, when any document environment changes.
    pub fn new(entries: impl IntoIterator<Item = (Arc<str>, Box<[u32]>, Value)>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        check(cancelled)?;
        let mut values = Vec::new();
        let mut arenas = HashSet::new();
        let mut bytes = size_of::<Self>();
        if bytes > limits.retained_bytes {
            return Err(Error::Limit);
        }
        let mut work = 0usize;
        let mut nodes = 0usize;
        for (name, indices, value) in entries {
            check(cancelled)?;
            if values.len() >= limits.properties || indices.len() > MAX_INDICES || name.len() > limits.bytes {
                return Err(Error::Limit);
            }
            if value.needs_substitution() {
                return Err(Error::Invalid);
            }
            if !valid_name(&name) {
                return Err(Error::Invalid);
            }
            nodes = nodes.checked_add(((value.end & END) - value.start) as usize).ok_or(Error::Limit)?;
            if nodes > limits.nodes {
                return Err(Error::Limit);
            }
            let spelling = value.source_spelling();
            if spelling.len() > limits.bytes {
                return Err(Error::Limit);
            }
            work = work.checked_add(name.len() + indices.len() + spelling.len() + 1).ok_or(Error::Limit)?;
            bytes = bytes.checked_add(size_of::<Entry>() + name.len() + 2 * size_of::<usize>() + indices.len() * size_of::<u32>()).ok_or(Error::Limit)?;
            if arenas.insert(value.arena_identity()) {
                bytes = bytes.checked_add(value.arena_reserved_bytes()).ok_or(Error::Limit)?;
            }
            if nodes > limits.nodes || work > limits.work || bytes > limits.retained_bytes {
                return Err(Error::Limit);
            }
            values.push(Entry { name, indices, value });
        }
        values.sort_unstable_by(|a, b| (&a.name, &a.indices).cmp(&(&b.name, &b.indices)));
        check(cancelled)?;
        if values.windows(2).any(|v| v[0].name == v[1].name && v[0].indices == v[1].indices) {
            return Err(Error::Invalid);
        }
        Ok(Self { entries: values.into_boxed_slice(), bytes })
    }
    pub fn get(&self, name: &str, indices: &[u32]) -> Option<&Value> {
        self.entries.binary_search_by(|e| (e.name.as_ref(), e.indices.as_ref()).cmp(&(name, indices))).ok().map(|i| &self.entries[i].value)
    }
    /// Accounted payload/header bytes; shared token arenas are counted once.
    /// Allocator metadata and temporary construction storage are excluded.
    pub fn reserved_bytes(&self) -> usize {
        self.bytes
    }
    /// A rectangular native content viewport has no unsafe edges. Hardware
    /// segment variables remain undefined in this snapshot.
    pub fn rectangular() -> Result<Arc<Self>> {
        static VALUE: OnceLock<Result<Arc<Environment>>> = OnceLock::new();
        VALUE
            .get_or_init(|| {
                let syntax = Syntax::parse(Arc::from("0px"), Default::default(), None).map_err(syntax_error)?;
                let zero = Value::compile(syntax.components(), Default::default(), None)?;
                Self::new(
                    [
                        "safe-area-inset-top",
                        "safe-area-inset-right",
                        "safe-area-inset-bottom",
                        "safe-area-inset-left",
                        "safe-area-max-inset-top",
                        "safe-area-max-inset-right",
                        "safe-area-max-inset-bottom",
                        "safe-area-max-inset-left",
                    ]
                    .into_iter()
                    .map(|name| (Arc::from(name), Box::from([]), zero.clone())),
                    Default::default(),
                    None,
                )
                .map(Arc::new)
            })
            .clone()
    }
}
fn environment_name(c: super::super::Component<'_>) -> Option<&str> {
    if c.kind() != Kind::Ident {
        return None;
    }
    c.text().filter(|name| valid_name(name))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('\0') && !["default", "initial", "inherit", "unset", "revert", "revert-layer", "revert-rule"].iter().any(|reserved| name.eq_ignore_ascii_case(reserved))
}
pub(super) struct Key<'a> {
    pub name: &'a str,
    pub indices: SmallVec<[u32; 2]>,
    pub matchable: bool,
}
pub(super) fn key<'a>(values: Components<'a>, cancelled: Option<&AtomicBool>) -> Result<Key<'a>> {
    let mut values = values.filter(|c| c.kind() != Kind::Whitespace);
    let name = values.next().and_then(environment_name).ok_or(Error::Invalid)?;
    let mut key = Key { name, indices: SmallVec::new(), matchable: true };
    for c in values {
        check(cancelled)?;
        let number = c.number().filter(|_| c.kind() == Kind::Integer).ok_or(Error::Invalid)?.value;
        if number < 0. {
            return Err(Error::Invalid);
        }
        if number > f64::from(u32::MAX) || key.indices.len() >= MAX_INDICES {
            key.matchable = false;
        } else if key.matchable {
            key.indices.push(number as u32);
        }
    }
    Ok(key)
}
