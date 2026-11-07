//! Deferred positioned block layout. Normal-flow containing blocks finish first.
use super::super::positioned::{self, Axis, Input};
use super::*;
use crate::css::style::{Direction, Style};
#[derive(Clone, Copy)]
pub(super) struct Deferred {
    pub id: BoxId,
    pub queries: QueryBasis,
}
pub(super) struct Active {
    pub id: BoxId,
    pub used: Used,
    pub x: f64,
    pub y: Input,
    pub offset_y: f64,
}
#[allow(clippy::too_many_arguments)] // Explicit immutable geometry and worker resources.
fn input(s: &Style, axis: Axis, width: f64, height: f64, u: Used, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Input> {
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
    if inset.iter().all(Option::is_none) {
        return Err(Error::Unsupported("absolute static-position discovery"));
    }
    let size = len(props[2], basis, None)?.map(content);
    if horizontal && size.is_none() && inset.iter().any(Option::is_none) {
        return Err(Error::Unsupported("absolute shrink-to-fit intrinsic sizing"));
    }
    Ok(Input {
        axis,
        containing: basis,
        inset,
        size,
        margin: [len(props[5], width, None)?, len(props[6], width, None)?],
        decoration: extra,
        minimum: len(props[3], basis, Some(0.))?.map(content).unwrap(),
        maximum: len(props[4], basis, None)?.map(content),
        intrinsic: [0., 0.],
        static_position: 0.,
    })
}
impl Job<'_> {
    pub(super) fn positioned(&self, id: BoxId) -> Result<bool> {
        Ok(matches!(self.cache.tree.get(id).ok_or(Error::Invalid)?.kind, BoxKind::Block { absolute: true, .. }))
    }
    pub(super) fn defer_absolute(&mut self, id: BoxId) -> Result<()> {
        if self.absolute.len() >= self.limits.boxes {
            return Err(Error::Limit);
        }
        let parent = self.stack.last().ok_or(Error::Invalid)?;
        if let Some(&at) = self.absolute_indices.get(&id) {
            self.absolute[at].queries = parent.queries;
        } else {
            self.absolute_indices.insert(id, self.absolute.len());
            self.absolute.push(Deferred { id, queries: parent.queries });
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
        let tree = &self.cache.tree;
        let node = tree.get(d.id).ok_or(Error::Invalid)?;
        if !matches!(node.kind, BoxKind::Block { .. }) {
            return Err(Error::Unsupported("positioned replaced box"));
        }
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
        let mut u = Used::flex(s, width, Some(height), d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let x = input(s, Axis::Horizontal { rtl, static_rtl: rtl }, width, height, u, d.queries, &mut self.scratch, self.cancel, self.scale)?;
        let y = input(s, Axis::Vertical, width, height, u, d.queries, &mut self.scratch, self.cancel, self.scale)?;
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
