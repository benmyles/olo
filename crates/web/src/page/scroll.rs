//! CSSOM scrolling plans, made from one retained layout snapshot.
use super::Measured;
use crate::{
    css::{
        math,
        properties::{Keyword as K, Property as P},
        style::{Axes, Direction, Style, Value, WritingMode},
    },
    dom::{Document, NodeId, NodeMap},
    layout::{Rect, ScrollRange},
};

#[derive(Clone, Copy, Debug)]
pub enum Alignment {
    Start,
    Center,
    End,
    Nearest,
}
impl Alignment {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "start" => Self::Start,
            "center" => Self::Center,
            "end" => Self::End,
            "nearest" => Self::Nearest,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ScrollTarget {
    pub node: Option<NodeId>,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub smooth: bool,
}
#[derive(Clone, Copy)]
pub(super) struct Extent {
    pub min: [f64; 2],
    pub max: [f64; 2],
}
impl Extent {
    fn rect(r: Rect) -> Self {
        Self { min: [r.x, r.y], max: [r.x + r.width, r.y + r.height] }
    }
    fn include(&mut self, other: Self, axes: [bool; 2]) {
        for i in 0..2 {
            if axes[i] {
                self.min[i] = self.min[i].min(other.min[i]);
                self.max[i] = self.max[i].max(other.max[i]);
            }
        }
    }
}
fn keyword(s: &Style, p: P) -> K {
    match s.value(p) {
        Value::Keyword(k) => *k,
        _ => K::Auto,
    }
}
fn reverse(axes: Axes) -> [bool; 2] {
    let rtl = axes.used_direction() == Direction::Rtl;
    match axes.writing_mode {
        WritingMode::HorizontalTb => [rtl, false],
        WritingMode::VerticalRl | WritingMode::SidewaysRl => [true, rtl],
        WritingMode::VerticalLr => [false, rtl],
        WritingMode::SidewaysLr => [false, !rtl],
    }
}
struct Basis(f64);
impl math::Context for Basis {
    fn factor(&self, u: math::RelativeUnit) -> math::Result<f64> {
        if u == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
    }
}
fn length(s: &Style, p: P, basis: f64) -> f64 {
    match s.value(p) {
        Value::Numeric(n) => n.resolve(&Basis(basis), &mut Default::default(), Default::default(), None).unwrap_or(0.),
        _ => 0.,
    }
}
fn inset(r: Rect, s: &Style, p: [P; 4], sign: f64) -> Rect {
    let t = length(s, p[0], r.height) * sign;
    let b = length(s, p[2], r.height) * sign;
    let l = length(s, p[3], r.width) * sign;
    let right = length(s, p[1], r.width) * sign;
    Rect { x: r.x + l, y: r.y + t, width: (r.width - l - right).max(0.), height: (r.height - t - b).max(0.) }
}
fn delta(a: f64, b: f64, start: f64, end: f64, alignment: Alignment, reverse: bool) -> f64 {
    let alignment = match (alignment, reverse) {
        (Alignment::Start, true) => Alignment::End,
        (Alignment::End, true) => Alignment::Start,
        (a, _) => a,
    };
    match alignment {
        Alignment::Start => a - start,
        Alignment::End => b - end,
        Alignment::Center => (a + b - start - end) / 2.,
        Alignment::Nearest => {
            if (a < start && b > end) || (a >= start && b <= end) {
                0.
            } else if (a < start && b - a <= end - start) || (b > end && b - a > end - start) {
                a - start
            } else {
                b - end
            }
        }
    }
}
impl Measured {
    // One bottom-up pass. A clipped descendant contributes its border box,
    // not the overflow hidden behind it. Scroll reads reuse this snapshot.
    fn extents(&self) -> &NodeMap<Extent> {
        self.scroll_extents.get_or_init(|| {
            let tree = self.layout.cache().tree();
            let n = tree.boxes().len();
            let mut origins = vec![[0.; 2]; n];
            let mut bounds = vec![None; n];
            let mut totals = vec![None; n];
            for (id, b) in tree.iter() {
                let i = id.index();
                if let Some(p) = b.parent {
                    if let Some(g) = self.layout.geometry(p) {
                        let c = g.content_box();
                        origins[i] = [origins[p.index()][0] + c.x, origins[p.index()][1] + c.y];
                    }
                }
                if let Some(g) = self.layout.geometry(id) {
                    let r = g.border_box;
                    let r = Rect { x: r.x + origins[i][0], y: r.y + origins[i][1], ..r };
                    bounds[i] = Some(Extent::rect(r));
                    totals[i] = Some(Extent::rect(Rect {
                        x: r.x + g.border_width.left,
                        y: r.y + g.border_width.top,
                        width: (r.width - g.border_width.horizontal()).max(0.),
                        height: (r.height - g.border_width.vertical()).max(0.),
                    }));
                }
            }
            for i in (0..n).rev() {
                let Some((id, b)) = tree.at(i) else { continue };
                let Some(p) = b.parent else { continue };
                if self.layout.fixed_box(id) {
                    continue;
                }
                let Some(mut contribution) = bounds[i] else { continue };
                let axes = tree.style(id).map_or([true; 2], |s| [keyword(s, P::OverflowX) == K::Visible, keyword(s, P::OverflowY) == K::Visible]);
                if let Some(t) = totals[i] {
                    contribution.include(t, axes);
                }
                if let Some(parent) = &mut totals[p.index()] {
                    parent.include(contribution, [true; 2]);
                }
            }
            let mut result = NodeMap::default();
            for (&node, &id) in &self.index {
                if let Some(t) = totals[id.index()] {
                    result.insert(node, t);
                }
            }
            result
        })
    }
    pub fn scroll_range(&self, node: NodeId) -> Option<ScrollRange> {
        let s = self.layout.cache().tree().styles().snapshot(node)?;
        let padding = self.padding_box(node)?;
        let extent = self.extents().get(&node)?;
        let origin = [padding.x, padding.y];
        let size = [padding.width, padding.height];
        let rev = reverse(s.axes());
        let mut range = ScrollRange::default();
        for i in 0..2 {
            if matches!(keyword(s, [P::OverflowX, P::OverflowY][i]), K::Visible | K::Clip) {
                continue;
            }
            if rev[i] {
                range.min[i] = (extent.min[i] - origin[i]).min(0.);
            } else {
                range.max[i] = (extent.max[i] - origin[i] - size[i]).max(0.);
            }
        }
        Some(range)
    }
    pub fn clamp_element_scroll(&self, node: NodeId, to: [f64; 2]) -> Option<[f64; 2]> {
        let range = self.scroll_range(node)?;
        let scale = self.viewport.scale;
        Some(range.clamp(range.clamp(to).map(|v| f64::from(((v as f32) * (scale as f32) + 0.5).floor()) / scale)))
    }
    pub(super) fn scroll_extent_size(&self, node: NodeId) -> Option<(f64, f64)> {
        let r = self.padding_box(node)?;
        let e = self.extents().get(&node)?;
        let s = self.layout.cache().tree().styles().snapshot(node)?;
        let rev = reverse(s.axes());
        Some((
            if rev[0] { (r.x + r.width - e.min[0]).max(r.width) } else { (e.max[0] - r.x).max(r.width) },
            if rev[1] { (r.y + r.height - e.min[1]).max(r.height) } else { (e.max[1] - r.y).max(r.height) },
        ))
    }
    /// Inner-to-outer destinations, calculated without mutating the document.
    /// `nearest` stops after the first scrolling ancestor, even if it is full.
    pub fn scroll_into_view(&self, doc: &Document, node: NodeId, window: [f64; 2], block: Alignment, inline: Alignment, nearest: bool) -> Vec<ScrollTarget> {
        let mut out = Vec::new();
        if !doc.is_connected(node).unwrap_or(false) {
            return out;
        }
        let Some(mut target) = self.border_box(node).map(|r| self.transformed(doc, node, r)) else { return out };
        let tree = self.layout.cache().tree();
        let styles = tree.styles();
        let source = tree.viewport_overflow().source;
        let target_style = styles.snapshot(node);
        let target_axes = target_style.map_or(Axes::default(), Style::axes);
        let area = |r| target_style.map_or(r, |s| inset(r, s, [P::ScrollMarginTop, P::ScrollMarginRight, P::ScrollMarginBottom, P::ScrollMarginLeft], -1.));
        let mut chain = Vec::new();
        let mut at = doc.composed_parent(node).ok().flatten().or_else(|| doc.shadow_host(node));
        while let Some(n) = at.filter(|_| chain.len() < 1024) {
            chain.push(n);
            let o = doc.scroll_offset(n);
            target.x -= o[0];
            target.y -= o[1];
            at = doc.composed_parent(n).ok().flatten().or_else(|| doc.shadow_host(n));
        }
        let fixed = self.viewport_fixed(doc, node);
        if fixed && styles.snapshot(node).is_some_and(|s| keyword(s, P::Position) == K::Fixed) {
            return out;
        }
        if !fixed {
            target.x -= window[0];
            target.y -= window[1];
        }
        let root = chain.iter().rev().find(|&&n| doc.node_type(n).ok() == Some(crate::dom::NodeType::Element)).copied();
        for (index, &n) in chain.iter().enumerate() {
            if Some(n) == source || Some(n) == root {
                continue;
            }
            let Some(s) = styles.snapshot(n) else { continue };
            if [P::OverflowX, P::OverflowY].iter().all(|&p| matches!(keyword(s, p), K::Visible | K::Clip)) {
                continue;
            }
            let Some(mut r) = self.padding_box(n) else { continue };
            for &ancestor in &chain[index + 1..] {
                let o = doc.scroll_offset(ancestor);
                r.x -= o[0];
                r.y -= o[1];
            }
            if !self.viewport_fixed(doc, n) {
                r.x -= window[0];
                r.y -= window[1];
            }
            let clip = r;
            r = inset(r, s, [P::ScrollPaddingTop, P::ScrollPaddingRight, P::ScrollPaddingBottom, P::ScrollPaddingLeft], 1.);
            let from = doc.scroll_offset(n);
            let to = destination(area(target), r, target_axes, from, block, inline);
            let to = self.clamp_element_scroll(n, to).unwrap_or(from);
            target.x -= to[0] - from[0];
            target.y -= to[1] - from[1];
            out.push(ScrollTarget { node: Some(n), from, to, smooth: keyword(s, P::ScrollBehavior) == K::Smooth });
            // An outer scroller reveals the part exposed by the inner
            // scrollport. It cannot reveal overflow clipped inside that port.
            let x = target.x.max(clip.x);
            let y = target.y.max(clip.y);
            let right = (target.x + target.width).min(clip.x + clip.width);
            let bottom = (target.y + target.height).min(clip.y + clip.height);
            if right >= x {
                target.x = x;
                target.width = right - x;
            }
            if bottom >= y {
                target.y = y;
                target.height = bottom - y;
            }
            if nearest || (fixed && keyword(s, P::Position) == K::Fixed) {
                return out;
            }
        }
        if !fixed {
            if let Some(s) = root.and_then(|n| styles.snapshot(n)) {
                let client = self.layout.viewport_geometry().client;
                let r = Rect { x: 0., y: 0., width: client[0], height: client[1] };
                let r = inset(r, s, [P::ScrollPaddingTop, P::ScrollPaddingRight, P::ScrollPaddingBottom, P::ScrollPaddingLeft], 1.);
                let to = self.layout.clamp_viewport_scroll(destination(area(target), r, target_axes, window, block, inline));
                out.push(ScrollTarget { node: None, from: window, to, smooth: keyword(s, P::ScrollBehavior) == K::Smooth });
            }
        }
        out
    }
}
fn destination(target: Rect, port: Rect, axes: Axes, from: [f64; 2], block: Alignment, inline: Alignment) -> [f64; 2] {
    let rev = reverse(axes);
    let align = if axes.writing_mode == WritingMode::HorizontalTb { [inline, block] } else { [block, inline] };
    [
        from[0] + delta(target.x, target.x + target.width, port.x, port.x + port.width, align[0], rev[0]),
        from[1] + delta(target.y, target.y + target.height, port.y, port.y + port.height, align[1], rev[1]),
    ]
}
