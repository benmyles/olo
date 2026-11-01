//! Cards of different sizes in columns, packed tight: the layout is
//! `olo_kit_base::masonry` (shared with the GPUI kit); these take and
//! give egui's rectangles.

use egui::{Rect, pos2, vec2};
use olo_kit_base::masonry::Place;
pub use olo_kit_base::masonry::{AHEAD, Step, columns, span_width};

fn rect(p: &Place) -> Rect {
    if p.w <= 0.0 && p.h <= 0.0 { Rect::NOTHING } else { Rect::from_min_size(pos2(p.x, p.y), vec2(p.w, p.h)) }
}

fn place(r: &Rect) -> Place {
    if r.is_positive() { Place { x: r.min.x, y: r.min.y, w: r.width(), h: r.height() } } else { Place::default() }
}

/// Where each item goes (see `olo_kit_base::masonry::arrange`).
pub fn arrange(cols: usize, column: f32, gap: f32, items: &[(usize, f32)]) -> (Vec<Rect>, f32) {
    let (places, h) = olo_kit_base::masonry::arrange(cols, column, gap, items);
    (places.iter().map(rect).collect(), h)
}

/// How far down every column reaches (see `olo_kit_base::masonry::lowest`).
pub fn lowest(cols: usize, column: f32, gap: f32, rects: &[Rect]) -> f32 {
    olo_kit_base::masonry::lowest(cols, column, gap, &rects.iter().map(place).collect::<Vec<_>>())
}

/// The item an arrow key goes to (see `olo_kit_base::masonry::neighbour`).
pub fn neighbour(rects: &[Rect], from: usize, step: Step) -> Option<usize> {
    olo_kit_base::masonry::neighbour(&rects.iter().map(place).collect::<Vec<_>>(), from, step)
}
