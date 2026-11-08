//! Immutable input state for a single inert document. Other HTML control,
//! media, custom-element and semantic-direction states remain explicit errors.
use crate::{
    css::selectors::{self, PseudoElement, State, StateSource},
    dom::{Document, Namespace, NodeId},
    html::controls,
};
use std::{cell::OnceCell, sync::atomic::AtomicBool};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interaction {
    pub hover: Option<NodeId>,
    pub active: Option<NodeId>,
    pub focus: Option<NodeId>,
    pub keyboard_focus: bool,
    pub target: Option<NodeId>,
}
impl Interaction {
    pub fn changes_style(self, other: Self, mask: u64) -> bool {
        let uses = |states: &[State]| states.iter().any(|s| mask & (1u64 << *s as u8) != 0);
        self.hover != other.hover && uses(&[State::Hover])
            || self.active != other.active && uses(&[State::Active])
            || self.focus != other.focus && uses(&[State::Focus, State::FocusWithin, State::FocusVisible])
            || self.keyboard_focus != other.keyboard_focus && uses(&[State::FocusVisible])
            || self.target != other.target && uses(&[State::Target])
    }
}
impl Interaction {
    /// The elements whose interaction states differ between `self` (as
    /// styled) and `now`, each with the key of a state that flipped on it
    /// (`State::key`): the hovered and pressed elements' ancestors that
    /// gained or lost the state, the focused ones (and their ancestors,
    /// for `:focus-within`), the targets. Only states some rule tests
    /// (`mask`).
    pub fn changed_elements(self, now: Self, doc: &Document, mask: u64) -> Vec<(NodeId, &'static str)> {
        let uses = |s: State| mask & (1u64 << s as u8) != 0;
        let chain = |n: Option<NodeId>| -> Vec<NodeId> { std::iter::successors(n, |&n| doc.parent(n).ok().flatten()).collect() };
        let mut out = Vec::new();
        let mut differ = |a: Option<NodeId>, b: Option<NodeId>, state: State, out: &mut Vec<(NodeId, &'static str)>| {
            if a == b || !uses(state) {
                return;
            }
            let (x, y) = (chain(a), chain(b));
            for n in x.iter().filter(|n| !y.contains(n)).chain(y.iter().filter(|n| !x.contains(n))) {
                out.push((*n, state.key()));
            }
        };
        differ(self.hover, now.hover, State::Hover, &mut out);
        differ(self.active, now.active, State::Active, &mut out);
        differ(self.focus, now.focus, State::FocusWithin, &mut out);
        let one = |a: Option<NodeId>, b: Option<NodeId>, state: State, out: &mut Vec<(NodeId, &'static str)>| {
            if a != b && uses(state) {
                out.extend(a.into_iter().chain(b).map(|n| (n, state.key())));
            }
        };
        one(self.focus, now.focus, State::Focus, &mut out);
        if self.focus != now.focus || self.keyboard_focus != now.keyboard_focus {
            if uses(State::FocusVisible) {
                out.extend(self.focus.into_iter().chain(now.focus).map(|n| (n, State::FocusVisible.key())));
            }
        }
        one(self.target, now.target, State::Target, &mut out);
        out
    }
}
pub(super) struct Snapshot<'a> {
    pub doc: &'a Document,
    pub input: Interaction,
    cancel: Option<&'a AtomicBool>,
    forms: OnceCell<Result<controls::DisabledStates, controls::Error>>,
}
impl<'a> Snapshot<'a> {
    pub fn new(doc: &'a Document, input: Interaction, cancel: Option<&'a AtomicBool>) -> Self {
        Self { doc, input, cancel, forms: OnceCell::new() }
    }
    fn disabled(&self, node: NodeId) -> selectors::Result<Option<bool>> {
        if self.doc.namespace(node)? != Some(Namespace::Html) || !controls::eligible(self.doc.name(node)?) {
            return Ok(None);
        }
        let map = |error| match error {
            controls::Error::Limit => selectors::Error::MatchLimit,
            controls::Error::Cancelled => selectors::Error::Cancelled,
            controls::Error::Dom(error) => selectors::Error::Dom(error),
            _ => selectors::Error::InvalidNode,
        };
        let forms = self.forms.get_or_init(|| controls::DisabledStates::new(self.doc, Default::default(), self.cancel)).as_ref().map_err(|e| map(*e))?;
        match forms.disabled(self.doc, node) {
            // A control the forms' walk didn't see (in a shadow tree, MDN's)
            // or one since changed: disabled by its own attribute.
            Err(controls::Error::Disconnected | controls::Error::Stale) => Ok(Some(self.doc.attribute(node, "disabled")?.is_some())),
            other => other.map_err(map),
        }
    }
    fn within(&self, mut subject: Option<NodeId>, ancestor: NodeId) -> selectors::Result<bool> {
        while let Some(node) = subject {
            if node == ancestor {
                return Ok(true);
            }
            subject = self.doc.parent(node)?;
        }
        Ok(false)
    }
}
impl StateSource for Snapshot<'_> {
    fn state(&self, node: NodeId, pseudo: Option<PseudoElement>, state: State) -> selectors::Result<bool> {
        self.doc.node_type(node)?; // Reject foreign/stale identities even for false states.
        if pseudo.is_some() {
            // Pseudo-element states (::before:hover) never match until pseudo-elements exist.
            return Ok(false);
        }
        Ok(match state {
            State::AnyLink | State::Link => self.doc.namespace(node)? == Some(Namespace::Html) && matches!(self.doc.name(node)?, Some("a" | "area")) && self.doc.attribute(node, "href")?.is_some(),
            // This initial session does not expose browsing history to CSS.
            State::Visited => false,
            State::Hover => self.within(self.input.hover, node)?,
            State::Active => self.within(self.input.active, node)?,
            State::Focus => self.input.focus == Some(node),
            State::FocusWithin => self.within(self.input.focus, node)?,
            State::FocusVisible => self.input.keyboard_focus && self.input.focus == Some(node),
            State::Target => self.input.target == Some(node),
            State::Disabled => self.disabled(node)? == Some(true),
            State::Enabled => self.disabled(node)? == Some(false),
            // A checkbox or radio ticked now, an option selected.
            State::Checked => match self.doc.name(node)? {
                Some("input") => {
                    let kind = self.doc.attribute(node, "type")?.unwrap_or("").trim().to_ascii_lowercase();
                    (kind == "checkbox" || kind == "radio") && self.doc.control_checked(node)?
                }
                Some("option") => self.doc.attribute(node, "selected")?.is_some(),
                _ => false,
            },
            // A field with a placeholder and nothing in it.
            State::PlaceholderShown => matches!(self.doc.name(node)?, Some("input" | "textarea")) && self.doc.attribute(node, "placeholder")?.is_some() && self.doc.control_value(node)?.is_empty(),
            // Elements are defined, bar custom elements (their registry is
            // the scripts'; an undefined one is what `:not(:defined)` finds).
            State::Defined => self.doc.is_defined(node)?,
            // States the engine does not track yet (:invalid, …) do not
            // match, as in a document where nothing has that state.
            _ => false,
        })
    }
    fn direction(&self, node: NodeId) -> selectors::Result<bool> {
        selectors::dir_attribute(self.doc, node)
    }
}
