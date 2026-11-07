//! Deferred positioned block layout. Normal-flow containing blocks finish first.
use super::super::positioned::{self, Axis, Input};
use super::*;
use crate::css::style::{Direction, Style};
#[derive(Clone, Copy)]
pub(super) struct Deferred {
    pub id: BoxId,
    pub queries: QueryBasis,
    /// Where normal flow would have put the box, in its parent's content box:
    /// the static position an auto inset falls back to.
    pub static_position: Point,
}
pub(super) struct Active {
    pub id: BoxId,
    pub used: Used,
    pub x: f64,
    pub y: Input,
    pub offset_y: f64,
}
#[allow(clippy::too_many_arguments)] // Explicit immutable geometry and worker resources.
fn input(
    s: &Style,
    axis: Axis,
    width: f64,
    height: f64,
    u: Used,
    size_override: Option<f64>,
    intrinsic: [f64; 2],
    static_position: f64,
    queries: QueryBasis,
    scratch: &mut math::Scratch,
    cancel: Option<&AtomicBool>,
    scale: f64,
) -> Result<Input> {
    let horizontal = matches!(axis, Axis::Horizontal { .. });
    let basis = if horizontal { width } else { height };
    let props = if horizontal {
        [P::Left, P::Right, P::Width, P::MinWidth, P::MaxWidth, P::MarginLeft, P::MarginRight]
    } else {
        [P::Top, P::Bottom, P::Height, P::MinHeight, P::MaxHeight, P::MarginTop, P::MarginBottom]
    };
    let extra = if horizontal { u.padding.horizontal() + u.border.horizontal() } else { u.padding.vertical() + u.border.vertical() };
    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
    let content = |v: f64| (v - if border_box { extra } else { 0. }).max(0.);
    let mut len = |p, b, f| used::length(s, p, Some(b), f, queries, scratch, cancel, scale);
    let inset = [len(props[0], basis, None)?, len(props[1], basis, None)?];
    let size = match size_override {
        Some(size) => Some(size),
        None => len(props[2], basis, None)?.map(content),
    };
    Ok(Input {
        axis,
        containing: basis,
        inset,
        size,
        margin: [len(props[5], width, None)?, len(props[6], width, None)?],
        decoration: extra,
        minimum: len(props[3], basis, Some(0.))?.map(content).unwrap(),
        maximum: len(props[4], basis, None)?.map(content),
        intrinsic,
        static_position,
    })
}
impl Job<'_> {
    pub(super) fn positioned(&self, id: BoxId) -> Result<bool> {
        Ok(self.cache.tree.get(id).ok_or(Error::Invalid)?.absolute)
    }
    pub(super) fn defer_absolute(&mut self, id: BoxId) -> Result<()> {
        if self.absolute.len() >= self.limits.boxes {
            return Err(Error::Limit);
        }
        let parent = self.stack.last().ok_or(Error::Invalid)?;
        // The next block would start at the cursor plus the pending margin; in a
        // flex row it would start where the row's cursor is. Both are the
        // static position's block-axis estimate for a hypothetical block box.
        let static_position = Point { x: 0., y: parent.cursor + parent.pending.value() };
        if let Some(&at) = self.absolute_indices.get(&id) {
            self.absolute[at].queries = parent.queries;
            self.absolute[at].static_position = static_position;
        } else {
            self.absolute_indices.insert(id, self.absolute.len());
            self.absolute.push(Deferred { id, queries: parent.queries, static_position });
        }
        Ok(())
    }
    fn global_content(&self, id: BoxId) -> Result<Rect> {
        let mut result = self.geometry[id.index()].content_box();
        let mut parent = self.cache.tree.get(id).ok_or(Error::Invalid)?.parent;
        let mut depth = 0;
        while let Some(id) = parent {
            check(self.cancel)?;
            depth += 1;
            if depth > self.limits.depth {
                return Err(Error::Limit);
            }
            let p = self.geometry[id.index()].content_box();
            result.x += p.x;
            result.y += p.y;
            parent = self.cache.tree.get(id).ok_or(Error::Invalid)?.parent;
        }
        Ok(result)
    }
    pub(super) fn begin_absolute(&mut self) -> Result<()> {
        let d = *self.absolute.get(self.absolute_at).ok_or(Error::Invalid)?;
        self.absolute_at += 1;
        let cache = self.cache.clone();
        let tree = &cache.tree;
        let node = tree.get(d.id).ok_or(Error::Invalid)?;
        let parent = node.parent.ok_or(Error::Invalid)?;
        let mut cb = parent;
        let mut depth = 0;
        while cb != tree.root() && matches!(tree.style(cb).ok_or(Error::Stale)?.value(P::Position), Value::Keyword(K::Static)) {
            check(self.cancel)?;
            depth += 1;
            if depth > self.limits.depth {
                return Err(Error::Limit);
            }
            cb = tree.get(cb).ok_or(Error::Invalid)?.parent.ok_or(Error::Invalid)?;
        }
        let cg = self.geometry[cb.index()];
        let content = self.global_content(cb)?;
        let parent_content = self.global_content(parent)?;
        let width = content.width + cg.padding.horizontal();
        let height = content.height + cg.padding.vertical();
        let s = tree.style(d.id).ok_or(Error::Stale)?;
        let rtl = tree.style(cb).is_some_and(|s| s.axes().direction == Direction::Rtl);
        // Static positions relative to the containing block's padding box.
        let static_x = parent_content.x + d.static_position.x - (content.x - cg.padding.left);
        let static_y = parent_content.y + d.static_position.y - (content.y - cg.padding.top);
        if node.kind.is_replaced() {
            // Replaced boxes have their size before placement (CSS 2.2 10.3.8/10.6.5).
            let natural = tree.natural(d.id)?;
            let mut u = super::images::used(s, width, Some(height), natural, false, rtl, d.queries, &mut self.scratch, self.cancel, self.scale)?;
            let h = u.height.ok_or(Error::Invalid)?;
            let x = input(s, Axis::Horizontal { rtl, static_rtl: rtl }, width, height, u, Some(u.width), [u.width, u.width], static_x, d.queries, &mut self.scratch, self.cancel, self.scale)?;
            let y = input(s, Axis::Vertical, width, height, u, Some(h), [h, h], static_y, d.queries, &mut self.scratch, self.cancel, self.scale)?;
            let horizontal = positioned::solve(x, self.scale)?;
            let vertical = positioned::solve(y, self.scale)?;
            u.margin = Edges { left: horizontal.margin[0], right: horizontal.margin[1], top: vertical.margin[0], bottom: vertical.margin[1] };
            let mut g = super::images::geometry(u);
            g.border_box.x = content.x - cg.padding.left - parent_content.x + horizontal.border_start();
            g.border_box.y = content.y - cg.padding.top - parent_content.y + vertical.border_start();
            self.geometry[d.id.index()] = g;
            self.progress.boxes += 1;
            return Ok(());
        }
        if !matches!(node.kind, BoxKind::Block { .. }) {
            return Err(Error::Invalid);
        }
        let mut u = Used::flex(s, width, Some(height), d.queries, &mut self.scratch, self.cancel, self.scale)?;
        // Shrink-to-fit needs the content's intrinsic widths only when the
        // width is auto and an inset is auto; other boxes never measure.
        let width_auto = used::length(s, P::Width, Some(width), None, d.queries, &mut self.scratch, self.cancel, self.scale)?.is_none();
        let left = used::length(s, P::Left, Some(width), None, d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let right = used::length(s, P::Right, Some(width), None, d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let intrinsic = if width_auto && (left.is_none() || right.is_none()) {
            let (w, _) = self.intrinsic_content(d.id, d.queries, width, 0)?;
            [w.min_content, w.max_content]
        } else {
            [0., 0.]
        };
        let x = input(s, Axis::Horizontal { rtl, static_rtl: rtl }, width, height, u, None, intrinsic, static_x, d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let y = input(s, Axis::Vertical, width, height, u, None, [0., 0.], static_y, d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let horizontal = positioned::solve(x, self.scale)?;
        let vertical = positioned::solve(y, self.scale)?;
        u.width = horizontal.size;
        u.margin.left = horizontal.margin[0];
        u.margin.right = horizontal.margin[1];
        if y.size.is_some() || y.inset.iter().all(Option::is_some) {
            u.height = Some(vertical.size);
        }
        u.margin.top = vertical.margin[0];
        u.margin.bottom = vertical.margin[1];
        u.relative = Point::default();
        u.bfc = true;
        self.active_absolute =
            Some(Active { id: d.id, used: u, x: content.x - cg.padding.left - parent_content.x + horizontal.border_start(), y, offset_y: content.y - cg.padding.top - parent_content.y });
        self.stack.push(Frame {
            id: parent,
            next: None,
            used: u,
            cursor: 0.,
            pending: Strut::default(),
            top: Strut::default(),
            at_start: true,
            collapse_top: false,
            collapse_bottom: false,
            through: false,
            queries: d.queries,
        });
        self.enter(d.id)
    }
    pub(super) fn finish_absolute(&mut self, id: BoxId) -> Result<bool> {
        if !self.active_absolute.as_ref().is_some_and(|a| a.id == id) {
            return Ok(false);
        }
        let a = self.active_absolute.take().ok_or(Error::Invalid)?;
        let g = &mut self.geometry[id.index()];
        let h = g.content_box().height;
        let vertical = positioned::solve(Input { intrinsic: [h, h], ..a.y }, self.scale)?;
        g.border_box.x = a.x;
        g.border_box.y = a.offset_y + vertical.border_start();
        g.border_box.height = vertical.size + g.padding.vertical() + g.border_width.vertical();
        g.margin.top = vertical.margin[0];
        g.margin.bottom = vertical.margin[1];
        self.stack.pop().ok_or(Error::Invalid)?;
        Ok(true)
    }
}
