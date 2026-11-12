//! The canvas on Olo's own kit (olo-screensaver): everything on the GPU,
//! lines and circles as the Interface Kit's antialiased segments and
//! sectors, the desktop picture and its bent triangles as textured
//! triangles (`Painter::paint_image_triangles`).

use std::{cell::RefCell, sync::Arc};

use olo_kit::{self as kit, Bounds, ContentMask, FontWeight, Hsla, Painter, PathBuilder, Pixels, RenderImage, SharedString, TextRun, icons, point, px, style};

use crate::{Anchor, Canvas, Color32, Pos2, Rect, Vec2, vec2};

fn c(color: Color32) -> Hsla {
    let [r, g, b, a] = color.to_f32();
    kit::Rgba { r, g, b, a }.into()
}

fn p(p: Pos2) -> kit::Point<Pixels> {
    point(px(p.x), px(p.y))
}

fn b(r: Rect) -> Bounds<Pixels> {
    Bounds::from_corners(p(r.min), p(r.max))
}

/// What a module drew, painted when the frame is done ([`KitCanvas::finish`]).
enum Cmd {
    Quad(Bounds<Pixels>, Pixels, Hsla),
    Segment(kit::Point<Pixels>, kit::Point<Pixels>, Pixels, bool, Hsla),
    Polyline(Vec<(kit::Point<Pixels>, kit::Point<Pixels>)>, Pixels, Hsla),
    Sector(kit::Point<Pixels>, Pixels, Pixels, Hsla),
    Triangles(Vec<[kit::Point<Pixels>; 3]>, Hsla),
    Word(SharedString, f32, Pos2, Anchor, Hsla),
    Text(kit::Point<Pixels>, Vec<kit::WrappedLine>, Pixels),
    Icon(Bounds<Pixels>, String, f32),
    Desktop(Bounds<Pixels>),
    DesktopTriangles(Vec<[(kit::Point<Pixels>, kit::Point<f32>); 3]>, Hsla),
}

impl Cmd {
    /// What it's drawn as: a run of the same is painted in one layer.
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

/// A painter to paint with, clipped to the frame, with the desktop
/// picture when there is one. What a module draws is kept, and painted
/// by [`KitCanvas::finish`] a run of the same kind to a layer: each
/// primitive outside a layer has a place of its own in the scene's
/// spatial index, which hundreds of long lines crossing each other made
/// quadratic (Lissart's 480 lines, SuperString's); a layer is one place
/// for the whole run, and runs keep the order things were drawn in.
pub struct KitCanvas<'a, 'p> {
    painter: RefCell<&'a mut Painter<'p>>,
    clip: Bounds<Pixels>,
    desktop: Option<Arc<RenderImage>>,
    cmds: RefCell<Vec<Cmd>>,
}

impl<'a, 'p> KitCanvas<'a, 'p> {
    pub fn new(painter: &'a mut Painter<'p>, clip: Rect, desktop: Option<Arc<RenderImage>>) -> KitCanvas<'a, 'p> {
        KitCanvas { painter: RefCell::new(painter), clip: b(clip), desktop, cmds: RefCell::default() }
    }

    fn push(&self, cmd: Cmd) {
        self.cmds.borrow_mut().push(cmd);
    }

    fn wrapped(&self, text: &str, font_size: f32, width: f32, color: Color32) -> Vec<kit::WrappedLine> {
        let text = SharedString::from(text.to_owned());
        let run = TextRun { len: text.len(), font: style::mono_font(FontWeight::NORMAL), color: c(color), background_color: None, underline: None, strikethrough: None, size: None };
        let g = self.painter.borrow();
        g.text_system().shape_text(text, px(font_size), &[run], Some(px(width)), None).map(|l| l.into_vec()).unwrap_or_default()
    }

    /// Paint the frame.
    pub fn finish(self) {
        let cmds = self.cmds.into_inner();
        let g = self.painter.into_inner();
        let (clip, desktop) = (self.clip, self.desktop);
        // (Its words are a picture: one running off the screen's edge
        // isn't text cut short, to be shown whole in a tip.)
        g.without_cut_notes(|g| {
            g.with_content_mask(Some(ContentMask { bounds: clip }), |g| {
                for run in cmds.chunk_by(|a, b| a.kind() == b.kind()) {
                    g.paint_layer(clip, |g| {
                        for cmd in run {
                            paint(cmd, g, desktop.as_ref());
                        }
                    });
                }
            });
        });
    }
}

fn paint(cmd: &Cmd, g: &mut Painter, desktop: Option<&Arc<RenderImage>>) {
    match cmd {
        Cmd::Quad(r, radius, color) => g.paint_quad(kit::fill(*r, *color).corner_radii(*radius)),
        Cmd::Segment(a, b, width, round, color) => g.paint_segment(*a, *b, *width, *round, *color),
        Cmd::Polyline(pieces, width, color) => g.paint_segments(pieces, *width, true, *color),
        Cmd::Sector(center, inner, outer, color) => g.paint_sector(*center, *inner, *outer, 0.0, std::f32::consts::TAU, *color),
        Cmd::Triangles(triangles, color) => {
            let mut path = PathBuilder::fill();
            for t in triangles {
                path.move_to(t[0]);
                path.line_to(t[1]);
                path.line_to(t[2]);
                path.close();
            }
            if let Ok(path) = path.build() {
                g.paint_path(path, *color);
            }
        }
        Cmd::Word(word, font_size, at, anchor, color) => {
            let align = match anchor {
                Anchor::Center => style::Align::Center,
                Anchor::LeftCenter => style::Align::Left,
            };
            style::label(g, px(at.x), px(at.y), align, word.clone(), *font_size, style::ui_font(FontWeight::BOLD), *color);
        }
        Cmd::Text(at, lines, lh) => {
            let mut y = at.y;
            for line in lines {
                let _ = line.paint(point(at.x, y), *lh, kit::TextAlign::Left, None, g);
                y += line.size(*lh).height;
            }
        }
        Cmd::Icon(r, name, alpha) => {
            icons::paint_faded(g, *r, name, *alpha);
        }
        Cmd::Desktop(r) => {
            if let Some(image) = desktop {
                let _ = g.paint_image(*r, Default::default(), image.clone(), 0, false);
            }
        }
        Cmd::DesktopTriangles(triangles, tint) => {
            if let Some(image) = desktop {
                let _ = g.paint_image_triangles(image.clone(), triangles, *tint);
            }
        }
    }
}

/// A line's height for text `font_size` high.
fn line_height(font_size: f32) -> Pixels {
    px((font_size * style::LINE_HEIGHT).round())
}

impl Canvas for KitCanvas<'_, '_> {
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
        self.push(Cmd::DesktopTriangles(triangles, kit::Rgba { r: light, g: light, b: light, a: 1.0 }.into()));
    }

    fn pixel(&self) -> f32 {
        1.0 / self.painter.borrow().scale_factor()
    }
}

/// The desktop picture (straight RGBA, `width` × `height`) as the
/// Interface Kit draws pictures.
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
        assert_eq!(super::line_height(24.0), olo_kit::px((24.0 * olo_kit::style::LINE_HEIGHT).round()));
    }
}
