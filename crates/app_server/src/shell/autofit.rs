//! Fitting auto-tiling's tiles to their windows' least sizes. A tree
//! grows a window at a time (each new one halves the tile that had the
//! focus), so its tiles can end up too small for what is in them, and
//! windows that will go no smaller overlap, where another arrangement
//! would have room for every one. [`refit`] finds that arrangement:
//!
//! 1. the same tiles with some splits turned the other way (side by side
//!    for one above the other), the fewest that will do;
//! 2. else arranged anew: of every way of cutting the room in two, and
//!    each part in two, and so on (with up to [`SUBSETS`] tiles, any
//!    tiles on either side; with more, keeping their order), one where
//!    every window has its least size, found from each group's shapes
//!    (the least widths and heights it can be laid out in, as a
//!    slicing floorplan's shape curve), then built from the top: across
//!    the longer side and as even as it can be, where that fits;
//! 3. where none fits, the arrangement that overlaps least, if that's
//!    less than now.
//!
//! Sizes are fractions of the area (gap included), as `autotile::layout`
//! takes them.

use std::collections::HashMap;

use super::autotile::{Cut, Node, overflow};

type Size = (f32, f32);

/// Small enough to be the same.
const EPS: f32 = 1e-4;
/// Up to this many tiles, any can go with any; with more, they keep
/// their order (the search over every grouping grows as 3ⁿ).
pub const SUBSETS: usize = 8;
/// Most splits turned in step 1 (2ⁿ ways at worst), and the most ways tried.
const FLIPS: usize = 14;
const FLIP_TRIES: usize = 4096;
/// Most shapes kept for a group (beyond, evenly thinned).
const SHAPES: usize = 24;

/// The tree to use instead of `node`, if it doesn't give every window
/// its least size (`min`) and another arrangement does (or overlaps
/// less). `aspect` is the area's width over its height.
pub fn refit(node: &Node, min: &dyn Fn(u64) -> Size, aspect: f32) -> Option<Node> {
    let now = overflow(node, (1.0, 1.0), min);
    if now <= EPS {
        return None;
    }
    if let Some(t) = flipped(node, min) {
        return Some(t);
    }
    let t = arranged(node, min, aspect)?;
    (overflow(&t, (1.0, 1.0), min) < now - EPS).then_some(t)
}

// ---- 1. Splits turned ---------------------------------------------------

fn splits(node: &Node) -> usize {
    match node {
        Node::Split { a, b, .. } => 1 + splits(a) + splits(b),
        _ => 0,
    }
}

/// `node` with the splits marked in `turn` (pre-order) cut the other way.
fn turned(node: &Node, turn: &[bool], i: &mut usize) -> Node {
    match node {
        Node::Split { cut, ratio, a, b } => {
            let flip = turn[*i];
            *i += 1;
            let cut = match (cut, flip) {
                (Cut::Columns, true) => Cut::Rows,
                (Cut::Rows, true) => Cut::Columns,
                (c, false) => *c,
            };
            let a = turned(a, turn, i);
            let b = turned(b, turn, i);
            Node::split(cut, *ratio, a, b)
        }
        n => n.clone(),
    }
}

/// The tree with the fewest splits turned that fits, if one does.
fn flipped(node: &Node, min: &dyn Fn(u64) -> Size) -> Option<Node> {
    let s = splits(node);
    if s == 0 || s > FLIPS {
        return None;
    }
    let mut tries = 0;
    for k in 1..=s {
        // Each choice of k of the s splits, in order.
        let mut pick: Vec<usize> = (0..k).collect();
        loop {
            tries += 1;
            if tries > FLIP_TRIES {
                return None;
            }
            let mut turn = vec![false; s];
            for &p in &pick {
                turn[p] = true;
            }
            let t = turned(node, &turn, &mut 0);
            if overflow(&t, (1.0, 1.0), min) <= EPS {
                return Some(t);
            }
            // The next choice.
            let Some(j) = (0..k).rev().find(|&j| pick[j] < s - k + j) else { break };
            pick[j] += 1;
            for l in j + 1..k {
                pick[l] = pick[l - 1] + 1;
            }
        }
    }
    None
}

// ---- 2. Arranged anew ---------------------------------------------------

/// What takes room: a window's tile or a place held for one, with the
/// places beside it that take none (kept for windows away for now),
/// which go where it goes.
struct Item {
    node: Node,
    min: Size,
}

/// Every group of items' shapes: the least sizes it can be laid out in,
/// none narrower and lower than another.
struct Shapes {
    items: Vec<Item>,
    /// Any items may group (else only ones next to each other).
    any: bool,
    known: HashMap<u32, Vec<Size>>,
}

fn shows(n: &Node) -> bool {
    match n {
        Node::Window(_) => true,
        Node::Kept(k) => k.holds,
        Node::Split { .. } => false,
    }
}

/// The leaves of `node` that take room, each with the ones that don't
/// following it (or, before the first, leading it).
fn items(node: &Node, min: &dyn Fn(u64) -> Size) -> Vec<Item> {
    let mut leaves = Vec::new();
    node.each_leaf(&mut |n| leaves.push(n.clone()));
    let mut out: Vec<Item> = Vec::new();
    let mut before: Vec<Node> = Vec::new();
    for leaf in leaves {
        if shows(&leaf) {
            let size = if let Node::Window(id) = leaf { min(id) } else { (0.0, 0.0) };
            let mut n = leaf;
            for k in before.drain(..).rev() {
                n = Node::split(Cut::Columns, 0.5, k, n);
            }
            out.push(Item { node: n, min: size });
        } else if let Some(last) = out.last_mut() {
            let n = std::mem::replace(&mut last.node, Node::Window(0));
            last.node = Node::split(Cut::Columns, 0.5, n, leaf);
        } else {
            before.push(leaf);
        }
    }
    out
}

/// The shapes that aren't wider and taller than another, narrowest
/// first, at most [`SHAPES`] (the narrowest, the lowest and evenly
/// between).
fn pareto(mut v: Vec<Size>) -> Vec<Size> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<Size> = Vec::new();
    for s in v {
        if out.last().is_none_or(|l| s.1 < l.1 - EPS) {
            out.push(s);
        }
    }
    if out.len() > SHAPES {
        let n = out.len();
        out = (0..SHAPES).map(|k| out[k * (n - 1) / (SHAPES - 1)]).collect();
    }
    out
}

impl Shapes {
    fn new(items: Vec<Item>) -> Shapes {
        let any = items.len() <= SUBSETS;
        Shapes { items, any, known: HashMap::new() }
    }

    fn all(&self) -> u32 {
        if self.items.len() >= 32 { u32::MAX } else { (1u32 << self.items.len()) - 1 }
    }

    /// The ways to cut group `mask` in two: the part with its first
    /// item, any of its others with it (or, keeping order, its first
    /// few).
    fn halves(&self, mask: u32) -> Vec<u32> {
        let low = mask & mask.wrapping_neg();
        if self.any {
            let rest = mask ^ low;
            // Every subset of the rest, with the first item, but not all.
            let mut out = Vec::new();
            let mut sub = rest;
            loop {
                if sub | low != mask {
                    out.push(sub | low);
                }
                if sub == 0 {
                    break;
                }
                sub = (sub - 1) & rest;
            }
            out
        } else {
            // Its first k items, for each k (the group is a run).
            let mut out = Vec::new();
            let mut a = 0;
            let mut m = mask;
            while m != 0 {
                let bit = m & m.wrapping_neg();
                m ^= bit;
                a |= bit;
                if m != 0 {
                    out.push(a);
                }
            }
            out
        }
    }

    fn of(&mut self, mask: u32) -> Vec<Size> {
        if let Some(s) = self.known.get(&mask) {
            return s.clone();
        }
        let s = if mask.count_ones() == 1 {
            vec![self.items[mask.trailing_zeros() as usize].min]
        } else {
            let mut all = Vec::new();
            for a in self.halves(mask) {
                let (sa, sb) = (self.of(a), self.of(mask ^ a));
                for pa in &sa {
                    for pb in &sb {
                        all.push((pa.0 + pb.0, pa.1.max(pb.1)));
                        all.push((pa.0.max(pb.0), pa.1 + pb.1));
                    }
                }
            }
            pareto(all)
        };
        self.known.insert(mask, s.clone());
        s
    }

    /// The least length along `cut` group `mask` needs where it has
    /// `across` the other way, if it fits that.
    fn least(&mut self, mask: u32, cut: Cut, across: f32) -> Option<f32> {
        self.of(mask)
            .into_iter()
            .filter_map(|(w, h)| match cut {
                Cut::Columns => (h <= across + EPS).then_some(w),
                Cut::Rows => (w <= across + EPS).then_some(h),
            })
            .min_by(f32::total_cmp)
    }

    /// Group `mask` as a tree for room `room`: cut across its longer side
    /// (in pixels: `aspect`) where that fits, as evenly as fits (by how
    /// many tiles each side has), its first tiles first.
    fn build(&mut self, mask: u32, room: Size, aspect: f32) -> Node {
        if mask.count_ones() == 1 {
            return self.items[mask.trailing_zeros() as usize].node.clone();
        }
        let n = mask.count_ones() as f32;
        let longer = if room.0 * aspect >= room.1 { Cut::Columns } else { Cut::Rows };
        let other = if longer == Cut::Columns { Cut::Rows } else { Cut::Columns };
        // Evenest first; then the group's first tiles before others.
        let run = |a: u32| first(mask, a.count_ones()) == a;
        let mut halves = self.halves(mask);
        halves.sort_by(|&a, &b| {
            let even = |x: u32| ((x.count_ones() as f32) - n / 2.0).abs();
            even(a).total_cmp(&even(b)).then(run(b).cmp(&run(a))).then(a.cmp(&b))
        });
        let mut fallback = None;
        for cut in [longer, other] {
            let (len, across) = match cut {
                Cut::Columns => (room.0, room.1),
                Cut::Rows => (room.1, room.0),
            };
            for &a in &halves {
                let b = mask ^ a;
                let (Some(la), Some(lb)) = (self.least(a, cut, across), self.least(b, cut, across)) else { continue };
                let share = a.count_ones() as f32 / n;
                if la + lb > len + EPS {
                    fallback.get_or_insert((cut, a, share));
                    continue;
                }
                let at = (share * len).clamp(la, (len - lb).max(la)) / len.max(EPS);
                return self.split(mask, cut, a, at, room, aspect);
            }
        }
        // (Never, in room made for the group's least shape; but always a tree.)
        let (cut, a, share) = fallback.unwrap_or((longer, halves[0], 0.5));
        self.split(mask, cut, a, share, room, aspect)
    }

    fn split(&mut self, mask: u32, cut: Cut, a: u32, ratio: f32, room: Size, aspect: f32) -> Node {
        let (ra, rb) = match cut {
            Cut::Columns => ((room.0 * ratio, room.1), (room.0 * (1.0 - ratio), room.1)),
            Cut::Rows => ((room.0, room.1 * ratio), (room.0, room.1 * (1.0 - ratio))),
        };
        Node::split(cut, ratio, self.build(a, ra, aspect), self.build(mask ^ a, rb, aspect))
    }
}

/// The first `k` of `mask`'s items.
fn first(mask: u32, k: u32) -> u32 {
    let (mut m, mut out) = (mask, 0);
    for _ in 0..k {
        let bit = m & m.wrapping_neg();
        m ^= bit;
        out |= bit;
    }
    out
}

/// `node`'s tiles arranged anew to fit their least sizes (or overlap
/// least): see the module doc.
fn arranged(node: &Node, min: &dyn Fn(u64) -> Size, aspect: f32) -> Option<Node> {
    let items = items(node, min);
    if items.len() < 2 || items.len() > 32 {
        return None;
    }
    let mut shapes = Shapes::new(items);
    let all = shapes.all();
    // The area, or where nothing fits it, the least shape nearest it.
    let over = |s: &Size| (s.0 - 1.0).max(0.0) + (s.1 - 1.0).max(0.0);
    let best = shapes.of(all).into_iter().min_by(|a, b| over(a).total_cmp(&over(b)))?;
    let room = (best.0.max(1.0), best.1.max(1.0));
    Some(shapes.build(all, room, aspect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::autotile::{Kept, Who, layout};

    fn w(id: u64) -> Node {
        Node::Window(id)
    }

    fn mins(list: &[(u64, Size)]) -> impl Fn(u64) -> Size + '_ {
        move |id| list.iter().find(|(i, _)| *i == id).map_or((0.0, 0.0), |(_, s)| *s)
    }

    /// No two windows' tiles, at their least sizes, overlap.
    fn fits(t: &Node, min: &dyn Fn(u64) -> Size) -> bool {
        layout(t, min).iter().all(|(n, p)| match n {
            Node::Window(id) => {
                let m = min(*id);
                p.2 + EPS >= m.0 && p.3 + EPS >= m.1
            }
            _ => true,
        })
    }

    #[test]
    fn a_fitting_tree_is_left_alone() {
        let t = Node::split(Cut::Columns, 0.5, w(1), w(2));
        let m = [(1, (0.3, 0.3)), (2, (0.3, 0.3))];
        assert!(refit(&t, &mins(&m), 16.0 / 9.0).is_none());
    }

    #[test]
    fn a_split_turned_is_enough() {
        // Two windows each wider than half the area, side by side: one
        // above the other fits them.
        let t = Node::split(Cut::Columns, 0.5, w(1), w(2));
        let m = [(1, (0.7, 0.3)), (2, (0.7, 0.3))];
        let f = refit(&t, &mins(&m), 16.0 / 9.0).expect("refitted");
        assert_eq!(f, Node::split(Cut::Rows, 0.5, w(1), w(2)));
        assert!(fits(&f, &mins(&m)));
    }

    #[test]
    fn arranged_anew_where_turning_splits_wont_do() {
        // Each new window halved the last: 1 | (2 / (3 | 4)). Window 1
        // needs most of the width, 2 is wide too; 3 and 4 small. Only
        // 1 and 3 side by side over 2 and 4 fits... or the like.
        let t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Rows, 0.5, w(2), Node::split(Cut::Columns, 0.5, w(3), w(4))));
        let m = [(1, (0.6, 0.45)), (2, (0.6, 0.45)), (3, (0.35, 0.45)), (4, (0.35, 0.45))];
        assert!(!fits(&t, &mins(&m)));
        let f = refit(&t, &mins(&m), 16.0 / 9.0).expect("refitted");
        assert!(fits(&f, &mins(&m)), "{f}");
        let mut ids = f.windows();
        ids.sort();
        assert_eq!(ids, [1, 2, 3, 4]);
    }

    #[test]
    fn where_nothing_fits_the_least_overlap() {
        let t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Columns, 0.5, w(2), w(3)));
        let m = [(1, (0.6, 0.6)), (2, (0.6, 0.6)), (3, (0.6, 0.6))];
        let before = overflow(&t, (1.0, 1.0), &mins(&m));
        let f = refit(&t, &mins(&m), 1.0).expect("less overlap");
        assert!(overflow(&f, (1.0, 1.0), &mins(&m)) < before);
        // And it stays put from there.
        assert!(refit(&f, &mins(&m), 1.0).is_none());
    }

    #[test]
    fn places_held_and_kept_come_along() {
        let kept = Node::Kept(Kept { who: Who::Window(9), holds: false, until: None });
        let t = Node::split(
            Cut::Columns,
            0.5,
            w(1),
            Node::split(Cut::Columns, 0.5, Node::split(Cut::Rows, 0.5, w(2), kept.clone()), Node::split(Cut::Columns, 0.5, w(3), Node::hole())),
        );
        let m = [(1, (0.5, 0.5)), (2, (0.5, 0.5)), (3, (0.5, 0.5))];
        let f = refit(&t, &mins(&m), 1.0).expect("refitted");
        assert!(fits(&f, &mins(&m)), "{f}");
        assert!(f.any_leaf(&|n| *n == kept));
        assert!(f.any_leaf(&|n| *n == Node::hole()));
    }

    #[test]
    fn many_windows_keep_their_order_and_fit() {
        // Twelve windows, each a third of the area's height and a fifth of
        // its width, all in one row: a grid of them fits.
        let mut t = w(1);
        for id in 2..=12 {
            t = Node::split(Cut::Columns, 0.5, t, w(id));
        }
        let m: Vec<(u64, Size)> = (1..=12).map(|id| (id, (0.2, 0.3))).collect();
        let f = refit(&t, &mins(&m), 16.0 / 9.0).expect("refitted");
        assert!(fits(&f, &mins(&m)), "{f}");
        let start = std::time::Instant::now();
        let _ = arranged(&t, &mins(&m), 16.0 / 9.0);
        assert!(start.elapsed() < std::time::Duration::from_millis(50), "{:?}", start.elapsed());
    }

    #[test]
    fn the_most_tiles_it_searches_every_way_quickly() {
        let mut t = w(1);
        for id in 2..=SUBSETS as u64 {
            t = Node::split(Cut::Columns, 0.5, t, w(id));
        }
        let m: Vec<(u64, Size)> = (1..=SUBSETS as u64).map(|id| (id, (0.1 + 0.02 * id as f32, 0.2 + 0.03 * (id % 3) as f32))).collect();
        let start = std::time::Instant::now();
        let f = arranged(&t, &mins(&m), 16.0 / 9.0).expect("arranged");
        let took = start.elapsed();
        assert!(fits(&f, &mins(&m)), "{f}");
        assert!(took < std::time::Duration::from_millis(20), "{took:?}");
    }
}
