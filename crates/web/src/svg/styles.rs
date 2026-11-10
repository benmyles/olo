//! Styles read by a compiled SVG, including paint servers outside its root.
use crate::{
    css::style::{Style, StyleTree},
    dom::NodeId,
};
use std::cell::RefCell;

pub(super) struct Styles<'a> {
    tree: &'a StyleTree,
    reads: RefCell<Vec<NodeId>>,
}
impl<'a> Styles<'a> {
    pub fn new(tree: &'a StyleTree) -> Self {
        Self { tree, reads: RefCell::default() }
    }
    pub fn snapshot(&self, node: NodeId) -> Option<&'a Style> {
        self.reads.borrow_mut().push(node);
        self.tree.snapshot(node)
    }
    pub fn checkpoint(&self) -> usize {
        self.reads.borrow().len()
    }
    pub fn since(&self, checkpoint: usize) -> Vec<NodeId> {
        self.reads.borrow()[checkpoint..].to_vec()
    }
    pub fn include(&self, nodes: &[NodeId]) {
        self.reads.borrow_mut().extend_from_slice(nodes);
    }
    pub fn finish(self) -> Vec<NodeId> {
        let mut reads = self.reads.into_inner();
        reads.sort_unstable_by_key(|node| (node.document(), node.slot(), node.generation()));
        reads.dedup();
        reads
    }
}
