//! Olo: the draw order of what a scene paints. Each thing gets one more
//! than the highest order of what it overlaps, so things apart share an
//! order and are drawn in one batch. Upstream kept them in a bounding
//! volume tree that never rebalanced: a frame's things come in reading
//! order, the tree grew towards a chain, and a full screen of cards
//! (Notifications' history: 1,600 things, each line of text one) took 3
//! ms a frame to order. Here they go in a grid of cells, each keeping
//! what overlaps it and its highest order, so a thing looks only at its
//! neighbours, and at a cell's only when that could raise its order.

use crate::{Bounds, ScaledPixels};
use collections::FxHashMap;
use std::fmt::Debug;

/// A cell's side, in device pixels (a few lines of text across).
const CELL: f32 = 256.0;
/// Things far outside any window share the edge cells (their order is
/// still exact: what overlaps is checked, not assumed).
const FAR: i32 = 512;

/// What a cell's coordinate comes from.
pub(crate) trait GridUnit:
    Copy
    + Clone
    + Debug
    + Default
    + PartialEq
    + PartialOrd
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
{
    fn grid(self) -> f32;
}

impl GridUnit for ScaledPixels {
    fn grid(self) -> f32 {
        self.0
    }
}

impl GridUnit for f32 {
    fn grid(self) -> f32 {
        self
    }
}

#[derive(Debug, Default)]
struct Cell {
    /// The highest order of what's in it.
    max: u32,
    /// What overlaps it (indices into `BoundsTree::things`).
    things: Vec<u32>,
}

#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: GridUnit,
{
    things: Vec<(Bounds<U>, u32)>,
    /// Cells with something in them this frame; kept (emptied) from frame
    /// to frame, so a frame like the last allocates nothing.
    cells: FxHashMap<(i32, i32), Cell>,
}

impl<U> BoundsTree<U>
where
    U: GridUnit,
{
    pub fn clear(&mut self) {
        if self.things.is_empty() {
            return;
        }
        self.things.clear();
        for cell in self.cells.values_mut() {
            cell.max = 0;
            cell.things.clear();
        }
    }

    /// The cells `bounds` covers: first and last column, first and last row.
    fn span(bounds: &Bounds<U>) -> (i32, i32, i32, i32) {
        let at = |v: f32| ((v / CELL).floor() as i32).clamp(-FAR, FAR);
        let end = bounds.bottom_right();
        (
            at(bounds.origin.x.grid()),
            at(end.x.grid()),
            at(bounds.origin.y.grid()),
            at(end.y.grid()),
        )
    }

    /// `new_bounds`' order: one more than the highest of what it overlaps.
    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        let (x0, x1, y0, y1) = Self::span(&new_bounds);
        let mut max = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Some(cell) = self.cells.get(&(x, y)) else {
                    continue;
                };
                if cell.max <= max {
                    continue;
                }
                for &i in &cell.things {
                    let (bounds, order) = &self.things[i as usize];
                    if *order > max && bounds.intersects(&new_bounds) {
                        max = *order;
                    }
                }
            }
        }
        let order = max + 1;
        let index = self.things.len() as u32;
        self.things.push((new_bounds, order));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let cell = self.cells.entry((x, y)).or_default();
                cell.things.push(index);
                cell.max = cell.max.max(order);
            }
        }
        order
    }
}

impl<U> Default for BoundsTree<U>
where
    U: GridUnit,
{
    fn default() -> Self {
        BoundsTree {
            things: Vec::new(),
            cells: FxHashMap::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Point, Size};
    use rand::{Rng, SeedableRng};

    #[test]
    fn test_insert() {
        let mut tree = BoundsTree::<f32>::default();
        let bounds1 = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds2 = Bounds {
            origin: Point { x: 5.0, y: 5.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds3 = Bounds {
            origin: Point { x: 10.0, y: 10.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };

        // Insert the bounds into the tree and verify the order is correct
        assert_eq!(tree.insert(bounds1), 1);
        assert_eq!(tree.insert(bounds2), 2);
        assert_eq!(tree.insert(bounds3), 3);

        // Insert non-overlapping bounds and verify they can reuse orders
        let bounds4 = Bounds {
            origin: Point { x: 20.0, y: 20.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds5 = Bounds {
            origin: Point { x: 40.0, y: 40.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds6 = Bounds {
            origin: Point { x: 25.0, y: 25.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        assert_eq!(tree.insert(bounds4), 1); // bounds4 does not overlap with bounds1, bounds2, or bounds3
        assert_eq!(tree.insert(bounds5), 1); // bounds5 does not overlap with any other bounds
        assert_eq!(tree.insert(bounds6), 2); // bounds6 overlaps with bounds4, so it should have a different order
    }

    /// Olo: a frame of cards in columns as Notifications' history paints
    /// it (a ground, then each card's shadow, plate, icon and lines of
    /// text), timed: `cargo test -p gpui --release bounds_tree_cards --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bounds_tree_cards() {
        let r = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: Point { x, y },
            size: Size {
                width: w,
                height: h,
            },
        };
        let mut rects = vec![r(0.0, 0.0, 3456.0, 2234.0), r(0.0, 30.0, 3456.0, 2204.0)];
        for col in 0..6 {
            let x = 10.0 + col as f32 * 570.0;
            let mut y = 40.0;
            while y < 2200.0 {
                rects.push(r(x - 4.0, y - 2.0, 568.0, 128.0));
                rects.push(r(x, y, 560.0, 120.0));
                rects.push(r(x + 8.0, y + 8.0, 32.0, 32.0));
                for line in 0..6 {
                    rects.push(r(x + 48.0, y + 8.0 + line as f32 * 17.0, 480.0, 16.0));
                }
                y += 130.0;
            }
        }
        let t = std::time::Instant::now();
        let mut orders = Vec::new();
        for _ in 0..10 {
            let mut tree = BoundsTree::<f32>::default();
            orders = rects.iter().map(|b| tree.insert(*b)).collect::<Vec<_>>();
        }
        println!(
            "{} rects: {:.3} ms a frame, orders up to {}",
            rects.len(),
            t.elapsed().as_secs_f64() * 100.0,
            orders.iter().max().unwrap()
        );
    }

    #[test]
    fn test_random_iterations() {
        random_iterations(1.0);
        // (Olo: spread over many cells, far outside too.)
        random_iterations(40.0);
        random_iterations(1000.0);
    }

    fn random_iterations(spread: f32) {
        let max_bounds = 100;
        for seed in 1..=1000 {
            // let seed = 44;
            let mut tree = BoundsTree::default();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
            let mut expected_quads: Vec<(Bounds<f32>, u32)> = Vec::new();

            // Insert a random number of random AABBs into the tree.
            let num_bounds = rng.random_range(1..=max_bounds);
            for _ in 0..num_bounds {
                let min_x: f32 = rng.random_range(-100.0..100.0) * spread;
                let min_y: f32 = rng.random_range(-100.0..100.0) * spread;
                let width: f32 = rng.random_range(0.0..50.0) * spread;
                let height: f32 = rng.random_range(0.0..50.0) * spread;
                let bounds = Bounds {
                    origin: Point { x: min_x, y: min_y },
                    size: Size { width, height },
                };

                let expected_ordering = expected_quads
                    .iter()
                    .filter_map(|quad| quad.0.intersects(&bounds).then_some(quad.1))
                    .max()
                    .unwrap_or(0)
                    + 1;
                expected_quads.push((bounds, expected_ordering));

                // Insert the AABB into the tree and collect intersections.
                let actual_ordering = tree.insert(bounds);
                assert_eq!(actual_ordering, expected_ordering);
            }
        }
    }
}
