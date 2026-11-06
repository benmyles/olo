use super::*;
use crate::layout::{Point, Rect};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, Transform};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
pub struct Tile {
    pub region: DeviceRect,
    pixmap: Pixmap,
}
impl Tile {
    pub fn rgba(&self) -> &[u8] {
        self.pixmap.data()
    }
    pub fn into_rgba(self) -> Vec<u8> {
        self.pixmap.take()
    }
    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        self.pixmap.save_png(path).map_err(|_| Error::Invalid)
    }
}
fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p
}
impl Engine {
    pub fn raster(&mut self, scene: &Scene, region: DeviceRect, cancel: Option<&AtomicBool>) -> Result<Tile> {
        check(cancel)?;
        let pixels = (region.width as usize).checked_mul(region.height as usize).ok_or(Error::Limit)?;
        if pixels > self.limits().tile_pixels {
            return Err(Error::Limit);
        }
        if pixels == 0 {
            return Err(Error::Invalid);
        }
        let scale = scene.device_scale();
        let rect = Rect { x: f64::from(region.x) / scale, y: f64::from(region.y) / scale, width: f64::from(region.width) / scale, height: f64::from(region.height) / scale };
        if !valid(rect) {
            return Err(Error::Invalid);
        }
        let mut out = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
        out.fill(tiny_skia::Color::from_rgba8(scene.base[0], scene.base[1], scene.base[2], 255));
        if scene.canvas[3] != 0 {
            out.fill_rect(tiny_skia::Rect::from_xywh(0., 0., region.width as f32, region.height as f32).ok_or(Error::Invalid)?, &paint(scene.canvas), Transform::identity(), None);
        }
        // Subtract the tile origin in f64 before converting to raster coordinates.
        // Global f32 coordinates discard subpixels in long documents.
        let transform = Transform::identity();
        let local = |r: Rect| {
            let x = r.x.max(rect.x);
            let y = r.y.max(rect.y);
            let right = (r.x + r.width).min(rect.x + rect.width);
            let bottom = (r.y + r.height).min(rect.y + rect.height);
            tiny_skia::Rect::from_ltrb(
                (x * scale - f64::from(region.x)) as f32,
                (y * scale - f64::from(region.y)) as f32,
                (right * scale - f64::from(region.x)) as f32,
                (bottom * scale - f64::from(region.y)) as f32,
            )
        };
        let local_point = |p: Point| -> Result<(f32, f32)> {
            let x = (p.x * scale - f64::from(region.x)) as f32;
            let y = (p.y * scale - f64::from(region.y)) as f32;
            if !x.is_finite() || !y.is_finite() {
                return Err(Error::Invalid);
            }
            Ok((x, y))
        };
        let mut hits = Vec::new();
        scene.query(rect, &mut hits, cancel)?;
        let mut last_clip = u32::MAX;
        let mut mask: Option<Mask> = None;
        for id in hits {
            check(cancel)?;
            let c = &scene.commands[id as usize];
            if c.clip != last_clip {
                mask = if c.clip == u32::MAX {
                    None
                } else {
                    let r = scene.clips[c.clip as usize];
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    if let Some(r) = local(r) {
                        let path = PathBuilder::from_rect(r);
                        m.fill_path(&path, FillRule::Winding, true, transform);
                    }
                    Some(m)
                };
                last_clip = c.clip;
            }
            match &c.primitive {
                Primitive::Fill { color } => {
                    if let Some(r) = local(c.bounds) {
                        out.fill_rect(r, &paint(*color), transform, mask.as_ref());
                    }
                }
                Primitive::Border { owner, colors } => {
                    let b = scene.layout.geometry(*owner).ok_or(Error::Invalid)?.border_width;
                    let o = c.bounds;
                    let inner = Rect { x: o.x + b.left, y: o.y + b.top, width: (o.width - b.left - b.right).max(0.), height: (o.height - b.top - b.bottom).max(0.) };
                    if colors.iter().all(|c| *c == colors[0]) {
                        let mut path = PathBuilder::new();
                        if let Some(r) = local(o) {
                            path.push_rect(r)
                        }
                        if let Some(r) = local(inner) {
                            path.push_rect(r)
                        }
                        if let Some(path) = path.finish() {
                            out.fill_path(&path, &paint(colors[0]), FillRule::EvenOdd, transform, mask.as_ref());
                        }
                    } else {
                        let outer = [Point { x: o.x, y: o.y }, Point { x: o.x + o.width, y: o.y }, Point { x: o.x + o.width, y: o.y + o.height }, Point { x: o.x, y: o.y + o.height }];
                        let inn = [
                            Point { x: inner.x, y: inner.y },
                            Point { x: inner.x + inner.width, y: inner.y },
                            Point { x: inner.x + inner.width, y: inner.y + inner.height },
                            Point { x: inner.x, y: inner.y + inner.height },
                        ];
                        for i in 0..4 {
                            if colors[i][3] == 0 {
                                continue;
                            }
                            let j = (i + 1) % 4;
                            let mut path = PathBuilder::new();
                            let (x, y) = local_point(outer[i])?;
                            path.move_to(x, y);
                            for p in [outer[j], inn[j], inn[i]] {
                                let (x, y) = local_point(p)?;
                                path.line_to(x, y)
                            }
                            path.close();
                            if let Some(path) = path.finish() {
                                let mut p = paint(colors[i]);
                                p.anti_alias = false;
                                out.fill_path(&path, &p, FillRule::Winding, transform, mask.as_ref());
                            }
                        }
                    }
                }
                Primitive::Text { owner, fragment, glyphs, origin, color } => {
                    let text = scene.layout.text_layout(*owner).ok_or(Error::Invalid)?;
                    let f = text.fragments().get(*fragment as usize).ok_or(Error::Invalid)?;
                    let run = text.run(f).ok_or(Error::Invalid)?;
                    let unit = text.text().paragraph().font_size / f64::from(run.font.units_per_em());
                    let mut pen = 0i64;
                    let p = paint(*color);
                    for g in &run.glyphs[glyphs.start as usize..glyphs.end as usize] {
                        check(cancel)?;
                        let ink = self.ink(&run.font, g.id, cancel)?;
                        if let Some(path) = ink.path {
                            let x = origin.x + (pen + i64::from(g.x_offset)) as f64 * unit;
                            let y = origin.y - f64::from(g.y_offset) * unit;
                            let t = Transform::from_row((unit * scale) as f32, 0., 0., (-unit * scale) as f32, (x * scale - f64::from(region.x)) as f32, (y * scale - f64::from(region.y)) as f32);
                            if !t.is_finite() || t.sx == 0. || t.sy == 0. {
                                return Err(Error::Invalid);
                            }
                            out.fill_path(&path, &p, FillRule::Winding, t, mask.as_ref());
                        }
                        pen += i64::from(g.x_advance);
                    }
                }
            }
        }
        Ok(Tile { region, pixmap: out })
    }
}
