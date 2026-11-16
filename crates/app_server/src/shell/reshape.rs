//! Reshaping auto-tiled trees, as tiling window managers do (bspwm's
//! rotate, flip and balance; i3's and Hyprland's toggled split; yabai's
//! `--rotate`, `--mirror`, `--balance`): each a function of the tree
//! alone, so it can be tested on its own and Undo takes it back as any
//! other change of tiles.
//!
//! - **Rotate** (⌃⌥⌘R, ⌃⌥⇧⌘R the other way): the whole arrangement turned
//!   a quarter, clockwise or not: side by side becomes one above the other.
//! - **Flip** (⌃⌥⌘X left and right, ⌃⌥⌘Y top and bottom): mirrored.
//! - **Even out** (⌃⌥⌘=): every run of tiles side by side (or one above
//!   the other) made as big as each other, the arrangement kept: dragged
//!   edges undone without building it again.
//! - **Toggle split** (⌃⌥⌘S): the split a window shares with its
//!   neighbour turned the other way (beside it ↔ above or below it).
//!
//! Main and Stack keeps its shape by its main window's place, not by its
//! tree: there rotating and flipping move the place
//! ([`rotated_place`], [`flipped_place`]), and evening out builds it again
//! with the place's own share.

use olo_protocol::Tile;

use super::autotile::{Cut, Node};

/// `node` turned a quarter, clockwise (`clockwise`) or not. A split's
/// first side (left, top) goes where it turns to: clockwise, the left
/// goes to the top and the top to the right.
pub fn rotate(node: &Node, clockwise: bool) -> Node {
    match node {
        Node::Split { cut, ratio, a, b } => {
            let (a, b) = (rotate(a, clockwise), rotate(b, clockwise));
            match (cut, clockwise) {
                // Left → top; top → right (the bottom to the left).
                (Cut::Columns, true) => Node::split(Cut::Rows, *ratio, a, b),
                (Cut::Rows, true) => Node::split(Cut::Columns, 1.0 - *ratio, b, a),
                // Left → bottom (the right to the top); top → left.
                (Cut::Columns, false) => Node::split(Cut::Rows, 1.0 - *ratio, b, a),
                (Cut::Rows, false) => Node::split(Cut::Columns, *ratio, a, b),
            }
        }
        leaf => leaf.clone(),
    }
}

/// `node` mirrored: left and right swapped (`across`), else top and
/// bottom.
pub fn flip(node: &Node, across: bool) -> Node {
    match node {
        Node::Split { cut, ratio, a, b } => {
            let (a, b) = (flip(a, across), flip(b, across));
            let mirrored = if across { Cut::Columns } else { Cut::Rows };
            if *cut == mirrored { Node::split(*cut, 1.0 - *ratio, b, a) } else { Node::split(*cut, *ratio, a, b) }
        }
        leaf => leaf.clone(),
    }
}

/// How many tiles side by side (or one above the other, as `cut`) `node`
/// is: a split the same way counts its two sides', anything else one; a
/// place kept that takes no room none.
fn run_of(node: &Node, cut: Cut) -> f32 {
    match node {
        Node::Split { cut: c, a, b, .. } if *c == cut => run_of(a, cut) + run_of(b, cut),
        Node::Kept(k) if !k.holds => 0.0,
        _ => 1.0,
    }
}

/// `node` with every run of tiles the same way as big as each other (each
/// split's ratio its first side's share of the run): the arrangement
/// kept, the edges dragged put back. A split beside a place that takes
/// no room keeps its ratio.
pub fn even_out(node: &Node) -> Node {
    match node {
        Node::Split { cut, ratio, a, b } => {
            let (na, nb) = (run_of(a, *cut), run_of(b, *cut));
            let ratio = if na + nb > 0.0 && na > 0.0 && nb > 0.0 { na / (na + nb) } else { *ratio };
            Node::split(*cut, ratio, even_out(a), even_out(b))
        }
        leaf => leaf.clone(),
    }
}

/// `node` with the split that holds window `id` as one of its own two
/// sides turned the other way (its ratio kept). Whether there was one.
pub fn toggle_split(node: &mut Node, id: u64) -> bool {
    let Node::Split { cut, a, b, .. } = node else { return false };
    if **a == Node::Window(id) || **b == Node::Window(id) {
        *cut = match cut {
            Cut::Columns => Cut::Rows,
            Cut::Rows => Cut::Columns,
        };
        return true;
    }
    toggle_split(a, id) || toggle_split(b, id)
}

/// Main and Stack's main place turned a quarter (as [`rotate`] turns a
/// tree): None where there's no such place (the thirds side by side, the
/// middle two thirds), which stays as it is.
pub fn rotated_place(place: Tile, clockwise: bool) -> Option<Tile> {
    use Tile::*;
    let round: [&[Tile]; 3] = [&[Left, Top, Right, Bottom], &[LeftTwoThirds, TopTwoThirds, RightTwoThirds, BottomTwoThirds], &[TopLeft, TopRight, BottomRight, BottomLeft]];
    round.iter().find_map(|r| {
        let i = r.iter().position(|t| *t == place)?;
        Some(if clockwise { r[(i + 1) % r.len()] } else { r[(i + r.len() - 1) % r.len()] })
    })
}

/// Main and Stack's main place mirrored (left and right, `across`, else
/// top and bottom); the same place where it's its own mirror.
pub fn flipped_place(place: Tile, across: bool) -> Tile {
    use Tile::*;
    let pairs: &[(Tile, Tile)] = if across {
        &[(Left, Right), (LeftThird, RightThird), (LeftTwoThirds, RightTwoThirds), (TopLeft, TopRight), (BottomLeft, BottomRight)]
    } else {
        &[(Top, Bottom), (TopTwoThirds, BottomTwoThirds), (TopLeft, BottomLeft), (TopRight, BottomRight)]
    };
    pairs
        .iter()
        .find_map(|(x, y)| {
            if *x == place {
                Some(*y)
            } else if *y == place {
                Some(*x)
            } else {
                None
            }
        })
        .unwrap_or(place)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::autotile::{Inset, layout};
    use crate::shell::tiling::Part;

    fn w(id: u64) -> Node {
        Node::Window(id)
    }

    fn part(t: &Node, id: u64) -> Part {
        layout(t, &|_| (0.0, 0.0), Inset::default()).into_iter().find(|(n, _)| **n == w(id)).unwrap().1
    }

    fn near(p: Part, q: Part) -> bool {
        (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4 && (p.2 - q.2).abs() < 1e-4 && (p.3 - q.3).abs() < 1e-4
    }

    /// 1 the left 0.6, 2 above 3 (0.3) on the right.
    fn three() -> Node {
        Node::split(Cut::Columns, 0.6, w(1), Node::split(Cut::Rows, 0.3, w(2), w(3)))
    }

    /// Where `p` goes, turned a quarter clockwise in a unit square.
    fn turned(p: Part) -> Part {
        (1.0 - p.1 - p.3, p.0, p.3, p.2)
    }

    #[test]
    fn rotating_and_flipping() {
        let t = three();
        let cw = rotate(&t, true);
        for id in 1..=3 {
            assert!(near(part(&cw, id), turned(part(&t, id))), "{id}: {:?} not {:?}", part(&cw, id), turned(part(&t, id)));
        }
        // Four quarters: back where it was; one each way: the same.
        let round = (0..4).fold(t.clone(), |n, _| rotate(&n, true));
        let back = rotate(&cw, false);
        for id in 1..=3 {
            assert!(near(part(&round, id), part(&t, id)) && near(part(&back, id), part(&t, id)), "{id}");
        }
        // Flipped left and right: 1 on the right 0.6; twice: as it was.
        let f = flip(&t, true);
        assert!(near(part(&f, 1), (0.4, 0.0, 0.6, 1.0)), "{:?}", part(&f, 1));
        assert!(near(part(&f, 2), (0.0, 0.0, 0.4, 0.3)), "{:?}", part(&f, 2));
        let f = flip(&t, false);
        assert!(near(part(&f, 1), part(&t, 1)) && near(part(&f, 3), (0.6, 0.0, 0.4, 0.7)), "{:?}", part(&f, 3));
        assert!(near(part(&flip(&flip(&t, true), true), 2), part(&t, 2)));
    }

    #[test]
    fn evening_out_and_toggling() {
        // Four columns dragged about: each a quarter again.
        let cols = Node::split(Cut::Columns, 0.1, w(1), Node::split(Cut::Columns, 0.7, w(2), Node::split(Cut::Columns, 0.2, w(3), w(4))));
        let e = even_out(&cols);
        for id in 1..=4 {
            assert!((part(&e, id).2 - 0.25).abs() < 1e-4, "{id}: {:?}", part(&e, id));
        }
        // A grid's rows as tall as each other, a row's cells as wide.
        let grid = Node::split(Cut::Rows, 0.8, Node::split(Cut::Columns, 0.3, w(1), Node::split(Cut::Columns, 0.5, w(2), w(3))), Node::split(Cut::Columns, 0.9, w(4), w(5)));
        let e = even_out(&grid);
        assert!(near(part(&e, 1), (0.0, 0.0, 1.0 / 3.0, 0.5)) && near(part(&e, 5), (0.5, 0.5, 0.5, 0.5)), "{:?} {:?}", part(&e, 1), part(&e, 5));
        // Toggled: 2 and 3 side by side; 1's split (with the column) too.
        let mut t = three();
        assert!(toggle_split(&mut t, 3));
        assert!(near(part(&t, 2), (0.6, 0.0, 0.4 * 0.3, 1.0)), "{:?}", part(&t, 2));
        assert!(toggle_split(&mut t, 1));
        assert!(near(part(&t, 1), (0.0, 0.0, 1.0, 0.6)), "{:?}", part(&t, 1));
        assert!(!toggle_split(&mut w(9), 9), "a lone window has no split");
    }

    #[test]
    fn places_turned_and_mirrored() {
        use Tile::*;
        assert_eq!(rotated_place(Left, true), Some(Top));
        assert_eq!(rotated_place(Left, false), Some(Bottom));
        assert_eq!(rotated_place(LeftTwoThirds, true), Some(TopTwoThirds));
        assert_eq!(rotated_place(BottomLeft, true), Some(TopLeft));
        assert_eq!(rotated_place(CenterTwoThirds, true), None);
        for t in Tile::CHOOSABLE {
            if let Some(r) = rotated_place(t, true) {
                assert_eq!(rotated_place(r, false), Some(t));
            }
            assert_eq!(flipped_place(flipped_place(t, true), true), t);
            assert_eq!(flipped_place(flipped_place(t, false), false), t);
        }
        assert_eq!(flipped_place(LeftTwoThirds, true), RightTwoThirds);
        assert_eq!(flipped_place(TopLeft, false), BottomLeft);
        assert_eq!(flipped_place(CenterThird, true), CenterThird);
    }
}
