//! Immutable input state for a single inert document. Other HTML control,
//! media, custom-element and semantic-direction states remain explicit errors.
use crate::{
    css::selectors::{self, PseudoElement, State, StateSource},
    dom::{Document, Namespace, NodeId},
};
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
pub(super) struct Snapshot<'a> {
    pub doc: &'a Document,
    pub input: Interaction,
}
impl Snapshot<'_> {
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
            return Err(selectors::Error::StateUnavailable);
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
            _ => return Err(selectors::Error::StateUnavailable),
        })
    }
}
