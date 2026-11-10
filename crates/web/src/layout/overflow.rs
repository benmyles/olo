//! The viewport's CSS overflow source, shared by paint and measurements.
use crate::{
    css::{
        properties::{Display, Keyword as K, Property as P},
        style::{Style, StyleTree, Value},
    },
    dom::{Document, Namespace, NodeId, NodeType},
};

/// CSS Overflow 3 §3.1.4: only the root, or the first HTML body when the
/// root's overflow is visible on both axes, propagates to the viewport.
/// The source's used overflow is visible; it does not clip its descendants
/// to its own box. Scrollbar width/gutter propagate from the root alone.
/// CSS containment and allocating scrollbar gutters are not implemented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportOverflow {
    pub source: Option<NodeId>,
    pub axes: [K; 2],
    pub scrollbar_width: K,
    pub scrollbar_gutter: K,
}

/// Node identities, not computed values: patches to a retained style tree
/// must change overflow propagation immediately. Found only on box-tree
/// construction; there is no document walk during paint or measurement.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ViewportElements {
    root: Option<NodeId>,
    body: Option<NodeId>,
}
impl ViewportElements {
    pub(super) fn rtl(self, styles: &StyleTree) -> bool {
        // HTML propagates the body's direction to the viewport independently
        // of which element supplies overflow (CSS Writing Modes).
        self.body
            .and_then(|n| styles.snapshot(n))
            .filter(|s| !matches!(s.value(P::Display), Value::Display(Display::None)))
            .or_else(|| self.root_style(styles))
            .is_some_and(|s| s.axes().direction == crate::css::style::Direction::Rtl)
    }
    pub(super) fn root_style(self, styles: &StyleTree) -> Option<&Style> {
        self.root.and_then(|n| styles.snapshot(n))
    }
    pub(super) fn new(doc: &Document) -> Self {
        let root = doc.children(doc.root()).ok().and_then(|mut children| children.find(|&n| doc.node_type(n).ok() == Some(NodeType::Element)));
        let body = root
            .filter(|&r| doc.namespace(r).ok().flatten() == Some(Namespace::Html) && doc.name(r).ok().flatten() == Some("html"))
            .and_then(|r| doc.children(r).ok())
            .and_then(|mut children| children.find(|&n| doc.namespace(n).ok().flatten() == Some(Namespace::Html) && doc.name(n).ok().flatten() == Some("body")));
        Self { root, body }
    }
    pub(super) fn resolve(self, styles: &StyleTree) -> ViewportOverflow {
        let keyword = |s: &Style, p| match s.value(p) {
            Value::Keyword(k) => *k,
            _ => K::Auto,
        };
        let overflow = |s: &Style| [keyword(s, P::OverflowX), keyword(s, P::OverflowY)];
        let root = self.root.and_then(|n| styles.snapshot(n)).filter(|s| !matches!(s.value(P::Display), Value::Display(Display::None)));
        let mut result = ViewportOverflow { source: None, axes: [K::Auto; 2], scrollbar_width: K::Auto, scrollbar_gutter: K::Auto };
        if let Some(s) = root {
            result.source = self.root;
            result.axes = overflow(s);
            result.scrollbar_width = keyword(s, P::ScrollbarWidth);
            result.scrollbar_gutter = keyword(s, P::ScrollbarGutter);
            if result.axes == [K::Visible; 2]
                && let Some(body) = self.body.and_then(|n| styles.snapshot(n)).filter(|s| !matches!(s.value(P::Display), Value::Display(Display::None)))
            {
                result.source = self.body;
                result.axes = overflow(body);
            }
            result.axes = result.axes.map(|k| match k {
                K::Visible => K::Auto,
                K::Clip => K::Hidden,
                k => k,
            });
        }
        result
    }
}
