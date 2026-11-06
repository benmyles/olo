//! Immutable CSS :disabled/:enabled states for a connected HTML light tree.
//! Built lazily by the page worker; no DOM clone or per-control ancestor scans.
//! Selector state is distinct from control/option activation and submission.
use crate::dom::{Document, Namespace, NodeId};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub nodes: usize,
    pub controls: usize,
    pub depth: usize,
    pub work: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { nodes: 1_000_000, controls: 100_000, depth: 1024, work: 4_000_000 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Cancelled,
    Stale,
    Disconnected,
    Dom(crate::dom::Error),
}
impl From<crate::dom::Error> for Error {
    fn from(error: crate::dom::Error) -> Self {
        Self::Dom(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTML control state {self:?}")
    }
}
impl std::error::Error for Error {}
fn check(cancel: Option<&AtomicBool>) -> Result<(), Error> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn html_name(doc: &Document, node: NodeId) -> Result<Option<&str>, Error> {
    Ok(if doc.namespace(node)? == Some(Namespace::Html) { doc.name(node)? } else { None })
}
pub(crate) fn eligible(name: Option<&str>) -> bool {
    matches!(name, Some("button" | "input" | "select" | "textarea" | "fieldset" | "optgroup" | "option"))
}
struct Cursor {
    next: Option<NodeId>,
    disabled: bool,
    first_legend: Option<NodeId>,
    before_fieldset: bool,
    select: Option<bool>,
    optgroup: Option<bool>,
}
/// Generation-scoped, sparse state for native controls. `None` means the
/// element cannot match either :enabled or :disabled. Custom form-associated
/// elements need their future definition/upgrade model and are not included.
pub struct DisabledStates {
    document: u64,
    revision: u64,
    entries: Box<[(u32, bool)]>,
}
impl DisabledStates {
    pub fn new(doc: &Document, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Self, Error> {
        check(cancel)?;
        if limits.depth == 0 {
            return Err(Error::Limit);
        }
        let mut work = 0usize;
        let mut tick = || -> Result<(), Error> {
            if work.is_multiple_of(128) {
                check(cancel)?;
            }
            work = work.checked_add(1).ok_or(Error::Limit)?;
            if work > limits.work { Err(Error::Limit) } else { Ok(()) }
        };
        let mut entries = Vec::new();
        let mut stack = vec![Cursor { next: Some(doc.root()), disabled: false, first_legend: None, before_fieldset: false, select: None, optgroup: None }];
        let mut visited = 0usize;
        while let Some(cursor) = stack.last_mut() {
            tick()?;
            let Some(node) = cursor.next else {
                stack.pop();
                continue;
            };
            if visited == limits.nodes {
                return Err(Error::Limit);
            }
            visited += 1;
            cursor.next = doc.next_sibling(node)?;
            let inherited = if cursor.first_legend == Some(node) { cursor.before_fieldset } else { cursor.disabled };
            let (mut select, mut optgroup) = (cursor.select, cursor.optgroup);
            let name = html_name(doc, node)?;
            let own_disabled = eligible(name) && doc.attribute(node, "disabled")?.is_some();
            if eligible(name) {
                let disabled = match name {
                    Some("optgroup") => own_disabled || optgroup.is_none() && select == Some(true),
                    Some("option") => own_disabled || optgroup == Some(true) || select == Some(true),
                    _ => own_disabled || inherited,
                };
                if entries.len() == limits.controls {
                    return Err(Error::Limit);
                }
                entries.push((node.slot(), disabled));
            }
            // Related-select association stops at options, hr/datalist and
            // doubly nested optgroups. CSS option/group state includes the
            // owning select, unlike their intrinsic disabledness algorithms.
            match name {
                Some("select") => {
                    select = Some(own_disabled || inherited);
                    optgroup = None;
                }
                Some("optgroup") => {
                    if optgroup.is_some() {
                        select = None;
                    }
                    optgroup = Some(own_disabled);
                }
                Some("option" | "hr" | "datalist") => {
                    select = None;
                    optgroup = None;
                }
                _ => {}
            }
            let Some(child) = doc.first_child(node)? else { continue };
            if stack.len() >= limits.depth {
                return Err(Error::Limit);
            }
            let fieldset = name == Some("fieldset") && own_disabled;
            let mut first_legend = None;
            if fieldset {
                // Direct children are inspected once for this fieldset. The
                // exemption removes this fieldset's contribution only; an
                // outer disabled fieldset still disables nested controls.
                for child in doc.children(node)? {
                    tick()?;
                    if html_name(doc, child)? == Some("legend") {
                        first_legend = Some(child);
                        break;
                    }
                }
            }
            stack.push(Cursor { next: Some(child), disabled: inherited || fieldset, first_legend, before_fieldset: inherited, select, optgroup });
        }
        check(cancel)?;
        // Allocation order need not be tree order (e.g. foster parenting).
        entries.sort_unstable_by_key(|e| e.0);
        Ok(Self { document: doc.id(), revision: doc.revision(), entries: entries.into_boxed_slice() })
    }
    pub fn disabled(&self, doc: &Document, node: NodeId) -> Result<Option<bool>, Error> {
        if doc.id() != self.document || doc.revision() != self.revision {
            return Err(Error::Stale);
        }
        doc.node_type(node)?; // Reject foreign/stale handles, even for false states.
        if !eligible(html_name(doc, node)?) {
            return Ok(None);
        }
        let index = self.entries.binary_search_by_key(&node.slot(), |e| e.0).map_err(|_| Error::Disconnected)?;
        Ok(Some(self.entries[index].1))
    }
    pub fn control_count(&self) -> usize {
        self.entries.len()
    }
    /// Sparse entries only; excludes allocator headers and construction stack.
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of_val(self.entries.as_ref())
    }
}
