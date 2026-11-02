//! The canvas on the egui kit's painter (Preferences ▸ Screen Saver's
//! monitor, until Preferences moves to Olo's GPUI).

use olo_kit::{
    big_text,
    egui::{self, Align2, FontId, Mesh, Painter, Shape, Stroke, TextureId, epaint::Vertex},
    icons, style,
};

use crate::{Anchor, Canvas, Color32, Pos2, Rect, Vec2, vec2};

fn c(color: Color32) -> egui::Color32 {
    let [r, g, b, a] = color.0;
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn p(p: Pos2) -> egui::Pos2 {
    egui::pos2(p.x, p.y)
}

fn r(r: Rect) -> egui::Rect {
    egui::Rect::from_min_max(p(r.min), p(r.max))
}

/// An egui painter, clipped to the frame, and the desktop's texture with
/// its size, when there is one.
pub struct EguiCanvas<'a> {
    pub painter: Painter,
    pub desktop: Option<(TextureId, egui::Vec2)>,
    _frame: std::marker::PhantomData<&'a ()>,
}

impl<'a> EguiCanvas<'a> {
    pub fn new(painter: &'a Painter, clip: Rect, desktop: Option<(TextureId, egui::Vec2)>) -> EguiCanvas<'a> {
        EguiCanvas { painter: painter.with_clip_rect(self::r(clip)), desktop, _frame: std::marker::PhantomData }
    }
}

impl Canvas for EguiCanvas<'_> {
    fn rect_filled(&self, rect: Rect, radius: f32, color: Color32) {
        self.painter.rect_filled(r(rect), radius, c(color));
    }

    fn line_segment(&self, a: Pos2, b: Pos2, width: f32, color: Color32) {
        self.painter.line_segment([p(a), p(b)], Stroke::new(width, c(color)));
    }

    fn polyline(&self, points: &[Pos2], width: f32, color: Color32) {
        self.painter.add(Shape::line(points.iter().copied().map(p).collect(), Stroke::new(width, c(color))));
    }

    fn circle_filled(&self, center: Pos2, radius: f32, color: Color32) {
        self.painter.circle_filled(p(center), radius, c(color));
    }

    fn circle_stroke(&self, center: Pos2, radius: f32, width: f32, color: Color32) {
        self.painter.circle_stroke(p(center), radius, Stroke::new(width, c(color)));
    }

    fn triangles(&self, triangles: &[[Pos2; 3]], color: Color32) {
        let mut mesh = Mesh::default();
        for t in triangles {
            let base = mesh.vertices.len() as u32;
            for q in t {
                mesh.colored_vertex(p(*q), c(color));
            }
            mesh.add_triangle(base, base + 1, base + 2);
        }
        self.painter.add(Shape::mesh(mesh));
    }

    fn big_text(&self, word: &str, size: f32, at: Pos2, anchor: Anchor, color: Color32) {
        // Hundreds of pixels high: made on a worker and shown once it is.
        if let Some(text) = big_text::get(self.painter.ctx(), word, &FontId::new(size, style::bold())) {
            let align = match anchor {
                Anchor::Center => Align2::CENTER_CENTER,
                Anchor::LeftCenter => Align2::LEFT_CENTER,
            };
            text.paint(&self.painter, align.anchor_size(p(at), text.size()).min, c(color));
        }
    }

    fn measure(&self, text: &str, size: f32, width: f32) -> Vec2 {
        let galley = self.painter.layout(text.to_owned(), style::mono_font(size), egui::Color32::WHITE, width);
        vec2(galley.size().x, galley.size().y)
    }

    fn text(&self, at: Pos2, text: &str, size: f32, width: f32, color: Color32) {
        let galley = self.painter.layout(text.to_owned(), style::mono_font(size), c(color), width);
        self.painter.galley(p(at), galley, c(color));
    }

    fn icon(&self, rect: Rect, name: &str, alpha: f32) {
        icons::paint_tinted(&self.painter, r(rect), name, egui::Color32::from_white_alpha((alpha * 255.0) as u8));
    }

    fn has_desktop(&self) -> bool {
        self.desktop.is_some()
    }

    fn desktop(&self, rect: Rect) {
        if let Some((tex, _)) = self.desktop {
            let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            self.painter.image(tex, r(rect), full, egui::Color32::WHITE);
        }
    }

    fn desktop_triangles(&self, triangles: &[[(Pos2, Pos2); 3]], light: f32) {
        let Some((tex, _)) = self.desktop else { return };
        let tint = egui::Color32::from_gray((light.clamp(0.0, 1.0) * 255.0) as u8);
        let mut mesh = Mesh::with_texture(tex);
        for t in triangles {
            let base = mesh.vertices.len() as u32;
            for (pos, uv) in t {
                mesh.vertices.push(Vertex { pos: p(*pos), uv: p(*uv), color: tint });
            }
            mesh.add_triangle(base, base + 1, base + 2);
        }
        self.painter.add(Shape::mesh(mesh));
    }

    fn pixel(&self) -> f32 {
        1.0 / self.painter.ctx().pixels_per_point()
    }
}
