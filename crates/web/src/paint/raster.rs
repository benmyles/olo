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
    /// The whole scene over `region` (document device pixels at the
    /// initial scroll): every stratum in order, as the page looks unscrolled.
    pub fn raster(&mut self, scene: &Scene, region: DeviceRect, cancel: Option<&AtomicBool>) -> Result<Tile> {
        let mut tile = self.raster_stratum(scene, 0, region, true, cancel)?;
        for i in 1..scene.strata().len() {
            let over = self.raster_stratum(scene, i, region, false, cancel)?;
            tile.pixmap.draw_pixmap(0, 0, over.pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
        }
        Ok(tile)
    }
    /// Stratum `i` of the scene over `region` (device pixels in the
    /// stratum's own space): on the page's background when `opaque` (the
    /// base), else on nothing, to composite over what is below.
    pub fn raster_stratum(&mut self, scene: &Scene, stratum: usize, region: DeviceRect, opaque: bool, cancel: Option<&AtomicBool>) -> Result<Tile> {
        let fixed = !opaque;
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
        let mut base = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
        if !fixed {
            base.fill(tiny_skia::Color::from_rgba8(scene.base[0], scene.base[1], scene.base[2], 255));
        }
        if !fixed && scene.canvas[3] != 0 {
            base.fill_rect(tiny_skia::Rect::from_xywh(0., 0., region.width as f32, region.height as f32).ok_or(Error::Invalid)?, &paint(scene.canvas), Transform::identity(), None);
        }
        // Subtract the tile origin in f64 before converting to raster coordinates.
        // Global f32 coordinates discard subpixels in long documents.
        let transform = Transform::identity();
        // A CSS transform in device space: document pixels through the box's
        // matrix, scaled, less the tile origin. Built in f64 so far-down
        // tiles keep their subpixels.
        let device = |t: u32| -> Transform {
            if t == u32::MAX {
                return Transform::identity();
            }
            let m = scene.transforms[t as usize];
            let (rx, ry) = (f64::from(region.x), f64::from(region.y));
            Transform::from_row(m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, (m[0] * rx + m[2] * ry + scale * m[4] - rx) as f32, (m[1] * rx + m[3] * ry + scale * m[5] - ry) as f32)
        };
        // A rect in untransformed device space, unclamped: the transform
        // decides where it lands, and the tile clips.
        let raw = |r: Rect| tiny_skia::Rect::from_xywh((r.x * scale - f64::from(region.x)) as f32, (r.y * scale - f64::from(region.y)) as f32, (r.width * scale) as f32, (r.height * scale) as f32);
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
        scene.query_stratum(stratum, rect, &mut hits, cancel)?;
        // OLO_TILE_PROFILE: time per primitive kind, per tile.
        let profile = std::env::var_os("OLO_TILE_PROFILE").map(|_| std::time::Instant::now());
        let mut counts = [0usize; 6];
        let mut spent = [0f64; 6];
        let mut last_clip = u32::MAX;
        let mut mask: Option<Mask> = None;
        let mut layer_work = 0usize;
        // Opacity groups composite through layers: one tile-sized surface per
        // open group, entered and left as the commands' groups change.
        let mut layers: Vec<(u32, Pixmap)> = Vec::new();
        let chain = |group: u32| -> Vec<u32> {
            let mut path = Vec::new();
            let mut at = group;
            while at != u32::MAX && path.len() < 64 {
                path.push(at);
                at = scene.groups[at as usize].parent;
            }
            path.reverse();
            path
        };
        let layer_bytes = (region.width as usize) * (region.height as usize) * 4;
        for id in hits {
            check(cancel)?;
            let c = &scene.commands[id as usize];
            if layers.last().map_or(u32::MAX, |l| l.0) != c.group {
                let wanted = chain(c.group);
                // Leave groups no longer open, compositing each onto its parent.
                while let Some(&(open, _)) = layers.last() {
                    if wanted.contains(&open) {
                        break;
                    }
                    let (group, layer) = layers.pop().unwrap();
                    let opacity = scene.groups[group as usize].opacity;
                    let group_mask = self.group_mask(scene, group, scale, region, cancel)?;
                    let target: &mut Pixmap = match layers.last_mut() {
                        Some(l) => &mut l.1,
                        None => &mut base,
                    };
                    target.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint { opacity, ..Default::default() }, Transform::identity(), group_mask.as_ref());
                }
                for &group in &wanted {
                    if !layers.iter().any(|l| l.0 == group) {
                        if (layers.len() + 1) * layer_bytes > self.limits().layer_bytes {
                            return Err(Error::Limit);
                        }
                        layers.push((group, Pixmap::new(region.width, region.height).ok_or(Error::Limit)?));
                    }
                }
            }
            let out: &mut Pixmap = match layers.last_mut() {
                Some(l) => &mut l.1,
                None => &mut base,
            };
            if c.clip != last_clip {
                mask = if c.clip == u32::MAX {
                    None
                } else {
                    let clip = &scene.clips[c.clip as usize];
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    let ct = device(clip.transform);
                    if let Some(radii) = &clip.radii {
                        if let Some(path) = super::decoration::path(clip.rect, radii, scale, region) {
                            m.fill_path(&path, FillRule::Winding, true, ct);
                        }
                    } else if clip.transform != u32::MAX {
                        if let Some(r) = raw(clip.rect) {
                            m.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, true, ct);
                        }
                    } else if let Some(r) = local(clip.rect) {
                        let path = PathBuilder::from_rect(r);
                        m.fill_path(&path, FillRule::Winding, false, transform);
                    }
                    Some(m)
                };
                last_clip = c.clip;
            }
            let ct = device(c.transform);
            let kind = match &c.primitive {
                Primitive::Image { .. } => 0,
                Primitive::Vector { .. } => 1,
                Primitive::Fill { .. } => 2,
                Primitive::Border { .. } => 3,
                Primitive::Decoration { .. } => 4,
                Primitive::Text { .. } | Primitive::Glyph { .. } => 5,
            };
            let started = profile.map(|_| std::time::Instant::now());
            counts[kind] += 1;
            match &c.primitive {
                Primitive::Image { owner, image } => {
                    let image = scene.layout.cache().tree().image(*image).ok_or(Error::Invalid)?;
                    // Its pixels are in the window's renderer: shown as a surface.
                    if image.shared_texture().is_some() {
                        continue;
                    }
                    let geometry = scene.layout.geometry(*owner).ok_or(Error::Invalid)?;
                    let mut bounds = c.bounds;
                    bounds.x += geometry.border_width.left + geometry.padding.left;
                    bounds.y += geometry.border_width.top + geometry.padding.top;
                    bounds.width = (bounds.width - geometry.border_width.left - geometry.border_width.right - geometry.padding.left - geometry.padding.right).max(0.);
                    bounds.height = (bounds.height - geometry.border_width.top - geometry.border_width.bottom - geometry.padding.top - geometry.padding.bottom).max(0.);
                    if bounds.width == 0. || bounds.height == 0. {
                        continue;
                    }
                    if c.transform == u32::MAX {
                        super::image::raster(self, out, image, bounds, scale, region, mask.as_ref(), cancel)?;
                    } else {
                        super::image::transformed(self, out, image, bounds, scale, region, ct, mask.as_ref(), cancel)?;
                    }
                }
                Primitive::Vector { owner, vector, origin } => {
                    let tree = scene.layout.cache().tree();
                    let resolved_clips = scene.vector_clips.get(vector);
                    let used_geometry = scene.vector_geometry.get(vector);
                    let vector = tree.vector(*vector).ok_or(Error::Invalid)?;
                    let geometry = scene.layout.geometry(*owner).ok_or(Error::Invalid)?;
                    let bounds = Rect { x: origin.x, y: origin.y, ..geometry.content_box() };
                    let Some([sx, sy, tx, ty]) = vector.transform(bounds.width, bounds.height) else {
                        continue;
                    };
                    let vector_mask = if vector.clip.iter().any(|c| *c) {
                        // The viewport clip, in CSS pixels, follows the
                        // command's transform (an ancestor's translate or
                        // scale moves the clip with the content).
                        let clip = Rect {
                            x: if vector.clip[0] { bounds.x } else { c.bounds.x },
                            y: if vector.clip[1] { bounds.y } else { c.bounds.y },
                            width: if vector.clip[0] { bounds.width } else { c.bounds.width },
                            height: if vector.clip[1] { bounds.height } else { c.bounds.height },
                        };
                        let (clip, clip_transform) = if c.transform == u32::MAX {
                            let Some(clip) = local(clip) else {
                                continue;
                            };
                            (clip, Transform::identity())
                        } else {
                            let Some(clip) = raw(clip) else {
                                continue;
                            };
                            (clip, ct)
                        };
                        let mut m = if let Some(mask) = &mask {
                            mask.clone()
                        } else {
                            let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                            m.data_mut().fill(255);
                            m
                        };
                        m.intersect_path(&PathBuilder::from_rect(clip), FillRule::Winding, false, clip_transform);
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
                    let transform = transform.post_concat(ct);
                    // A degenerate transform (a scale of zero, as an
                    // animation passes through) draws nothing.
                    if transform.is_valid() {
                        super::vector::raster(out, vector, used_geometry, resolved_clips.map(Vec::as_slice).unwrap_or(&[]), transform, vector_mask, self.limits(), &mut layer_work, cancel)?;
                    }
                }
                // Rounded (an inline box's background), antialiased.
                Primitive::Fill { color, radii, .. } if radii.iter().any(|r| *r > 0.) => {
                    let r = radii.map(|v| [f64::from(v); 2]);
                    if let Some(p) = super::decoration::path(c.bounds, &r, scale, region) {
                        out.fill_path(&p, &paint(*color), tiny_skia::FillRule::Winding, ct, mask.as_ref());
                    }
                }
                Primitive::Fill { color, .. } => {
                    if c.transform != u32::MAX {
                        if let Some(r) = raw(c.bounds) {
                            out.fill_rect(r, &paint(*color), ct, mask.as_ref());
                        }
                    } else if let Some(r) = local(c.bounds) {
                        out.fill_rect(r, &paint(*color), transform, mask.as_ref());
                    }
                }
                Primitive::Decoration { owner, outer } => {
                    let tree = scene.layout.cache().tree();
                    let s = tree.style(*owner).ok_or(Error::Invalid)?;
                    let g = scene.layout.geometry(*owner).ok_or(Error::Invalid)?;
                    super::decoration::raster(out, s, g, *outer, scale, region, mask.as_ref(), ct, self, tree, cancel)?;
                }
                Primitive::Border { owner, colors } => {
                    let b = scene.layout.geometry(*owner).ok_or(Error::Invalid)?.border_width;
                    let o = c.bounds;
                    let inner = Rect { x: o.x + b.left, y: o.y + b.top, width: (o.width - b.left - b.right).max(0.), height: (o.height - b.top - b.bottom).max(0.) };
                    super::border::raster(out, o, inner, *colors, scale, region, mask.as_ref(), ct, cancel)?;
                }
                Primitive::Glyph { owner, fragment, glyph, origin, color } => {
                    let text = scene.layout.text_layout(*owner).ok_or(Error::Invalid)?;
                    let f = text.fragments().get(*fragment as usize).ok_or(Error::Invalid)?;
                    let run = text.run(f).ok_or(Error::Invalid)?;
                    let unit = run.size / f64::from(run.font.units_per_em());
                    if let Some(path) = self.ink(&run.font, u32::from(*glyph), cancel)?.path {
                        let t = Transform::from_row(
                            (unit * scale) as f32,
                            0.,
                            0.,
                            (-unit * scale) as f32,
                            (origin.x * scale - f64::from(region.x)) as f32,
                            (origin.y * scale - f64::from(region.y)) as f32,
                        );
                        if t.is_finite() && t.sx != 0. && t.sy != 0. {
                            out.fill_path(&path, &paint(*color), FillRule::Winding, t.post_concat(ct), mask.as_ref());
                        }
                    }
                }
                Primitive::Text { owner, fragment, glyphs, origin, color, fill } => {
                    let text = scene.layout.text_layout(*owner).ok_or(Error::Invalid)?;
                    let f = text.fragments().get(*fragment as usize).ok_or(Error::Invalid)?;
                    let run = text.run(f).ok_or(Error::Invalid)?;
                    let unit = run.size / f64::from(run.font.units_per_em());
                    // `background-clip: text`: the glyphs' coverage, through
                    // which the box's backgrounds paint under their colour.
                    let mut coverage = match scene.text_fills.get(*fill as usize) {
                        Some(_) => {
                            if (region.width as usize).saturating_mul(region.height as usize) > self.limits().layer_bytes {
                                return Err(Error::Limit);
                            }
                            Some(Mask::new(region.width, region.height).ok_or(Error::Limit)?)
                        }
                        None => None,
                    };
                    let p = paint(*color);
                    let passes: &[bool] = if coverage.is_some() { &[true, false] } else { &[false] };
                    for &pass in passes {
                        if !pass && color[3] == 0 {
                            break;
                        }
                        let mut pen = 0i64;
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
                                let t = t.post_concat(ct);
                                match (pass, coverage.as_mut()) {
                                    (true, Some(m)) => m.fill_path(&path, FillRule::Winding, true, t),
                                    _ => out.fill_path(&path, &p, FillRule::Winding, t, mask.as_ref()),
                                }
                            }
                            pen += i64::from(g.x_advance);
                        }
                        if pass && let Some(m) = coverage.take() {
                            let m = match mask.as_ref() {
                                Some(clip) => {
                                    let mut m = m;
                                    for (a, b) in m.data_mut().iter_mut().zip(clip.data()) {
                                        *a = ((u16::from(*a) * u16::from(*b) + 127) / 255) as u8;
                                    }
                                    m
                                }
                                None => m,
                            };
                            let &(box_, outer) = &scene.text_fills[*fill as usize];
                            let tree = scene.layout.cache().tree();
                            let s = tree.style(box_).ok_or(Error::Invalid)?;
                            let g = scene.layout.geometry(box_).ok_or(Error::Invalid)?;
                            super::decoration::text_backgrounds(out, s, g, outer, scale, region, &m, ct, self, tree, cancel)?;
                        }
                    }
                }
            }
            if let Some(t) = started {
                spent[kind] += t.elapsed().as_secs_f64() * 1000.;
            }
        }
        if let Some(t) = profile {
            let names = ["image", "vector", "fill", "border", "decoration", "text"];
            let parts: Vec<String> = (0..6).filter(|&k| counts[k] > 0).map(|k| format!("{} {}×{:.2}", names[k], counts[k], spent[k])).collect();
            let d = super::decoration::PROFILE.with(|p| p.replace([0.; 8]));
            let i = super::image::PROFILE.with(|p| p.replace([(0, 0.); 5]));
            let image_paths: Vec<String> =
                ["copied", "blended", "masked", "sampled", "transformed"].iter().zip(i).filter(|(_, (n, _))| *n > 0).map(|(name, (n, ms))| format!("{name} {n}×{ms:.2}")).collect();
            eprintln!(
                "  tile {},{}: {:.1} ms: {} (images: {}; decoration: shadows {:.2} [inside {:.2}, cached ×{:.0}, fallback ×{:.0}], colour {:.2}, images {:.2}, gradients {:.2}, borders {:.2})",
                region.x,
                region.y,
                t.elapsed().as_secs_f64() * 1000.,
                parts.join(", "),
                image_paths.join(", "),
                d[0],
                d[5],
                d[6] * 1000.,
                d[7] * 1000.,
                d[1],
                d[2],
                d[3],
                d[4]
            );
        }
        while let Some((group, layer)) = layers.pop() {
            let opacity = scene.groups[group as usize].opacity;
            let group_mask = self.group_mask(scene, group, scale, region, cancel)?;
            let target: &mut Pixmap = match layers.last_mut() {
                Some(l) => &mut l.1,
                None => &mut base,
            };
            target.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint { opacity, ..Default::default() }, Transform::identity(), group_mask.as_ref());
        }
        Ok(Tile { region, pixmap: base })
    }
    /// A group's mask over this tile: its owner's mask images drawn on a
    /// transparent surface, taken as alpha.
    fn group_mask(&mut self, scene: &Scene, group: u32, scale: f64, region: DeviceRect, cancel: Option<&AtomicBool>) -> Result<Option<Mask>> {
        let Some(m) = scene.groups[group as usize].mask else { return Ok(None) };
        let tree = scene.layout.cache().tree();
        let s = tree.style(m.owner).ok_or(Error::Invalid)?;
        let mut surface = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
        let transform = if m.transform == u32::MAX {
            Transform::identity()
        } else {
            let a = scene.transforms[m.transform as usize];
            let (rx, ry) = (f64::from(region.x), f64::from(region.y));
            Transform::from_row(a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32, (a[0] * rx + a[2] * ry + scale * a[4] - rx) as f32, (a[1] * rx + a[3] * ry + scale * a[5] - ry) as f32)
        };
        super::decoration::mask_layers(&mut surface, s, m.outer, scale, region, transform, self, tree, cancel)?;
        Ok(Some(Mask::from_pixmap(surface.as_ref(), tiny_skia::MaskType::Alpha)))
    }
}
