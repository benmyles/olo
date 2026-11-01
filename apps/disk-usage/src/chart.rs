//! The ring chart, as Haiku's DiskUsage draws it: the folder in the
//! middle, what's in it in the first ring around it (each the share of
//! the circle its size is of the folder's), what's in those in the next
//! ring, and so on outwards. Each first-ring item has a colour of its own
//! that its contents carry outwards, lighter; files are greyer.

use std::f32::consts::TAU;

use olo_theme::{Hue, Palette, Rgba, mix};
use olo_ui::{
    gpui::{Bounds, Hsla, Pixels, Point, Window, point, px},
    style::{self, c},
};

use crate::scan::{NodeId, Tree};

/// Rings drawn around the middle.
pub const RINGS: usize = 5;
/// The smallest slice drawn, in radians, ~0.7° (smaller ones are left
/// out, as Haiku does; the list still has them).
const MIN_ANGLE: f32 = 0.012;

/// One slice of a ring.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub node: NodeId,
    /// 1 for the first ring.
    pub ring: usize,
    pub start: f32,
    pub end: f32,
    pub color: Hsla,
}

/// Where the chart is and how big its rings are.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub center: Point<Pixels>,
    /// The middle circle's radius; each ring is `ring` wide.
    pub hub: f32,
    pub ring: f32,
}

impl Geometry {
    pub fn fit(center: Point<Pixels>, radius: f32) -> Geometry {
        // The middle a little wider than a ring, as Haiku has it.
        let ring = (radius / (RINGS as f32 + 1.4)).floor();
        Geometry { center, hub: radius - ring * RINGS as f32, ring }
    }

    pub fn inner(&self, ring: usize) -> f32 {
        self.hub + self.ring * (ring - 1) as f32
    }

    /// The ring at `pos` (0 for the middle), and the angle, clockwise from
    /// the top.
    pub fn at(&self, pos: Point<Pixels>) -> Option<(usize, f32)> {
        let (dx, dy) = (f32::from(pos.x - self.center.x), f32::from(pos.y - self.center.y));
        let r = dx.hypot(dy);
        if r <= self.hub {
            return Some((0, 0.0));
        }
        let ring = ((r - self.hub) / self.ring).floor() as usize + 1;
        if ring > RINGS {
            return None;
        }
        Some((ring, angle(dx, dy)))
    }
}

/// Clockwise from 12 o'clock, 0..TAU.
fn angle(x: f32, y: f32) -> f32 {
    let a = x.atan2(-y);
    if a < 0.0 { a + TAU } else { a }
}

/// The look's hues for first-ring items, in turn: blue, yellow, green,
/// red, then purple, orange and teal.
const HUES: [Hue; 7] = [Hue::Blue, Hue::Yellow, Hue::Green, Hue::Red, Hue::Purple, Hue::Orange, Hue::Teal];

/// The slices for `folder`, ring by ring, in `p`'s hues.
pub fn layout(tree: &Tree, folder: NodeId, p: &Palette) -> Vec<Slice> {
    let hue = |i: usize| p.hue(HUES[i % HUES.len()]);
    let mut out = Vec::new();
    let total = tree.nodes[folder as usize].size.max(1) as f64;
    let min = (total * MIN_ANGLE as f64 / TAU as f64) as u64;
    let mut start = 0.0f32;
    for (i, child) in tree.children(folder, min.max(1)).into_iter().enumerate() {
        let sweep = (tree.nodes[child as usize].size as f64 / total * TAU as f64) as f32;
        place(tree, child, 1, start, sweep, hue(i), min.max(1), p, &mut out);
        start += sweep;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn place(tree: &Tree, node: NodeId, ring: usize, start: f32, sweep: f32, base: Rgba, min: u64, p: &Palette, out: &mut Vec<Slice>) {
    let n = &tree.nodes[node as usize];
    // Softer with each ring outwards (toward the ground it sits on);
    // files greyer.
    let mut color = mix(base, p.document_background, 0.16 * (ring - 1) as f32);
    if !n.dir {
        color = mix(color, p.hue(Hue::Gray), 0.55);
    }
    out.push(Slice { node, ring, start, end: start + sweep, color: c(color) });
    if ring == RINGS || !n.dir || n.size == 0 {
        return;
    }
    let mut at = start;
    for child in tree.children(node, min) {
        let s = (tree.nodes[child as usize].size as f64 / n.size as f64 * sweep as f64) as f32;
        place(tree, child, ring + 1, at, s, base, min, p, out);
        at += s;
    }
}

/// The slice at `pos`, if any.
pub fn hit(slices: &[Slice], g: &Geometry, pos: Point<Pixels>) -> Option<usize> {
    let (ring, a) = g.at(pos)?;
    slices.iter().position(|s| s.ring == ring && a >= s.start && a < s.end)
}

fn at(g: &Geometry, r: f32, a: f32) -> Point<Pixels> {
    point(g.center.x + px(a.sin() * r), g.center.y - px(a.cos() * r))
}

/// A third of the way to white: a slice under the pointer, or chosen.
fn lighter(color: Hsla) -> Hsla {
    let c = olo_ui::gpui::Rgba::from(color);
    let up = |v: f32| v + (1.0 - v) / 3.0;
    olo_ui::gpui::Rgba { r: up(c.r), g: up(c.g), b: up(c.b), a: c.a }.into()
}

/// Paints the rings: slices with a hairline between them in `line`, and
/// `hover` lighter. Each slice is one sector drawn on the GPU (no path to
/// tessellate or rasterise); the slices are one layer and the lines over
/// them another, so the scene orders two things, not hundreds.
pub fn paint(window: &mut Window, g: &Geometry, slices: &[Slice], hover: Option<usize>, selected: Option<NodeId>, line: Hsla) {
    let reach = px(g.inner(RINGS) + g.ring + 1.0);
    let bounds = Bounds::from_corners(point(g.center.x - reach, g.center.y - reach), point(g.center.x + reach, g.center.y + reach));
    window.paint_layer(bounds, |window| {
        for (i, s) in slices.iter().enumerate() {
            let color = if Some(i) == hover || Some(s.node) == selected { lighter(s.color) } else { s.color };
            let r0 = g.inner(s.ring);
            window.paint_sector(g.center, px(r0), px(r0 + g.ring), s.start, s.end - s.start, color);
        }
    });
    // Hairlines between slices and round each ring's outer edge, a device
    // pixel wide (one at 12 o'clock is straight up).
    let w = style::hairline(window);
    window.paint_layer(bounds, |window| {
        for s in slices {
            let (r0, r1) = (g.inner(s.ring), g.inner(s.ring) + g.ring);
            // Slices thinner than a few pixels share one edge, or a run of
            // small files becomes hatching.
            if (s.end - s.start) * r1 >= 6.0 {
                window.paint_segment(at(g, r0, s.start), at(g, r1, s.start), w, false, line);
            }
            if (s.end - s.start) * r1 >= 2.0 {
                window.paint_sector(g.center, px(r1) - w / 2.0, px(r1) + w / 2.0, s.start, s.end - s.start, line);
            }
        }
    });
}

/// The middle: the folder itself, a hairline round it.
pub fn paint_hub(window: &mut Window, g: &Geometry, fill: Hsla, line: Hsla) {
    let w = style::hairline(window);
    window.paint_sector(g.center, px(0.0), px(g.hub), 0.0, TAU, fill);
    window.paint_sector(g.center, px(g.hub) - w, px(g.hub), 0.0, TAU, line);
}

/// The middle of `ring` at angle `a`.
pub fn to_pos(g: &Geometry, ring: usize, a: f32) -> Point<Pixels> {
    at(g, g.inner(ring) + g.ring / 2.0, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, ROOT};
    use olo_ui::gpui::{point, px};

    fn pos2(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn tree(sizes: &[(u64, bool, u32)]) -> Tree {
        let mut t = Tree::new(std::path::Path::new("/x"));
        for (i, &(size, dir, parent)) in sizes.iter().enumerate() {
            let id = (i + 1) as NodeId;
            t.nodes.push(Node { name: format!("n{id}").into(), size, parent, dir, files: 1, children: Vec::new(), unreadable: false, gone: false });
            t.nodes[parent as usize].children.push(id);
        }
        t.nodes[ROOT as usize].size = t.nodes[ROOT as usize].children.iter().map(|&c| t.nodes[c as usize].size).sum();
        t
    }

    #[test]
    fn slices_share_the_circle_by_size() {
        // A folder of 300 (with a file of 100 in it) and a file of 100.
        let t = tree(&[(100, false, 0), (300, true, 0), (100, false, 2)]);
        let s = layout(&t, ROOT, &Palette::light());
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].node, s[0].ring), (2, 1), "largest first");
        assert!((s[0].end - TAU * 0.75).abs() < 1e-4);
        assert_eq!((s[1].node, s[1].ring), (3, 2));
        assert!((s[1].end - s[1].start - TAU * 0.25).abs() < 1e-4, "a third of its folder");
        assert_eq!((s[2].node, s[2].ring), (1, 1));
        let g = Geometry::fit(pos2(100.0, 100.0), 90.0);
        // Right of the middle, in the first ring: the folder (3 o'clock).
        assert_eq!(hit(&s, &g, pos2(100.0 + g.hub + 2.0, 100.0)), Some(0));
        // Left, first ring: the file (9 o'clock is within 270°..360°).
        assert_eq!(hit(&s, &g, pos2(100.0 - g.hub - 2.0, 100.0 - 1.0)), Some(2));
        assert_eq!(g.at(pos2(100.0, 100.0)), Some((0, 0.0)));
        assert_eq!(g.at(pos2(300.0, 100.0)), None);
    }

    #[test]
    fn tiny_slices_are_left_out() {
        let t = tree(&[(1_000_000, false, 0), (10, false, 0)]);
        assert_eq!(layout(&t, ROOT, &Palette::light()).len(), 1);
    }
}
