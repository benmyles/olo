//! Cards of different sizes in columns, packed tight: what a grid of
//! notifications, thumbnails or anything else whose items vary in height
//! (and some in width) uses. [`columns`] fits columns of at least a width
//! into the room there is; each item then says how many columns it spans
//! and how tall it is at that width, and [`arrange`] places them in their
//! order, each where the columns are lowest. A wide item would leave a
//! hole under the shorter of its columns: the next small items that fit
//! go there first (looking a few ahead, so the order hardly changes).
//! [`neighbour`] moves through the result with the arrow keys, by where
//! things are rather than their order.

use egui::{Rect, pos2, vec2};

/// How far ahead [`arrange`] looks for a small item to fill a hole.
const LOOK_AHEAD: usize = 4;

/// How many columns of at least `min` fit in `width` with `gap` between
/// and around them, and how wide each is then (whole points).
pub fn columns(width: f32, min: f32, gap: f32) -> (usize, f32) {
    let n = (((width - gap) / (min + gap)).floor() as usize).max(1);
    (n, ((width - gap) / n as f32 - gap).floor().max(1.0))
}

/// How wide an item spanning `span` columns is.
pub fn span_width(span: usize, column: f32, gap: f32) -> f32 {
    span as f32 * column + (span.max(1) - 1) as f32 * gap
}

/// Where each item goes (in the same order as `items`, which are (span,
/// height) pairs), from (0, 0), with `gap` around them; and the height
/// of the lot. A span wider than `cols` takes them all.
pub fn arrange(cols: usize, column: f32, gap: f32, items: &[(usize, f32)]) -> (Vec<Rect>, f32) {
    let cols = cols.max(1);
    let mut tops = vec![gap; cols];
    let mut out = vec![Rect::NOTHING; items.len()];
    let mut placed = vec![false; items.len()];
    let x = |c: usize| gap + c as f32 * (column + gap);
    for i in 0..items.len() {
        if placed[i] {
            continue;
        }
        let (span, h) = (items[i].0.clamp(1, cols), items[i].1);
        // The lowest place it fits, leftmost if several.
        let (c, y) = (0..=cols - span)
            .map(|c| (c, tops[c..c + span].iter().copied().fold(f32::MIN, f32::max)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, gap));
        // Under a wide one, fill the holes first.
        if span > 1 {
            for (k, top) in tops.iter_mut().enumerate().skip(c).take(span) {
                while let Some(j) = (i + 1..items.len().min(i + 1 + LOOK_AHEAD)).find(|&j| !placed[j] && items[j].0 <= 1 && *top + items[j].1 + gap <= y + 0.5) {
                    out[j] = Rect::from_min_size(pos2(x(k), *top), vec2(column, items[j].1));
                    *top += items[j].1 + gap;
                    placed[j] = true;
                }
            }
        }
        out[i] = Rect::from_min_size(pos2(x(c), y), vec2(span_width(span, column, gap), h));
        for t in &mut tops[c..c + span] {
            *t = y + h + gap;
        }
        placed[i] = true;
    }
    let height = tops.iter().copied().fold(gap, f32::max);
    (out, height)
}

/// How far down every column of an arrangement reaches (`rects` from
/// [`arrange`]): below this, all are still to come. For laying out only
/// as many items as show.
pub fn lowest(cols: usize, column: f32, gap: f32, rects: &[Rect]) -> f32 {
    let cols = cols.max(1);
    let mut bottoms = vec![0.0f32; cols];
    for r in rects.iter().filter(|r| r.is_positive()) {
        let c = (((r.min.x - gap) / (column + gap)).round().max(0.0) as usize).min(cols - 1);
        let span = (((r.width() + gap) / (column + gap)).round().max(1.0) as usize).min(cols - c);
        for b in &mut bottoms[c..c + span] {
            *b = b.max(r.max.y);
        }
    }
    bottoms.into_iter().fold(f32::MAX, f32::min)
}

/// How many items past the last that shows to lay out too, so that
/// [`arrange`] places those that show as it would with them all.
pub const AHEAD: usize = LOOK_AHEAD;

/// A direction to move in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

/// The item an arrow key goes to from `from`: the nearest one that way
/// which lines up with it (overlapping it across the way it moves), the
/// one most in line if several are as near. None at the edge.
pub fn neighbour(rects: &[Rect], from: usize, step: Step) -> Option<usize> {
    let a = *rects.get(from)?;
    let overlap = |lo0: f32, hi0: f32, lo1: f32, hi1: f32| hi0.min(hi1) - lo0.max(lo1);
    rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .filter_map(|(i, b)| {
            // (how far, how far off the line)
            let (gap, across) = match step {
                Step::Down if b.top() >= a.bottom() - 1.0 && overlap(a.left(), a.right(), b.left(), b.right()) > 0.0 => (b.top() - a.bottom(), (b.center().x - a.center().x).abs()),
                Step::Up if b.bottom() <= a.top() + 1.0 && overlap(a.left(), a.right(), b.left(), b.right()) > 0.0 => (a.top() - b.bottom(), (b.center().x - a.center().x).abs()),
                Step::Right if b.left() >= a.right() - 1.0 && overlap(a.top(), a.bottom(), b.top(), b.bottom()) > 0.0 => (b.left() - a.right(), (b.top() - a.top()).abs()),
                Step::Left if b.right() <= a.left() + 1.0 && overlap(a.top(), a.bottom(), b.top(), b.bottom()) > 0.0 => (a.left() - b.right(), (b.top() - a.top()).abs()),
                _ => return None,
            };
            Some((i, gap, across))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1).then(x.2.total_cmp(&y.2)))
        .map(|(i, ..)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_columns() {
        assert_eq!(columns(400.0, 176.0, 8.0), (2, 188.0));
        assert_eq!(columns(100.0, 176.0, 8.0), (1, 84.0));
        assert_eq!(span_width(2, 100.0, 8.0), 208.0);
    }

    #[test]
    fn packs_into_the_lowest_column() {
        // Three columns: a tall one, two short ones, then a fourth goes
        // under the shortest.
        let (r, h) = arrange(3, 100.0, 10.0, &[(1, 200.0), (1, 50.0), (1, 80.0), (1, 40.0)]);
        assert_eq!(r[0].min, pos2(10.0, 10.0));
        assert_eq!(r[1].min, pos2(120.0, 10.0));
        assert_eq!(r[2].min, pos2(230.0, 10.0));
        assert_eq!(r[3].min, pos2(120.0, 70.0), "under the shortest");
        assert_eq!(h, 220.0);
    }

    #[test]
    fn a_wide_one_fills_its_hole_first() {
        // Two columns, one taller; a wide card would leave a hole under
        // the short one, so the small card after it goes there.
        let (r, _) = arrange(2, 100.0, 10.0, &[(1, 100.0), (1, 30.0), (2, 50.0), (1, 60.0)]);
        assert_eq!(r[1].min, pos2(120.0, 10.0));
        assert_eq!(r[3].min, pos2(120.0, 50.0), "into the hole");
        assert_eq!(r[2].min, pos2(10.0, 120.0), "the wide one under both");
        assert_eq!(r[2].width(), 210.0);
        // A wide one in one column is as wide as it.
        let (r, _) = arrange(1, 100.0, 10.0, &[(2, 50.0)]);
        assert_eq!(r[0].width(), 100.0);
    }

    #[test]
    fn arrows_go_by_place() {
        let (r, _) = arrange(2, 100.0, 10.0, &[(2, 50.0), (1, 30.0), (1, 80.0), (1, 30.0)]);
        // The wide one on top, two under it, the fourth under the shorter.
        assert_eq!(neighbour(&r, 0, Step::Down), Some(1), "the nearest, then the leftmost");
        assert_eq!(neighbour(&r, 1, Step::Right), Some(2));
        assert_eq!(neighbour(&r, 1, Step::Down), Some(3));
        assert_eq!(neighbour(&r, 3, Step::Up), Some(1));
        assert_eq!(neighbour(&r, 2, Step::Up), Some(0));
        assert_eq!(neighbour(&r, 0, Step::Up), None);
    }
}
