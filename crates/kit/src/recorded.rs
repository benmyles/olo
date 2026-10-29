//! What a painting added, kept and shown again moved: for something
//! that looks the same frame after frame but moves, a card in a list
//! being scrolled, where painting it again (measuring and laying out
//! its text, finding its icon) is most of a frame's work. A History
//! full of cards drew its 200 in 1.6 ms; shown again, in a fraction of
//! that.
//!
//! The caller says when what it recorded is stale (its look changed, an
//! icon it showed finished loading: [`crate::icons::loaded`]) and paints
//! afresh while AccessKit is on, whose nodes only a painting adds.

use egui::{LayerId, Painter, Pos2, Rect, Shape};

/// The shapes a painting added to its layer, from where it was painted.
#[derive(Clone, Default)]
pub struct Recorded {
    at: Pos2,
    /// Each with its clip within the painting, or `Rect::EVERYTHING`
    /// where it had none of its own.
    shapes: Vec<(Rect, Shape)>,
}

impl Recorded {
    /// Paints with `paint` on (a copy of) `painter`, as painted from
    /// `at`, keeping what it adds. Its clips within the painting are
    /// kept; `painter`'s own (what of a list shows) clips it this frame
    /// only.
    pub fn record(painter: &Painter, at: Pos2, paint: impl FnOnce(&Painter)) -> Self {
        let (ctx, layer, clip) = (painter.ctx(), painter.layer_id(), painter.clip_rect());
        let first = ctx.graphics(|g| g.get(layer).map_or(0, |l| l.all_entries().len()));
        let mut unclipped = painter.clone();
        unclipped.set_clip_rect(Rect::EVERYTHING);
        paint(&unclipped);
        let shapes = ctx.graphics_mut(|g| {
            let list = g.entry(layer);
            let shapes: Vec<(Rect, Shape)> = list.all_entries().skip(first).map(|c| (c.clip_rect, c.shape.clone())).collect();
            for k in 0..shapes.len() {
                list.mutate_shape(egui::layers::ShapeIdx(first + k), |c| c.clip_rect = c.clip_rect.intersect(clip));
            }
            shapes
        });
        Self { at, shapes }
    }

    /// Shows it again on `painter`'s layer within its clip, painted from
    /// `at` this time. The colours are as recorded (an opacity `painter`
    /// has isn't applied again).
    pub fn replay(&self, painter: &Painter, at: Pos2) {
        self.replay_on(painter.ctx(), painter.layer_id(), painter.clip_rect(), at);
    }

    fn replay_on(&self, ctx: &egui::Context, layer: LayerId, clip: Rect, at: Pos2) {
        let d = at - self.at;
        ctx.graphics_mut(|g| {
            let list = g.entry(layer);
            for (c, shape) in &self.shapes {
                let c = if *c == Rect::EVERYTHING { clip } else { c.translate(d).intersect(clip) };
                let mut shape = shape.clone();
                shape.translate(d);
                list.add(c, shape);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shown again moved, a painting adds what painting it there would.
    #[test]
    fn replayed_as_painted_there() {
        let ctx = egui::Context::default();
        let layer = LayerId::background();
        let paint = |p: &Painter, at: Pos2| {
            p.rect_filled(Rect::from_min_size(at, egui::vec2(40.0, 20.0)), 4.0, egui::Color32::RED);
            p.with_clip_rect(Rect::from_min_size(at, egui::vec2(10.0, 10.0))).circle_filled(at + egui::vec2(5.0, 5.0), 8.0, egui::Color32::BLUE);
        };
        let mut recorded = None;
        let mut fresh = Vec::new();
        let mut replayed = Vec::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
            let p = egui::Painter::new(ui.ctx().clone(), layer, view);
            recorded = Some(Recorded::record(&p, Pos2::new(10.0, 10.0), |p| paint(p, Pos2::new(10.0, 10.0))));
            let n = ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.all_entries().len()));
            paint(&p, Pos2::new(30.0, 50.0));
            let m = ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.all_entries().len()));
            recorded.as_ref().unwrap().replay(&p, Pos2::new(30.0, 50.0));
            ui.ctx().graphics(|g| {
                let all: Vec<_> = g.get(layer).unwrap().all_entries().map(|c| (c.clip_rect, format!("{:?}", c.shape))).collect();
                fresh = all[n..m].to_vec();
                replayed = all[m..].to_vec();
            });
        });
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh, replayed);
    }
}
