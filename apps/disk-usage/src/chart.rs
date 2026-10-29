//! The ring chart, as Haiku's DiskUsage draws it: the folder in the
//! middle, what's in it in the first ring around it (each the share of
//! the circle its size is of the folder's), what's in those in the next
//! ring, and so on outwards. Each first-ring item has a colour of its own
//! that its contents carry outwards, lighter; files are greyer.

use std::f32::consts::TAU;

use olo_kit::egui::{Color32, Mesh, Painter, Pos2, Shape, Stroke, vec2};
use olo_kit::style::{c, hairline};
use olo_theme::{Hue, Palette, Rgba, mix};

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
    pub color: Color32,
}

/// Where the chart is and how big its rings are.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub center: Pos2,
    /// The middle circle's radius; each ring is `ring` wide.
    pub hub: f32,
    pub ring: f32,
}

impl Geometry {
    pub fn fit(center: Pos2, radius: f32) -> Geometry {
        // The middle a little wider than a ring, as Haiku has it.
        let ring = (radius / (RINGS as f32 + 1.4)).floor();
        Geometry { center, hub: radius - ring * RINGS as f32, ring }
    }

    pub fn inner(&self, ring: usize) -> f32 {
        self.hub + self.ring * (ring - 1) as f32
    }

    /// The ring at `pos` (0 for the middle), and the angle, clockwise from
    /// the top.
    pub fn at(&self, pos: Pos2) -> Option<(usize, f32)> {
        let d = pos - self.center;
        let r = d.length();
        if r <= self.hub {
            return Some((0, 0.0));
        }
        let ring = ((r - self.hub) / self.ring).floor() as usize + 1;
        if ring > RINGS {
            return None;
        }
        Some((ring, angle(d.x, d.y)))
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
pub fn hit(slices: &[Slice], g: &Geometry, pos: Pos2) -> Option<usize> {
    let (ring, a) = g.at(pos)?;
    slices.iter().position(|s| s.ring == ring && a >= s.start && a < s.end)
}

fn point(g: &Geometry, r: f32, a: f32) -> Pos2 {
    g.center + vec2(a.sin(), -a.cos()) * r
}

/// Paint the rings: slices with a hairline between them in `line`, and
/// `hover` lighter.
pub fn paint(painter: &Painter, g: &Geometry, slices: &[Slice], hover: Option<usize>, selected: Option<NodeId>, line: Color32) {
    let mut mesh = Mesh::default();
    for (i, s) in slices.iter().enumerate() {
        let mut color = s.color;
        if Some(i) == hover || Some(s.node) == selected {
            color = Color32::from_rgb(
                (color.r() as u16 + (255 - color.r() as u16) / 3) as u8,
                (color.g() as u16 + (255 - color.g() as u16) / 3) as u8,
                (color.b() as u16 + (255 - color.b() as u16) / 3) as u8,
            );
        }
        let (r0, r1) = (g.inner(s.ring), g.inner(s.ring) + g.ring);
        // Steps of at most ~2° keep the arcs round at any size.
        let steps = ((s.end - s.start) / 0.035).ceil().max(1.0) as usize;
        let base = mesh.vertices.len() as u32;
        for k in 0..=steps {
            let a = s.start + (s.end - s.start) * k as f32 / steps as f32;
            mesh.colored_vertex(point(g, r0, a), color);
            mesh.colored_vertex(point(g, r1, a), color);
        }
        for k in 0..steps as u32 {
            let v = base + k * 2;
            mesh.add_triangle(v, v + 1, v + 3);
            mesh.add_triangle(v, v + 3, v + 2);
        }
    }
    painter.add(Shape::mesh(mesh));
    // Hairlines between slices and round each ring's outer edge, whole
    // device pixels (one at 12 o'clock is straight up).
    let stroke = Stroke::new(hairline(painter.ctx()), line);
    for s in slices {
        let (r0, r1) = (g.inner(s.ring), g.inner(s.ring) + g.ring);
        // Slices thinner than a few pixels share one edge, or a run of
        // small files becomes hatching.
        if (s.end - s.start) * r1 >= 6.0 {
            painter.line_segment([point(g, r0, s.start), point(g, r1, s.start)], stroke);
        }
        if (s.end - s.start) * r1 < 2.0 {
            continue;
        }
        let steps = ((s.end - s.start) / 0.035).ceil().max(1.0) as usize;
        let arc: Vec<Pos2> = (0..=steps).map(|k| point(g, r1, s.start + (s.end - s.start) * k as f32 / steps as f32)).collect();
        painter.add(Shape::line(arc, stroke));
    }
}

/// The middle: the folder itself.
pub fn paint_hub(painter: &Painter, g: &Geometry, fill: Color32, line: Color32) {
    let w = hairline(painter.ctx());
    painter.circle(g.center, g.hub - w / 2.0, fill, Stroke::new(w, line));
}

#[allow(dead_code)]
pub fn to_pos(g: &Geometry, ring: usize, a: f32) -> Pos2 {
    point(g, g.inner(ring) + g.ring / 2.0, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, ROOT};
    use olo_kit::egui::pos2;

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
