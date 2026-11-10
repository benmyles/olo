//! A window's tree: what it shows, kept.
//!
//! BeOS kept a window's `BView`s in a tree and asked each to draw when
//! app_server said part of it was to be shown again. Olo keeps a tree of
//! nodes, each with where it's laid out and what it last drew. A view's
//! `body` describes what it shows ([`crate::Element`]s); the tree is
//! brought in line with that (`reconcile`): a node that's the same keeps
//! everything, one whose look changed is drawn again, one whose size
//! could change is laid out again, and nothing else is touched.

use std::{any::Any, sync::Arc};

use refineable::Refineable as _;
use slotmap::new_key_type;
use smallvec::SmallVec;

use crate::{
    AvailableSpace, Bounds, ContentMask, ControlState, Element, ElementId, Overflow, Pixels, Point, SharedString, Size, Style, StyleRefinement, TextAlign, TextOverflow, TextRun, TextStyle,
    TextStyleRefinement, WhiteSpace, WindowTextSystem, WrappedLine,
    element::{CanvasSpec, Interactivity, Kind},
    layout::LayoutId,
    painter::{Gfx, Notes, Painter},
    point, px,
    stage::Recording,
    window::WindowCore,
};

new_key_type! {
    /// A node of a window's tree.
    pub struct NodeId;
    /// A view in a window.
    pub struct ViewId;
}

/// What a leaf that sizes itself by what it holds is told.
pub struct Measuring<'a> {
    pub text_system: &'a Arc<WindowTextSystem>,
    pub rem_size: Pixels,
    pub scale_factor: f32,
    /// The text style in force where it is.
    pub text_style: &'a TextStyle,
}

pub(crate) struct TextNode {
    pub(crate) text: SharedString,
    pub(crate) runs: Option<Vec<TextRun>>,
    pub(crate) laid: Option<TextLaid>,
}

/// A text as it was last measured: shaped and wrapped.
pub(crate) struct TextLaid {
    pub(crate) lines: SmallVec<[WrappedLine; 1]>,
    pub(crate) line_height: Pixels,
    pub(crate) wrap_width: Option<Pixels>,
    pub(crate) size: Size<Pixels>,
    /// It doesn't show in full (cut short with an ellipsis).
    pub(crate) truncated: bool,
    pub(crate) align: TextAlign,
}

pub(crate) enum NodeKind {
    Group,
    Text(TextNode),
    Canvas(CanvasSpec),
}

impl NodeKind {
    fn tag(&self) -> u8 {
        match self {
            NodeKind::Group => 0,
            NodeKind::Text(_) => 1,
            NodeKind::Canvas(_) => 2,
        }
    }
}

pub(crate) struct Node {
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    /// The view whose `body` made it: its handlers and its drawing are
    /// that view's.
    pub(crate) owner: ViewId,
    /// It's where this view's `body` goes.
    pub(crate) view_root: Option<ViewId>,
    pub(crate) key: Option<ElementId>,
    pub(crate) kind: NodeKind,
    /// Its style as `body` gave it, and as it is now (with what the
    /// pointer and keyboard add).
    pub(crate) refinement: StyleRefinement,
    pub(crate) style: Style,
    pub(crate) layout: LayoutId,
    pub(crate) inter: Option<Box<Interactivity>>,
    /// Where it is in the window.
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) content_size: Size<Pixels>,
    /// What it and everything it holds reach (a child may lie outside
    /// its parent): where the pointer can't be over any of them, none is
    /// looked at.
    pub(crate) reach: Bounds<Pixels>,
    pub(crate) hovered: bool,
    pub(crate) pressed: bool,
    pub(crate) focused: bool,
    /// Its scroller, if it scrolls what it holds.
    pub(crate) scroll: Option<crate::ScrollHandle>,
    /// What it keeps for itself (`Painter::node_state`).
    pub(crate) state: Option<Box<dyn Any>>,
    /// What it holds is rows made as they come near what shows, and
    /// which of them are there now.
    pub(crate) rows: Option<(crate::element::RowsSpec, std::ops::Range<usize>)>,
    /// What it drew behind what it holds (its ground and its own
    /// content), and over it (its border), where it was when it did.
    pub(crate) behind: Option<Arc<Recording>>,
    pub(crate) over: Option<Arc<Recording>>,
    pub(crate) painted_at: Point<Pixels>,
    pub(crate) notes: Option<Box<Notes>>,
    /// It's to be drawn again.
    pub(crate) paint_dirty: bool,
    /// Where what it drew is in the window's list of what shows (behind
    /// what it holds, and over it), if it is.
    pub(crate) slots: [u32; 2],
}

impl Node {
    pub(crate) fn state(&self) -> ControlState {
        let enabled = !self.inter.as_ref().is_some_and(|i| i.disabled);
        ControlState { hovered: self.hovered && enabled, pressed: self.pressed && self.hovered && enabled, enabled, focused: self.focused }
    }

    /// Its style, from what `body` gave it and how the pointer and
    /// keyboard stand to it.
    fn computed_style(&self) -> Style {
        let mut style = Style::default();
        style.refine(&self.refinement);
        if let Some(inter) = &self.inter {
            if self.focused
                && let Some(focus) = &inter.focus_style
            {
                style.refine(focus);
            }
            if self.hovered
                && let Some(hover) = &inter.hover_style
            {
                style.refine(hover);
            }
            if self.pressed
                && self.hovered
                && let Some(active) = &inter.active_style
            {
                style.refine(active);
            }
        }
        style
    }

    /// Whether anything of it is drawn at all.
    fn paints(&self) -> bool {
        !matches!(self.kind, NodeKind::Group) || self.style.paints_behind() || self.style.paints_over()
    }
}

impl WindowCore {
    pub(crate) fn mark_paint(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.get_mut(id)
            && !node.paint_dirty
        {
            node.paint_dirty = true;
            self.paint_queue.push(id);
        }
        self.needs_commit = true;
    }

    /// What shows, or where, changed (not only what something drew): the
    /// list of what shows is made again.
    pub(crate) fn reshaped(&mut self) {
        self.needs_commit = true;
        self.items_stale = true;
    }

    /// Everything under `id` (and it) is to be drawn again; text is to
    /// be measured again too if `text` (the text style it's in changed).
    pub(crate) fn mark_subtree(&mut self, id: NodeId, text: bool) {
        let mut stack: SmallVec<[NodeId; 32]> = SmallVec::new();
        stack.push(id);
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get_mut(id) else { continue };
            stack.extend(node.children.iter().copied());
            if text && let NodeKind::Text(t) = &mut node.kind {
                t.laid = None;
                let layout = node.layout;
                self.layout.mark_dirty(layout);
            }
            if node.paints() {
                self.mark_paint(id);
            }
        }
    }

    /// A node for `element`, and nodes for all it holds.
    pub(crate) fn create(&mut self, element: Element, owner: ViewId, parent: Option<NodeId>) -> NodeId {
        let Element { kind, style: refinement, key, children, inter, rows } = element;
        let kind = self.keyed(kind, owner);
        if let Kind::View(view) = kind {
            return self.place_view(view, parent);
        }
        let kind = match kind {
            Kind::Group | Kind::View(_) => NodeKind::Group,
            Kind::Text(spec) => NodeKind::Text(TextNode { text: spec.text, runs: spec.runs, laid: None }),
            Kind::Canvas(spec) => NodeKind::Canvas(spec),
        };
        let measured = match &kind {
            NodeKind::Text(_) => true,
            NodeKind::Canvas(spec) => spec.measure.is_some(),
            NodeKind::Group => false,
        };
        let mut style = Style::default();
        style.refine(&refinement);
        let layout = self.layout.add(&style, self.gfx.rem_size, self.gfx.scale_factor, None);
        let hears = inter.as_ref().is_some_and(|i| i.hears_window());
        let id = self.nodes.insert(Node {
            parent,
            children: Vec::new(),
            owner,
            view_root: None,
            key,
            kind,
            refinement,
            style,
            layout,
            inter,
            bounds: Bounds::default(),
            content_size: Size::default(),
            reach: Bounds::default(),
            hovered: false,
            pressed: false,
            focused: false,
            scroll: None,
            state: None,
            rows: None,
            behind: None,
            over: None,
            painted_at: Point::default(),
            notes: None,
            paint_dirty: false,
            slots: [u32::MAX; 2],
        });
        if measured {
            self.layout.set_measured(layout, Some(id));
        }
        if hears {
            self.window_listeners.push(id);
        }
        self.follow_scroll(id);
        // (Rows: made once it's laid out, and what shows of it is
        // known.)
        if let Some(spec) = rows {
            self.nodes[id].rows = Some((spec, 0..0));
            self.row_nodes.push(id);
        }
        if !children.is_empty() {
            let made: Vec<NodeId> = children.into_iter().map(|child| self.create(child, owner, Some(id))).collect();
            let layouts: SmallVec<[LayoutId; 16]> = made.iter().map(|c| self.nodes[*c].layout).collect();
            self.layout.set_children(layout, &layouts);
            self.nodes[id].children = made;
        }
        self.mark_paint(id);
        id
    }

    /// The node a view's `body` goes in, under `parent`: the one it has
    /// (moved, if it was elsewhere), or a new one.
    fn place_view(&mut self, view: ViewId, parent: Option<NodeId>) -> NodeId {
        if let Some(root) = self.views.get(view).and_then(|v| v.root).filter(|root| self.nodes.contains_key(*root)) {
            let was = self.nodes[root].parent;
            if was != parent {
                if let Some(was) = was
                    && let Some(old) = self.nodes.get_mut(was)
                {
                    old.children.retain(|c| *c != root);
                    let layouts: SmallVec<[LayoutId; 16]> = old.children.clone().iter().map(|c| self.nodes[*c].layout).collect();
                    let layout = self.nodes[was].layout;
                    self.layout.set_children(layout, &layouts);
                }
                self.nodes[root].parent = parent;
                self.mark_subtree(root, true);
                // (It may have changed while it wasn't shown.)
                self.invalidate_view(view);
            }
            return root;
        }
        let style = Style::default();
        let layout = self.layout.add(&style, self.gfx.rem_size, self.gfx.scale_factor, None);
        let root = self.nodes.insert(Node {
            parent,
            children: Vec::new(),
            owner: view,
            view_root: Some(view),
            key: None,
            kind: NodeKind::Group,
            refinement: StyleRefinement::default(),
            style,
            layout,
            inter: None,
            bounds: Bounds::default(),
            content_size: Size::default(),
            reach: Bounds::default(),
            hovered: false,
            pressed: false,
            focused: false,
            scroll: None,
            state: None,
            rows: None,
            behind: None,
            over: None,
            painted_at: Point::default(),
            notes: None,
            paint_dirty: false,
            slots: [u32::MAX; 2],
        });
        if let Some(slot) = self.views.get_mut(view) {
            slot.root = Some(root);
        }
        self.invalidate_view(view);
        root
    }

    /// Takes a node and all it holds out of the tree. (A view placed in
    /// it is only let go of: it's its owner's to end.)
    pub(crate) fn destroy(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.view_root.is_some() {
            // (Not shown now; kept, with what it holds, for when it's
            // placed again.)
            self.nodes[id].parent = None;
            self.reshaped();
            return;
        }
        let Some(node) = self.nodes.remove(id) else { return };
        self.layout.remove(node.layout);
        for child in node.children {
            self.destroy(child);
        }
        self.forget(id);
        self.reshaped();
    }

    /// A node that scrolls (its style says its overflow does) has a
    /// scroller: its view's, or one of its own, kept while it scrolls.
    fn follow_scroll(&mut self, id: NodeId) {
        let node = &mut self.nodes[id];
        let scrolls = node.style.overflow.x == Overflow::Scroll || node.style.overflow.y == Overflow::Scroll;
        let was = node.scroll.as_ref().map(|h| h.id());
        let asked = node.inter.as_ref().and_then(|i| i.scroll.clone());
        node.scroll = match (scrolls, asked, node.scroll.take()) {
            (false, _, _) => None,
            (true, Some(handle), _) => Some(handle),
            (true, None, kept) => Some(kept.unwrap_or_default()),
        };
        let asked = node.scroll.as_ref().filter(|h| h.asked()).cloned();
        if node.scroll.as_ref().map(|h| h.id()) != was {
            self.reshaped();
        }
        // (Asked to go somewhere by whoever built it: once it's laid
        // out.)
        if let Some(handle) = asked {
            self.scroll_asked(handle);
        }
    }

    /// A node is gone: nothing points at it any more.
    fn forget(&mut self, id: NodeId) {
        self.row_nodes.retain(|n| *n != id);
        self.window_listeners.retain(|n| *n != id);
        self.hovered.retain(|n| *n != id);
        self.pressed.retain(|(n, _)| *n != id);
        if self.focus == Some(id) {
            self.focus = None;
        }
    }

    /// A drawing keyed by what its view says (`Element::key_by`) gets
    /// its key: asked for as the element takes its place, the view back
    /// in its own after saying what it shows.
    fn keyed(&self, kind: Kind, owner: ViewId) -> Kind {
        match kind {
            Kind::Canvas(mut spec) => {
                if let Some(key_by) = &spec.key_by
                    && let Some(view) = self.views.get(owner).and_then(|v| v.view.as_ref())
                {
                    spec.key = Some(key_by(view.as_any()));
                }
                Kind::Canvas(spec)
            }
            other => other,
        }
    }

    /// Brings a node in line with what `body` now says it is.
    pub(crate) fn reconcile(&mut self, id: NodeId, element: Element, owner: ViewId) {
        let Element { kind, style: refinement, key, children, inter, rows } = element;
        let kind = self.keyed(kind, owner);
        let (rem_size, scale_factor) = (self.gfx.rem_size, self.gfx.scale_factor);
        let mut repaint = false;
        let mut text_changed = false;
        let mut subtree = false;
        {
            let node = &mut self.nodes[id];
            node.owner = owner;
            node.key = key;
            // What it is.
            match (&mut node.kind, kind) {
                (NodeKind::Group, Kind::Group) => {}
                (NodeKind::Text(text), Kind::Text(spec)) => {
                    if text.text != spec.text || text.runs != spec.runs {
                        text.text = spec.text;
                        text.runs = spec.runs;
                        text.laid = None;
                        text_changed = true;
                        repaint = true;
                    }
                }
                (NodeKind::Canvas(canvas), Kind::Canvas(spec)) => {
                    let same = canvas.key.is_some() && canvas.key == spec.key;
                    let measured = (canvas.measure.is_some(), spec.measure.is_some());
                    *canvas = spec;
                    if measured.0 != measured.1 {
                        self.layout.set_measured(node.layout, measured.1.then_some(id));
                    }
                    if !same {
                        repaint = true;
                        text_changed = measured.1;
                    }
                }
                (_, new) => {
                    let (kind, measured) = match new {
                        Kind::Group | Kind::View(_) => (NodeKind::Group, false),
                        Kind::Text(spec) => (NodeKind::Text(TextNode { text: spec.text, runs: spec.runs, laid: None }), true),
                        Kind::Canvas(spec) => {
                            let measured = spec.measure.is_some();
                            (NodeKind::Canvas(spec), measured)
                        }
                    };
                    node.kind = kind;
                    self.layout.set_measured(node.layout, measured.then_some(id));
                    text_changed = true;
                    repaint = true;
                }
            }
            // How it takes the pointer and the keyboard.
            let state_styles_changed = match (&node.inter, &inter) {
                (Some(a), Some(b)) => a.hover_style != b.hover_style || a.active_style != b.active_style || a.focus_style != b.focus_style,
                (None, None) => false,
                (a, b) => {
                    let has = |i: &Option<Box<Interactivity>>| i.as_ref().is_some_and(|i| i.hover_style.is_some() || i.active_style.is_some() || i.focus_style.is_some());
                    has(a) || has(b)
                }
            };
            let disabled_changed = node.inter.as_ref().is_some_and(|i| i.disabled) != inter.as_ref().is_some_and(|i| i.disabled);
            let heard = node.inter.as_ref().is_some_and(|i| i.hears_window());
            let hears = inter.as_ref().is_some_and(|i| i.hears_window());
            node.inter = inter;
            if heard != hears {
                if hears {
                    self.window_listeners.push(id);
                } else {
                    self.window_listeners.retain(|n| *n != id);
                }
            }
            let node = &mut self.nodes[id];
            if disabled_changed && node.inter.as_ref().is_some_and(|i| i.paints_by_state) {
                repaint = true;
            }
            // How it looks and where it goes.
            if node.refinement != refinement || state_styles_changed {
                node.refinement = refinement;
                let style = node.computed_style();
                if style != node.style {
                    let text = style.text != node.style.text;
                    let opacity = style.opacity != node.style.opacity;
                    let clips = style.overflow != node.style.overflow || style.visibility != node.style.visibility;
                    let look = style.background != node.style.background
                        || style.border_color != node.style.border_color
                        || style.border_widths != node.style.border_widths
                        || style.corner_radii != node.style.corner_radii
                        || style.box_shadow != node.style.box_shadow
                        || style.border_style != node.style.border_style;
                    node.style = style;
                    self.layout.set_style(node.layout, &node.style, rem_size, scale_factor);
                    repaint |= look;
                    subtree |= text || opacity;
                    text_changed |= text;
                    if clips {
                        self.reshaped();
                    }
                }
            }
        }
        self.follow_scroll(id);
        if text_changed {
            let layout = self.nodes[id].layout;
            self.layout.mark_dirty(layout);
        }
        if subtree {
            self.mark_subtree(id, true);
        } else if repaint {
            self.mark_paint(id);
        }
        // Rows: the ones there now, made again (what they show may have
        // changed); which are wanted is looked at once it's laid out.
        let children = match rows {
            Some(spec) => {
                let node = &mut self.nodes[id];
                let near = match node.rows.take() {
                    Some((_, near)) => near.start.min(spec.count)..near.end.min(spec.count),
                    None => {
                        self.row_nodes.push(id);
                        0..0
                    }
                };
                let made = place_rows(&spec, near.clone());
                self.nodes[id].rows = Some((spec, near));
                made
            }
            None => {
                if self.nodes[id].rows.take().is_some() {
                    self.row_nodes.retain(|n| *n != id);
                }
                children
            }
        };
        self.reconcile_children(id, children, owner);
    }

    /// Rows are made for what can be seen now of everything that holds
    /// rows (it scrolled, or was laid out). Whether any were.
    pub(crate) fn update_rows(&mut self) -> bool {
        let mut any = false;
        for i in 0..self.row_nodes.len() {
            let id = self.row_nodes[i];
            let Some(node) = self.nodes.get(id) else { continue };
            let Some((spec, near)) = &node.rows else { continue };
            if !self.is_attached(id) {
                continue;
            }
            // The rows that can be seen (what can be seen is already
            // taken to a grid, with room to spare: `visible_at`).
            let visible =
                self.visible_at(id).intersect(&Bounds { origin: point(px(-1.0e7), node.bounds.top()), size: crate::size(px(2.0e7), node.bounds.size.height.max(spec.row_height * spec.count as f32)) });
            let h = spec.row_height.0.max(1.0);
            let wanted = if visible.size.height > px(0.) {
                let first = ((visible.top() - node.bounds.top()).0 / h).floor().max(0.0) as usize;
                let last = ((visible.bottom() - node.bounds.top()).0 / h).ceil().max(0.0) as usize;
                first.min(spec.count)..last.min(spec.count)
            } else {
                0..0
            };
            if wanted == *near {
                continue;
            }
            let (spec, owner) = (spec.clone(), node.owner);
            let made = place_rows(&spec, wanted.clone());
            self.nodes[id].rows = Some((spec, wanted));
            self.reconcile_children(id, made, owner);
            any = true;
        }
        any
    }

    fn reconcile_children(&mut self, id: NodeId, children: Vec<Element>, owner: ViewId) {
        let old = std::mem::take(&mut self.nodes[id].children);
        if old.is_empty() && children.is_empty() {
            return;
        }
        let mut used: SmallVec<[bool; 32]> = SmallVec::from_elem(false, old.len());
        // (The old ones with no key, in order: each new one with none
        // takes the next.)
        let mut unkeyed = old.iter().enumerate().filter(|(_, c)| self.nodes[**c].key.is_none() && self.nodes[**c].view_root.is_none()).map(|(i, _)| i).collect::<SmallVec<[usize; 32]>>().into_iter();
        let mut now: Vec<NodeId> = Vec::with_capacity(children.len());
        for element in children {
            let found = match (&element.kind, &element.key) {
                (Kind::View(view), _) => old.iter().position(|c| self.nodes[*c].view_root == Some(*view)),
                (_, Some(key)) => old.iter().enumerate().position(|(i, c)| !used[i] && self.nodes[*c].key.as_ref() == Some(key) && self.nodes[*c].view_root.is_none()),
                (_, None) => unkeyed.next(),
            };
            let child = match found {
                Some(i) if matches!(element.kind, Kind::View(_)) => {
                    used[i] = true;
                    old[i]
                }
                Some(i) if self.nodes[old[i]].kind.tag() == element.kind.tag() => {
                    used[i] = true;
                    self.reconcile(old[i], element, owner);
                    old[i]
                }
                _ => self.create(element, owner, Some(id)),
            };
            now.push(child);
        }
        for (i, child) in old.iter().enumerate() {
            // (Not one that went elsewhere as the new ones were made: a
            // view now placed deeper in what replaced it.)
            if !used[i] && self.nodes.get(*child).is_some_and(|n| n.parent == Some(id)) {
                self.destroy(*child);
            }
        }
        if now != old {
            let layouts: SmallVec<[LayoutId; 16]> = now.iter().map(|c| self.nodes[*c].layout).collect();
            let layout = self.nodes[id].layout;
            self.layout.set_children(layout, &layouts);
            self.reshaped();
        }
        self.nodes[id].children = now;
    }

    /// The text style in force at a node: the window's, as refined by
    /// everything that holds it, outermost first.
    pub(crate) fn text_refinements(&self, id: NodeId) -> Vec<TextStyleRefinement> {
        let mut found: SmallVec<[&TextStyleRefinement; 16]> = SmallVec::new();
        let mut at = Some(id);
        while let Some(node) = at.and_then(|id| self.nodes.get(id)) {
            if let Some(text) = node.style.text_style() {
                found.push(text);
            }
            at = node.parent;
        }
        found.into_iter().rev().cloned().collect()
    }

    pub(crate) fn text_style_at(&self, id: NodeId) -> TextStyle {
        let mut style = self.gfx.text_style.clone();
        for refinement in self.text_refinements(id) {
            style.refine(&refinement);
        }
        style
    }

    /// How faded what's at a node is drawn (what holds it may be).
    pub(crate) fn opacity_at(&self, id: NodeId) -> f32 {
        let mut opacity = 1.0;
        let mut at = self.nodes.get(id).and_then(|n| n.parent);
        while let Some(node) = at.and_then(|id| self.nodes.get(id)) {
            if let Some(o) = node.style.opacity {
                opacity *= o;
            }
            at = node.parent;
        }
        opacity
    }

    /// Sizes a leaf by what it holds (layout asks, with what's known of
    /// its size and the room it has).
    pub(crate) fn measure(&mut self, id: NodeId, known: Size<Option<Pixels>>, available: Size<AvailableSpace>) -> Size<Pixels> {
        let Some(node) = self.nodes.get(id) else { return Size::default() };
        match &node.kind {
            NodeKind::Group => Size::default(),
            NodeKind::Canvas(spec) => {
                let Some(measure) = spec.measure.clone() else { return Size::default() };
                let owner = node.owner;
                let text_style = self.text_style_at(id);
                let Some(view) = self.views.get(owner).and_then(|v| v.view.as_ref()) else { return Size::default() };
                let mut measuring = Measuring { text_system: &self.gfx.text_system, rem_size: self.gfx.rem_size, scale_factor: self.gfx.scale_factor, text_style: &text_style };
                measure(view.as_any(), known, available, &mut measuring)
            }
            NodeKind::Text(_) => {
                let text_style = self.text_style_at(id);
                let rem_size = self.gfx.rem_size;
                let text_system = self.gfx.text_system.clone();
                let NodeKind::Text(text) = &mut self.nodes[id].kind else { return Size::default() };
                measure_text(text, &text_style, rem_size, &text_system, known, available)
            }
        }
    }

    /// Draws a node again: what it draws is kept in place of what it
    /// last drew.
    pub(crate) fn record(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(id) else { return };
        let bounds = node.bounds;
        let state = node.state();
        let opacity = self.opacity_at(id);
        let mut text_styles = self.text_refinements(id);
        // (Its own text style is for what it holds; a text's own is
        // already in what it was measured with.)
        if matches!(node.kind, NodeKind::Group) {
            text_styles.pop();
        }
        let visible = self.visible_at(id);
        let WindowCore { nodes, views, gfx, recycled, .. } = self;
        let node = &mut nodes[id];
        let own_opacity = node.style.opacity;
        let mut notes = Notes::default();
        let mut kept = node.state.take();
        // Behind what it holds: its ground, and what it is.
        let mut scene = recycled.pop().unwrap_or_else(crate::Scene::recording);
        {
            let mut p = Painter::new(gfx, &mut scene, &mut notes, &mut kept, visible, opacity, text_styles.clone());
            p.with_element_opacity(own_opacity, |p| {
                node.style.paint_behind(bounds, p);
                match &node.kind {
                    NodeKind::Group => {}
                    NodeKind::Text(text) => paint_text(text, bounds, p, !node.inter.as_ref().is_some_and(|i| i.unlabelled)),
                    NodeKind::Canvas(spec) => {
                        if let Some(view) = views.get(node.owner).and_then(|v| v.view.as_ref()) {
                            // A control that can't be used now is faded.
                            let fade = (!state.enabled && node.inter.as_ref().is_some_and(|i| i.paints_by_state)).then_some(crate::DISABLED_OPACITY);
                            p.with_element_opacity(fade, |p| (spec.paint)(view.as_any(), p, bounds, state));
                        }
                    }
                }
            });
        }
        node.behind = keep(scene, recycled);
        // Over it: its border.
        node.over = None;
        if node.style.paints_over() {
            let mut scene = recycled.pop().unwrap_or_else(crate::Scene::recording);
            {
                let mut p = Painter::new(gfx, &mut scene, &mut notes, &mut kept, visible, opacity, text_styles);
                p.with_element_opacity(own_opacity, |p| node.style.paint_over(bounds, p));
            }
            node.over = keep(scene, recycled);
        }
        node.state = kept;
        node.painted_at = bounds.origin;
        node.paint_dirty = false;
        if notes.by_visible {
            // (Drawn again when what can be seen of it changes.)
            notes.visible = visible;
            if !self.by_visible.contains(&id) {
                self.by_visible.push(id);
            }
        }
        let again = notes.again.take();
        node.notes = (!notes.is_empty()).then(|| Box::new(notes));
        if let Some(seconds) = again {
            self.clock.ask(id, seconds);
        }
    }

    /// What can be seen of a node, where it's drawn: the window, within
    /// everything that clips it. Inside something that scrolls, that's
    /// its view, wherever it's scrolled to, taken to the bands of a
    /// grid with one to spare each way: what draws only what can be
    /// seen draws again once in a while as it scrolls, not every frame.
    pub(crate) fn visible_at(&self, id: NodeId) -> Bounds<Pixels> {
        let mut path: SmallVec<[NodeId; 16]> = SmallVec::new();
        let mut at = self.nodes.get(id).and_then(|n| n.parent);
        while let Some(id) = at {
            path.push(id);
            at = self.nodes.get(id).and_then(|n| n.parent);
        }
        let mut visible = Bounds { origin: point(px(0.), px(0.)), size: self.size };
        for id in path.iter().rev() {
            let node = &self.nodes[*id];
            if let Some(mask) = node.style.overflow_mask(node.bounds, self.gfx.rem_size) {
                visible = visible.intersect(&mask.bounds);
            }
            if let Some(handle) = &node.scroll {
                let offset = handle.offset();
                let max = handle.max_offset();
                let band = |from: Pixels, to: Pixels, by: Pixels, scrolls: bool| -> (Pixels, Pixels) {
                    let (from, to) = (from - by, to - by);
                    if !scrolls {
                        return (from, to);
                    }
                    (px(((from.0 / VISIBLE_BAND).floor() - 1.0) * VISIBLE_BAND), px(((to.0 / VISIBLE_BAND).ceil() + 1.0) * VISIBLE_BAND))
                };
                let (left, right) = band(visible.left(), visible.right(), offset.x, max.width > px(0.));
                let (top, bottom) = band(visible.top(), visible.bottom(), offset.y, max.height > px(0.));
                visible = Bounds::from_corners(point(left, top), point(right, bottom));
            }
        }
        visible
    }

    /// What draws by what can be seen of it, and can now be seen
    /// differently, is drawn again.
    pub(crate) fn check_visible(&mut self) {
        if self.by_visible.is_empty() {
            return;
        }
        let mut list = std::mem::take(&mut self.by_visible);
        list.retain(|id| self.nodes.get(*id).is_some_and(|n| n.notes.as_ref().is_some_and(|notes| notes.by_visible)));
        for id in &list {
            let seen = self.nodes[*id].notes.as_ref().map(|n| n.visible);
            if seen != Some(self.visible_at(*id)) {
                self.mark_paint(*id);
            }
        }
        self.by_visible = list;
    }

    /// How far what's in a node shows from where it's laid out: by
    /// everything round it that scrolls.
    pub(crate) fn shift_at(&self, id: NodeId) -> Point<Pixels> {
        let mut shift = Point::default();
        let mut at = self.nodes.get(id).and_then(|n| n.parent);
        while let Some(node) = at.and_then(|id| self.nodes.get(id)) {
            if let Some(handle) = &node.scroll {
                shift = shift + handle.0.shown.get();
            }
            at = node.parent;
        }
        shift
    }

    /// Where a node is in the window now.
    pub(crate) fn window_bounds(&self, id: NodeId) -> Bounds<Pixels> {
        let Some(node) = self.nodes.get(id) else { return Bounds::default() };
        Bounds { origin: node.bounds.origin + self.shift_at(id), size: node.bounds.size }
    }
}

/// The elements of rows `near`, each put at its row.
fn place_rows(spec: &crate::element::RowsSpec, near: std::ops::Range<usize>) -> Vec<Element> {
    use crate::Styled as _;
    if near.is_empty() {
        return Vec::new();
    }
    let made = (spec.make)(near.clone());
    made.into_iter().zip(near).map(|(row, at)| row.absolute().left(px(0.)).top(spec.row_height * at as f32).h(spec.row_height)).collect()
}

/// The grid what can be seen inside a scroller is taken to (points).
const VISIBLE_BAND: f32 = 256.0;

/// How faded a control that can't be used now is drawn.
pub const DISABLED_OPACITY: f32 = 0.5;

fn keep(scene: crate::Scene, recycled: &mut Vec<crate::Scene>) -> Option<Arc<Recording>> {
    if scene.is_empty() {
        recycled.push(scene);
        return None;
    }
    let bounds = scene.extent();
    Some(Arc::new(Recording { scene, bounds }))
}

/// Measures a text: shaped, wrapped to the room it has, cut short if its
/// style says. (GPUI's `TextLayout::layout`, kept between frames.)
fn measure_text(text: &mut TextNode, text_style: &TextStyle, rem_size: Pixels, text_system: &Arc<WindowTextSystem>, known: Size<Option<Pixels>>, available: Size<AvailableSpace>) -> Size<Pixels> {
    let font_size = text_style.font_size.to_pixels(rem_size);
    let line_height = text_style.line_height.to_pixels(font_size.into(), rem_size);
    let definite = |space: AvailableSpace| match space {
        AvailableSpace::Definite(x) => Some(x),
        _ => None,
    };
    let wrap_width = if text_style.white_space == WhiteSpace::Normal { known.width.or(definite(available.width)) } else { None };
    let (truncate_width, truncation_suffix) = match text_style.text_overflow.clone() {
        Some(TextOverflow::Truncate(suffix)) => {
            let width = known.width.or(definite(available.width).map(|x| match text_style.line_clamp {
                Some(max_lines) => x * max_lines,
                None => x,
            }));
            (width, suffix)
        }
        None => (None, "".into()),
    };
    if let Some(laid) = &text.laid
        && (wrap_width.is_none() || wrap_width == laid.wrap_width)
        && truncate_width.is_none()
    {
        return laid.size;
    }
    let mut runs = text.runs.clone().unwrap_or_else(|| vec![text_style.to_run(text.text.len())]);
    let whole = text.text.clone();
    let shown = match truncate_width {
        Some(width) => text_system.line_wrapper(text_style.font(), font_size).truncate_line(whole.clone(), width, &truncation_suffix, &mut runs),
        None => whole.clone(),
    };
    let mut truncated = shown != whole;
    let Ok(lines) = text_system.shape_text(shown, font_size, &runs, wrap_width, text_style.line_clamp) else {
        text.laid = Some(TextLaid { lines: SmallVec::new(), line_height, wrap_width, size: Size::default(), truncated: false, align: text_style.text_align });
        return Size::default();
    };
    // Clamped short of the rows it takes (each line's rows counted again
    // unclamped: the line cache keeps a clamped layout under the
    // unclamped one's key).
    if !truncated && let (Some(clamp), Some(width)) = (text_style.line_clamp, wrap_width) {
        let rows: usize = lines.iter().map(|l| l.unwrapped_layout.compute_wrap_boundaries(&l.text, width, None).len() + 1).sum();
        truncated = rows > clamp;
    }
    let mut size: Size<Pixels> = Size::default();
    for line in &lines {
        let line_size = line.size(line_height);
        size.height += line_size.height;
        size.width = size.width.max(line_size.width).ceil();
    }
    text.laid = Some(TextLaid { lines, line_height, wrap_width, size, truncated, align: text_style.text_align });
    size
}

fn paint_text(text: &TextNode, bounds: Bounds<Pixels>, p: &mut Painter, labelled: bool) {
    let Some(laid) = &text.laid else { return };
    let mut origin = bounds.origin;
    for line in &laid.lines {
        let _ = line.paint_background(origin, laid.line_height, laid.align, Some(bounds), p);
        let _ = line.paint(origin, laid.line_height, laid.align, Some(bounds), p);
        origin.y += line.size(laid.line_height).height;
    }
    // What agents read: its text, where it is; whole, if it's cut
    // short here.
    if labelled && !text.text.trim().is_empty() {
        p.agent(crate::agent::Node::label(text.text.clone(), bounds));
        if laid.truncated {
            p.note_cut(text.text.clone(), bounds);
        }
    }
}

/// The byte of a laid-out text nearest `position` (from the text's own
/// corner): GPUI's `TextLayout::index_for_position`.
pub(crate) fn text_index_at(laid: &TextLaid, position: Point<Pixels>) -> usize {
    let mut line_top = px(0.);
    let mut line_start = 0;
    for line in &laid.lines {
        let line_bottom = line_top + line.size(laid.line_height).height;
        if position.y > line_bottom {
            line_top = line_bottom;
            line_start += line.len() + 1;
        } else {
            return match line.index_for_position(point(position.x, position.y - line_top), laid.line_height) {
                Ok(index) | Err(index) => line_start + index,
            };
        }
    }
    line_start.saturating_sub(1)
}

/// A node as something outside the tree may know it (what a view's
/// drawing is given of the view it belongs to).
pub(crate) fn view_any(view: &dyn crate::view::AnyView) -> &dyn Any {
    view.as_any()
}

/// (Clips are the padding box of what scrolls or hides its overflow.)
pub(crate) fn clip_of(style: &Style, bounds: Bounds<Pixels>, rem_size: Pixels) -> Option<ContentMask<Pixels>> {
    style.overflow_mask(bounds, rem_size)
}

/// What a recording needs of the window to be made.
pub(crate) fn _uses(_: &Gfx) {}
