//! Windows by direction, as tiling window managers move about (i3, sway,
//! bspwm, Hyprland, Amethyst; Ben, 2026-11-15: "look to the other most
//! popular tiling WMs to make sure we support all the stuff they do"):
//!
//! - **Focus** (⌃⌥⌘ H J K L): the window beside the focused one that way
//!   takes the keyboard and comes to the front, in any windowing mode:
//!   from the windows' frames on screen ([`nearest`]), so past the edge
//!   of a display it goes on to the next display that way.
//! - **Swap** (⌃⌥⇧⌘ H J K L): an auto-tiled window trades tiles with the
//!   one beside it that way (Main and Stack's main window too: the other
//!   is the main one then).
//! - **Resize** (⌃⌥⇧⌘ arrows): the edge beside an auto-tiled window that
//!   way moves that way a step ([`resize_split`]): its own edge on that
//!   side (it grows), else, at the display's edge, its edge on the other
//!   side (it gives room back), as the boundary between it and its
//!   neighbour follows the arrow. Least sizes hold (the tree is kept as
//!   it's laid out, so a press past them doesn't pile up unseen).

use olo_protocol::Dir;

use super::autotile::{Cut, Node};
use super::tiling::Part;

/// The rectangle beside `from` in direction `dir`, among `others` (each
/// an id and its rectangle, `from` among them or not): the nearest whose
/// near edge is past `from`'s far edge that way (or within half of it,
/// for rectangles that overlap a little), preferring those that overlap
/// `from` across the other axis (a tile beside it, not one diagonally
/// off), then the least distance between their edges, then the nearest
/// middles across.
pub fn nearest(others: &[(u64, Part)], from: Part, dir: Dir) -> Option<u64> {
    let (fx, fy, fw, fh) = from;
    let centre = (fx + fw / 2.0, fy + fh / 2.0);
    let mut best: Option<(u64, (bool, f32, f32))> = None;
    for &(id, (x, y, w, h)) in others {
        if (x, y, w, h) == from {
            continue;
        }
        let c = (x + w / 2.0, y + h / 2.0);
        // How far past `from` it is that way, edge to edge, and whether
        // it's that way at all (its middle past `from`'s).
        let (gap, beyond, overlap) = match dir {
            Dir::Left => (fx - (x + w), c.0 < centre.0, y < fy + fh && y + h > fy),
            Dir::Right => (x - (fx + fw), c.0 > centre.0, y < fy + fh && y + h > fy),
            Dir::Up => (fy - (y + h), c.1 < centre.1, x < fx + fw && x + w > fx),
            Dir::Down => (y - (fy + fh), c.1 > centre.1, x < fx + fw && x + w > fx),
        };
        // Past `from`'s edge that way (overlapping it by less than half
        // of the smaller of the two along that axis: floating windows
        // overlap), not merely with its middle further that way: a tile
        // beside a full-height one isn't above it.
        let along = match dir {
            Dir::Left | Dir::Right => w.min(fw),
            Dir::Up | Dir::Down => h.min(fh),
        };
        if !beyond || gap < -along / 2.0 {
            continue;
        }
        let across = match dir {
            Dir::Left | Dir::Right => (c.1 - centre.1).abs(),
            Dir::Up | Dir::Down => (c.0 - centre.0).abs(),
        };
        // (Overlapping first: `false` sorts first, so keyed by `!overlap`.)
        let key = (!overlap, gap.max(0.0), across);
        if best.as_ref().is_none_or(|(_, b)| key < *b) {
            best = Some((id, key));
        }
    }
    best.map(|(id, _)| id)
}

/// The path from `node` to window `id`'s leaf: each split on the way and
/// whether the leaf is in its `b`.
fn path_to(node: &Node, id: u64) -> Option<Vec<bool>> {
    match node {
        Node::Window(w) if *w == id => Some(Vec::new()),
        Node::Split { a, b, .. } => {
            if let Some(mut p) = path_to(a, id) {
                p.insert(0, false);
                Some(p)
            } else if let Some(mut p) = path_to(b, id) {
                p.insert(0, true);
                Some(p)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The split at `path` in `node`, with the fraction of the whole area its
/// room spans along its cut (its ratio as it is: least sizes aside).
fn split_at<'a>(node: &'a mut Node, path: &[bool], span: (f32, f32)) -> Option<(&'a mut Node, (f32, f32))> {
    if path.is_empty() {
        return Some((node, span));
    }
    let Node::Split { cut, ratio, a, b } = node else { return None };
    let (w, h) = span;
    let (sa, sb) = match cut {
        Cut::Columns => ((w * *ratio, h), (w * (1.0 - *ratio), h)),
        Cut::Rows => ((w, h * *ratio), (w, h * (1.0 - *ratio))),
    };
    if path[0] { split_at(b, &path[1..], sb) } else { split_at(a, &path[1..], sa) }
}

/// Move the edge beside window `id` in `tree` that way by `step` (a
/// fraction of the whole area along that axis): the nearest split across
/// that axis with the window on the side the edge is on (its own edge
/// that way), else the nearest with it on the other side (its edge the
/// other way, which then moves that way too). Whether a split moved; its
/// ratio kept within 5% and 95%.
pub fn resize_split(tree: &mut Node, id: u64, dir: Dir, step: f32) -> bool {
    let Some(path) = path_to(tree, id) else { return false };
    let want_cut = match dir {
        Dir::Left | Dir::Right => Cut::Columns,
        Dir::Up | Dir::Down => Cut::Rows,
    };
    // The edge that way first: the window in `a` (its far edge is the
    // split) for Right and Down, in `b` for Left and Up.
    let toward_end = matches!(dir, Dir::Right | Dir::Down);
    let cuts: Vec<(usize, Cut, bool)> = (0..path.len())
        .filter_map(|depth| {
            let (n, _) = split_at(tree, &path[..depth], (1.0, 1.0))?;
            let Node::Split { cut, .. } = n else { return None };
            Some((depth, *cut, path[depth]))
        })
        .collect();
    let deepest = |own_side: bool| cuts.iter().rev().find(|(_, c, in_b)| *c == want_cut && (*in_b != toward_end) == own_side).map(|(d, ..)| *d);
    let Some(depth) = deepest(true).or_else(|| deepest(false)) else { return false };
    let Some((node, span)) = split_at(tree, &path[..depth], (1.0, 1.0)) else { return false };
    let Node::Split { ratio, .. } = node else { return false };
    let room = if want_cut == Cut::Columns { span.0 } else { span.1 };
    let delta = step / room.max(1e-3);
    let was = *ratio;
    *ratio = (if toward_end { *ratio + delta } else { *ratio - delta }).clamp(0.05, 0.95);
    (*ratio - was).abs() > 1e-6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: u64) -> Node {
        Node::Window(id)
    }

    /// Three tiles: 1 the left half, 2 above 3 on the right.
    fn three() -> Node {
        Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Rows, 0.5, w(2), w(3)))
    }

    fn parts(t: &Node) -> Vec<(u64, Part)> {
        super::super::autotile::layout(t, &|_| (0.0, 0.0), Default::default()).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect()
    }

    fn part(t: &Node, id: u64) -> Part {
        parts(t).into_iter().find(|(i, _)| *i == id).unwrap().1
    }

    #[test]
    fn nearest_by_direction() {
        let t = three();
        let p = parts(&t);
        assert_eq!(nearest(&p, part(&t, 1), Dir::Right), Some(2), "beside it, the nearer middle across first (2 and 3 tie on overlap and gap)");
        assert_eq!(nearest(&p, part(&t, 2), Dir::Left), Some(1));
        assert_eq!(nearest(&p, part(&t, 2), Dir::Down), Some(3));
        assert_eq!(nearest(&p, part(&t, 3), Dir::Up), Some(2));
        assert_eq!(nearest(&p, part(&t, 1), Dir::Left), None, "nothing that way");
        assert_eq!(nearest(&p, part(&t, 2), Dir::Up), None);
        assert_eq!(nearest(&p, part(&t, 1), Dir::Up), None, "2 is beside 1, not above it");
        assert_eq!(nearest(&p, part(&t, 1), Dir::Down), None);
        // Overlapping across beats nearer but diagonal.
        let others = [(10, (0.6, 0.0, 0.2, 0.2)), (11, (0.9, 0.4, 0.1, 0.2))];
        assert_eq!(nearest(&others, (0.0, 0.4, 0.5, 0.2), Dir::Right), Some(11), "11 is level with it; 10 nearer but above");
        // Frames on two displays side by side: on to the next display.
        let screens = [(20, (0.0, 0.0, 1920.0, 1200.0)), (21, (1920.0, 100.0, 2560.0, 1400.0))];
        assert_eq!(nearest(&screens, (0.0, 0.0, 1920.0, 1200.0), Dir::Right), Some(21));
        // Floating windows overlapping a little: still that way.
        let floats = [(30, (100.0, 100.0, 400.0, 300.0)), (31, (450.0, 150.0, 400.0, 300.0))];
        assert_eq!(nearest(&floats, (100.0, 100.0, 400.0, 300.0), Dir::Right), Some(31));
        assert_eq!(nearest(&floats, (450.0, 150.0, 400.0, 300.0), Dir::Left), Some(30));
    }

    #[test]
    fn resize_moves_the_edge_that_way() {
        // 1 on the left: Right grows it (its own right edge), Left gives
        // room back (no edge on its left: its right edge moves left).
        let mut t = three();
        assert!(resize_split(&mut t, 1, Dir::Right, 0.1));
        assert!((part(&t, 1).2 - 0.6).abs() < 1e-4, "{:?}", part(&t, 1));
        assert!(resize_split(&mut t, 1, Dir::Left, 0.1));
        assert!((part(&t, 1).2 - 0.5).abs() < 1e-4);
        // 2 on the right, top: Left grows it leftwards (the edge between
        // the halves); Down grows it downwards; Up gives room back.
        let mut t = three();
        assert!(resize_split(&mut t, 2, Dir::Left, 0.1));
        assert!((part(&t, 2).0 - 0.4).abs() < 1e-4, "{:?}", part(&t, 2));
        assert!(resize_split(&mut t, 2, Dir::Down, 0.1));
        assert!((part(&t, 2).3 - 0.6).abs() < 1e-4, "{:?}", part(&t, 2));
        assert!(resize_split(&mut t, 3, Dir::Up, 0.1), "3's own top edge");
        assert!((part(&t, 3).3 - 0.5).abs() < 1e-4, "{:?}", part(&t, 3));
        // A step is of the whole area: the right column's rows split
        // room half the area wide, the same 0.1 of the height.
        // Nothing that way at all: a lone window.
        let mut lone = w(7);
        assert!(!resize_split(&mut lone, 7, Dir::Right, 0.1));
        // Never past 5% and 95%.
        let mut t = three();
        for _ in 0..30 {
            resize_split(&mut t, 1, Dir::Right, 0.1);
        }
        assert!((part(&t, 1).2 - 0.95).abs() < 1e-4);
        assert!(!resize_split(&mut t, 1, Dir::Right, 0.1), "at the end: nothing moved");
    }
}
