//! The canvas on Olo's GPUI (olo-screensaver; Preferences once it moves):
//! everything on the GPU, lines and circles as GPUI's antialiased
//! segments and sectors, the desktop picture and its bent triangles as
//! textured triangles (`Window::paint_image_triangles`).

use std::{cell::RefCell, sync::Arc};

use olo_ui::{
    gpui::{self, App, Bounds, ContentMask, FontWeight, Hsla, PathBuilder, Pixels, RenderImage, SharedString, TextRun, Window, point, px},
    icons, style,
};

use crate::{Anchor, Canvas, Color32, Pos2, Rect, Vec2, vec2};

fn c(color: Color32) -> Hsla {
    let [r, g, b, a] = color.to_f32();
    gpui::Rgba { r, g, b, a }.into()
}

fn p(p: Pos2) -> gpui::Point<Pixels> {
    point(px(p.x), px(p.y))
}

fn b(r: Rect) -> Bounds<Pixels> {
    Bounds::from_corners(p(r.min), p(r.max))
}

/// What a module drew, painted when the frame is done ([`GpuiCanvas::finish`]).
enum Cmd {
    Quad(Bounds<Pixels>, Pixels, Hsla),
    Segment(gpui::Point<Pixels>, gpui::Point<Pixels>, Pixels, bool, Hsla),
    Polyline(Vec<(gpui::Point<Pixels>, gpui::Point<Pixels>)>, Pixels, Hsla),
    Sector(gpui::Point<Pixels>, Pixels, Pixels, Hsla),
    Triangles(Vec<[gpui::Point<Pixels>; 3]>, Hsla),
    Word(SharedString, f32, Pos2, Anchor, Hsla),
    Text(gpui::Point<Pixels>, Vec<gpui::WrappedLine>, Pixels),
    Icon(Bounds<Pixels>, String, f32),
    Desktop(Bounds<Pixels>),
    DesktopTriangles(Vec<[(gpui::Point<Pixels>, gpui::Point<f32>); 3]>, Hsla),
}

impl Cmd {
    /// What GPUI draws it as: a run of the same is painted in one layer.
    fn kind(&self) -> u8 {
        match self {
            Cmd::Quad(..) => 0,
            Cmd::Segment(..) | Cmd::Polyline(..) => 1,
            Cmd::Sector(..) => 2,
            Cmd::Triangles(..) => 3,
            Cmd::Word(..) | Cmd::Text(..) => 4,
            Cmd::Icon(..) | Cmd::Desktop(..) => 5,
            Cmd::DesktopTriangles(..) => 6,
        }
    }
}

/// A window to paint in, clipped to the frame, with the desktop picture
/// when there is one. What a module draws is kept, and painted by
/// [`GpuiCanvas::finish`] a run of the same kind to a layer: GPUI gives
/// each primitive outside a layer a place of its own in its spatial
/// index, which hundreds of long lines crossing each other made
/// quadratic (Lissart's 480 lines at twice egui's CPU, SuperString's at
/// three times); a layer is one place for the whole run, and runs keep
/// the order things were drawn in.
pub struct GpuiCanvas<'a> {
    window: RefCell<&'a mut Window>,
    cx: RefCell<&'a mut App>,
    clip: Bounds<Pixels>,
    desktop: Option<Arc<RenderImage>>,
    cmds: RefCell<Vec<Cmd>>,
}

impl<'a> GpuiCanvas<'a> {
    pub fn new(window: &'a mut Window, cx: &'a mut App, clip: Rect, desktop: Option<Arc<RenderImage>>) -> GpuiCanvas<'a> {
        GpuiCanvas { window: RefCell::new(window), cx: RefCell::new(cx), clip: b(clip), desktop, cmds: RefCell::default() }
    }

    fn push(&self, cmd: Cmd) {
        self.cmds.borrow_mut().push(cmd);
    }

    fn wrapped(&self, text: &str, font_size: f32, width: f32, color: Color32) -> Vec<gpui::WrappedLine> {
        let text = SharedString::from(text.to_owned());
        let run = TextRun { len: text.len(), font: style::mono_font(FontWeight::NORMAL), color: c(color), background_color: None, underline: None, strikethrough: None, size: None };
        let window = self.window.borrow();
        window.text_system().shape_text(text, px(font_size), &[run], Some(px(width)), None).map(|l| l.into_vec()).unwrap_or_default()
    }

    /// Paint the frame.
    pub fn finish(self) {
        let cmds = self.cmds.into_inner();
        let (window, cx) = (self.window.into_inner(), self.cx.into_inner());
        let (clip, desktop) = (self.clip, self.desktop);
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            for run in cmds.chunk_by(|a, b| a.kind() == b.kind()) {
                window.paint_layer(clip, |window| {
                    for cmd in run {
                        paint(cmd, window, cx, desktop.as_ref());
                    }
                });
            }
        });
    }
}

fn paint(cmd: &Cmd, window: &mut Window, cx: &mut App, desktop: Option<&Arc<RenderImage>>) {
    match cmd {
        Cmd::Quad(r, radius, color) => window.paint_quad(gpui::fill(*r, *color).corner_radii(*radius)),
        Cmd::Segment(a, b, width, round, color) => window.paint_segment(*a, *b, *width, *round, *color),
        Cmd::Polyline(pieces, width, color) => window.paint_segments(pieces, *width, true, *color),
        Cmd::Sector(center, inner, outer, color) => window.paint_sector(*center, *inner, *outer, 0.0, std::f32::consts::TAU, *color),
        Cmd::Triangles(triangles, color) => {
            let mut path = PathBuilder::fill();
            for t in triangles {
                path.move_to(t[0]);
                path.line_to(t[1]);
                path.line_to(t[2]);
                path.close();
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, *color);
            }
        }
        Cmd::Word(word, font_size, at, anchor, color) => {
            let align = match anchor {
                Anchor::Center => style::Align::Center,
                Anchor::LeftCenter => style::Align::Left,
            };
            style::label(window, cx, px(at.x), px(at.y), align, word.clone(), *font_size, style::ui_font(FontWeight::BOLD), *color);
        }
        Cmd::Text(at, lines, lh) => {
            let mut y = at.y;
            for line in lines {
                let _ = line.paint(point(at.x, y), *lh, gpui::TextAlign::Left, None, window, cx);
                y += line.size(*lh).height;
            }
        }
        Cmd::Icon(r, name, alpha) => {
            icons::paint_faded(window, *r, name, *alpha);
        }
        Cmd::Desktop(r) => {
            if let Some(image) = desktop {
                let _ = window.paint_image(*r, Default::default(), image.clone(), 0, false);
            }
        }
        Cmd::DesktopTriangles(triangles, tint) => {
            if let Some(image) = desktop {
                let _ = window.paint_image_triangles(image.clone(), triangles, *tint);
            }
        }
    }
}

/// A line's height for text `font_size` high.
fn line_height(font_size: f32) -> Pixels {
    px((font_size * style::LINE_HEIGHT).round())
}

impl Canvas for GpuiCanvas<'_> {
    fn rect_filled(&self, rect: Rect, radius: f32, color: Color32) {
        self.push(Cmd::Quad(b(rect), px(radius), c(color)));
    }

    fn line_segment(&self, a: Pos2, b: Pos2, width: f32, color: Color32) {
        self.push(Cmd::Segment(p(a), p(b), px(width), false, c(color)));
    }

    fn polyline(&self, points: &[Pos2], width: f32, color: Color32) {
        // Round ends, so the joins between the pieces close.
        let pieces = points.windows(2).map(|pair| (p(pair[0]), p(pair[1]))).collect();
        self.push(Cmd::Polyline(pieces, px(width), c(color)));
    }

    fn circle_filled(&self, center: Pos2, radius: f32, color: Color32) {
        self.push(Cmd::Sector(p(center), px(0.0), px(radius), c(color)));
    }

    fn circle_stroke(&self, center: Pos2, radius: f32, width: f32, color: Color32) {
        let (inner, outer) = ((radius - width / 2.0).max(0.0), radius + width / 2.0);
        self.push(Cmd::Sector(p(center), px(inner), px(outer), c(color)));
    }

    fn triangles(&self, triangles: &[[Pos2; 3]], color: Color32) {
        if !triangles.is_empty() {
            self.push(Cmd::Triangles(triangles.iter().map(|t| t.map(p)).collect(), c(color)));
        }
    }

    fn big_text(&self, word: &str, font_size: f32, at: Pos2, anchor: Anchor, color: Color32) {
        self.push(Cmd::Word(SharedString::from(word.to_owned()), font_size, at, anchor, c(color)));
    }

    fn measure(&self, text: &str, font_size: f32, width: f32) -> Vec2 {
        let lh = line_height(font_size);
        self.wrapped(text, font_size, width, Color32::WHITE).iter().fold(vec2(0.0, 0.0), |acc, l| {
            let s = l.size(lh);
            vec2(acc.x.max(f32::from(s.width)), acc.y + f32::from(s.height))
        })
    }

    fn text(&self, at: Pos2, text: &str, font_size: f32, width: f32, color: Color32) {
        let lines = self.wrapped(text, font_size, width, color);
        self.push(Cmd::Text(p(at), lines, line_height(font_size)));
    }

    fn icon(&self, rect: Rect, name: &str, alpha: f32) {
        self.push(Cmd::Icon(b(rect), name.to_owned(), alpha));
    }

    fn has_desktop(&self) -> bool {
        self.desktop.is_some()
    }

    fn desktop(&self, rect: Rect) {
        if self.desktop.is_some() {
            self.push(Cmd::Desktop(b(rect)));
        }
    }

    fn desktop_triangles(&self, triangles: &[[(Pos2, Pos2); 3]], light: f32) {
        if self.desktop.is_none() {
            return;
        }
        let triangles = triangles.iter().map(|t| t.map(|(at, uv)| (p(at), point(uv.x, uv.y)))).collect();
        let light = light.clamp(0.0, 1.0);
        self.push(Cmd::DesktopTriangles(triangles, gpui::Rgba { r: light, g: light, b: light, a: 1.0 }.into()));
    }

    fn pixel(&self) -> f32 {
        1.0 / self.window.borrow().scale_factor()
    }
}

/// The desktop picture (straight RGBA, `width` × `height`) as GPUI draws
/// pictures.
pub fn desktop_image(rgba: Vec<u8>, width: u32, height: u32) -> Option<Arc<RenderImage>> {
    let mut bgra = rgba;
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

#[cfg(test)]
mod tests {
    #[test]
    fn line_heights_are_whole_points() {
        assert_eq!(super::line_height(24.0), olo_ui::gpui::px((24.0 * olo_ui::style::LINE_HEIGHT).round()));
    }
}
