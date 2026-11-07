use super::*;
use crate::layout::Rect;
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
        if let Some(cache) = self.image_mips.as_mut() {
            let tree = scene.layout.cache().tree();
            cache.prepare(tree.get(tree.root()).ok_or(Error::Invalid)?.node.document());
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
            let x = (r.x * scale).round().max(f64::from(region.x));
            let y = (r.y * scale).round().max(f64::from(region.y));
            let right = ((r.x + r.width) * scale).round().min(f64::from(region.x) + f64::from(region.width));
            let bottom = ((r.y + r.height) * scale).round().min(f64::from(region.y) + f64::from(region.height));
            if right <= x || bottom <= y {
                return None;
            }
            tiny_skia::Rect::from_ltrb((x - f64::from(region.x)) as f32, (y - f64::from(region.y)) as f32, (right - f64::from(region.x)) as f32, (bottom - f64::from(region.y)) as f32)
        };
        let mut hits = Vec::new();
        scene.query(rect, &mut hits, cancel)?;
        let mut last_clip = u32::MAX;
        let mut mask: Option<Mask> = None;
        let mut clip_operations = 0usize;
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
                        m.fill_path(&path, FillRule::Winding, false, transform);
                    }
                    Some(m)
                };
                last_clip = c.clip;
            }
            match &c.primitive {
                Primitive::Image { owner, image } => {
                    let image = scene.layout.cache().tree().image(*image).ok_or(Error::Invalid)?;
                    let geometry = scene.layout.geometry(*owner).ok_or(Error::Invalid)?;
                    let mut bounds = c.bounds;
                    bounds.x += geometry.border_width.left + geometry.padding.left;
                    bounds.y += geometry.border_width.top + geometry.padding.top;
                    bounds.width = (bounds.width - geometry.border_width.left - geometry.border_width.right - geometry.padding.left - geometry.padding.right).max(0.);
                    bounds.height = (bounds.height - geometry.border_width.top - geometry.border_width.bottom - geometry.padding.top - geometry.padding.bottom).max(0.);
                    if bounds.width == 0. || bounds.height == 0. {
                        continue;
                    }
                    super::image::raster(self, &mut out, image, bounds, scale, region, mask.as_ref(), cancel)?;
                }
                Primitive::Vector { owner, vector, origin } => {
                    let tree = scene.layout.cache().tree();
                    let resolved_clips = scene.vector_clips.get(vector);
                    let vector = tree.vector(*vector).ok_or(Error::Invalid)?;
                    let geometry = scene.layout.geometry(*owner).ok_or(Error::Invalid)?;
                    let bounds = Rect { x: origin.x, y: origin.y, ..geometry.content_box() };
                    let Some([sx, sy, tx, ty]) = vector.transform(bounds.width, bounds.height) else {
                        continue;
                    };
                    let vector_mask = if vector.clip.iter().any(|c| *c) {
                        let clip = Rect {
                            x: if vector.clip[0] { bounds.x } else { rect.x },
                            y: if vector.clip[1] { bounds.y } else { rect.y },
                            width: if vector.clip[0] { bounds.width } else { rect.width },
                            height: if vector.clip[1] { bounds.height } else { rect.height },
                        };
                        let Some(clip) = local(clip) else {
                            continue;
                        };
                        let mut m = if let Some(mask) = &mask {
                            mask.clone()
                        } else {
                            let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                            m.data_mut().fill(255);
                            m
                        };
                        m.intersect_path(&PathBuilder::from_rect(clip), FillRule::Winding, false, Transform::identity());
                        Some(m)
                    } else {
                        None
                    };
                    let vector_mask = vector_mask.as_ref().or(mask.as_ref());
                    let transform = Transform::from_row(
                        (sx * scale) as f32,
                        0.,
                        0.,
                        (sy * scale) as f32,
                        ((bounds.x + tx) * scale - f64::from(region.x)) as f32,
                        ((bounds.y + ty) * scale - f64::from(region.y)) as f32,
                    );
                    if !transform.is_valid() {
                        return Err(Error::Unsupported("SVG viewport transform precision"));
                    }
                    let mut last = crate::svg::clip::NO_CLIP;
                    let mut clipped: Option<Mask> = None;
                    for shape in &vector.shapes {
                        check(cancel)?;
                        if shape.clip != last {
                            clipped = if shape.clip == crate::svg::clip::NO_CLIP {
                                None
                            } else {
                                let definitions = resolved_clips.ok_or(Error::Invalid)?;
                                let mut mask = if let Some(mask) = vector_mask {
                                    mask.clone()
                                } else {
                                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                                    m.data_mut().fill(255);
                                    m
                                };
                                let mut at = shape.clip;
                                while at != crate::svg::clip::NO_CLIP {
                                    check(cancel)?;
                                    clip_operations = clip_operations.checked_add(1).ok_or(Error::Limit)?;
                                    if clip_operations > self.limits().path_segments {
                                        return Err(Error::Limit);
                                    }
                                    let clip = definitions.get(at as usize).ok_or(Error::Invalid)?;
                                    if let Some(path) = clip.path.as_ref() {
                                        let path = path.clone().transform(transform).ok_or(Error::Unsupported("SVG clip transform precision"))?;
                                        let b = path.bounds();
                                        if [b.left(), b.top(), b.right(), b.bottom()].iter().any(|v| v.abs() > f32::MAX * 0.25) {
                                            return Err(Error::Unsupported("SVG clip raster precision"));
                                        }
                                        mask.intersect_path(&path, FillRule::Winding, true, Transform::identity());
                                    } else {
                                        mask.data_mut().fill(0);
                                        break;
                                    }
                                    if clip.parent != crate::svg::clip::NO_CLIP && clip.parent >= at {
                                        return Err(Error::Invalid);
                                    }
                                    at = clip.parent;
                                }
                                Some(mask)
                            };
                            last = shape.clip;
                        }
                        if let Some(path) = shape.path.geometry() {
                            out.fill_path(path, &paint(shape.color), if shape.evenodd { FillRule::EvenOdd } else { FillRule::Winding }, transform, clipped.as_ref().or(vector_mask));
                        }
                    }
                }
                Primitive::Fill { color } => {
                    if let Some(r) = local(c.bounds) {
                        out.fill_rect(r, &paint(*color), transform, mask.as_ref());
                    }
                }
                Primitive::Border { owner, colors } => {
                    let b = scene.layout.geometry(*owner).ok_or(Error::Invalid)?.border_width;
                    let o = c.bounds;
                    let inner = Rect { x: o.x + b.left, y: o.y + b.top, width: (o.width - b.left - b.right).max(0.), height: (o.height - b.top - b.bottom).max(0.) };
                    super::border::raster(&mut out, o, inner, *colors, scale, region, mask.as_ref(), cancel)?;
                }
                Primitive::Text { owner, fragment, glyphs, origin, color, .. } => {
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
