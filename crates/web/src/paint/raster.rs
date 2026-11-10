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
        self.raster_layer(scene, stratum, region, opaque, None, scene.device_scale(), cancel)
    }
    /// `raster_stratum` at `scale` (device pixels per CSS px), all of it or
    /// `only` what one group paints (its commands and its groups', on
    /// nothing): a blurred group's content, over more than the tile.
    #[allow(clippy::too_many_arguments)]
    fn raster_layer(&mut self, scene: &Scene, stratum: usize, region: DeviceRect, opaque: bool, only: Option<u32>, scale: f64, cancel: Option<&AtomicBool>) -> Result<Tile> {
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
        let unclipped = only.map_or(u32::MAX, |g| {
            let g = &scene.groups[g as usize];
            if g.blur > 0. || g.shadow.is_some() { g.clip } else { u32::MAX }
        });
        let mut hits = Vec::new();
        scene.query_stratum(stratum, rect, &mut hits, cancel)?;
        if let Some(only) = only {
            hits.retain(|&id| {
                let mut at = scene.commands[id as usize].group;
                while at != u32::MAX {
                    if at == only {
                        return true;
                    }
                    at = scene.groups[at as usize].parent;
                }
                false
            });
        }
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
            while at != u32::MAX && Some(at) != only && path.len() < 64 {
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
                    let layer = self.filtered(scene, stratum, group, layer, region, scale, cancel)?;
                    let opacity = scene.groups[group as usize].opacity;
                    let blend_mode = blend_mode(scene.groups[group as usize].blend);
                    let group_mask = self.composite_mask(scene, group, scale, region, cancel)?;
                    let target: &mut Pixmap = match layers.last_mut() {
                        Some(l) => &mut l.1,
                        None => &mut base,
                    };
                    composite(target, &layer, opacity, blend_mode, group_mask.as_ref());
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
            // A filtered group's content drawn again for its filter: what
            // is under only the group's own clip, unclipped (`Group::clip`).
            let clip = if c.clip == unclipped { u32::MAX } else { c.clip };
            if clip != last_clip {
                mask = if clip == u32::MAX {
                    None
                } else {
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    clip_into(&mut m, scene, clip, scale, region, false);
                    Some(m)
                };
                last_clip = clip;
            }
            let ct = device(c.transform);
            let kind = match &c.primitive {
                Primitive::Image { .. } => 0,
                Primitive::Vector { .. } => 1,
                Primitive::Fill { .. } | Primitive::Wave { .. } => 2,
                Primitive::Border { .. } => 3,
                Primitive::Decoration { .. } | Primitive::Control { .. } => 4,
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
                Primitive::Control { look, .. } => super::control::raster(out, look, c.bounds, scale, region, mask.as_ref(), ct),
                // A wavy decoration: a sine stroked across its bounds.
                Primitive::Wave { color, thickness, wavelength, .. } => {
                    let b = c.bounds;
                    let (mid, amplitude) = (b.y + b.height / 2., b.height / 2. - f64::from(*thickness));
                    let mut path = tiny_skia::PathBuilder::new();
                    let steps = ((b.width / f64::from(*wavelength)) * 16.).ceil().clamp(2., 4096.) as usize;
                    for k in 0..=steps {
                        let x = b.x + b.width * k as f64 / steps as f64;
                        let y = mid - amplitude * ((x - b.x) / f64::from(*wavelength) * std::f64::consts::TAU).sin();
                        let (px, py) = ((x * scale - f64::from(region.x)) as f32, (y * scale - f64::from(region.y)) as f32);
                        if k == 0 { path.move_to(px, py) } else { path.line_to(px, py) }
                    }
                    if let Some(path) = path.finish() {
                        let mut paint = tiny_skia::Paint::default();
                        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
                        paint.anti_alias = true;
                        let stroke = tiny_skia::Stroke { width: (f64::from(*thickness) * scale) as f32, ..Default::default() };
                        out.stroke_path(&path, &paint, &stroke, ct, mask.as_ref());
                    }
                }
                Primitive::Border { owner, colors, styles } => {
                    let b = scene.layout.geometry(*owner).ok_or(Error::Invalid)?.border_width;
                    let o = c.bounds;
                    let inner = Rect { x: o.x + b.left, y: o.y + b.top, width: (o.width - b.left - b.right).max(0.), height: (o.height - b.top - b.bottom).max(0.) };
                    super::border::raster(out, o, inner, *colors, *styles, scale, region, mask.as_ref(), ct, cancel)?;
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
                            self.paint_glyph(out, &path, *color, t.post_concat(ct), mask.as_ref(), cancel)?;
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
                                    _ => self.paint_glyph(out, &path, *color, t, mask.as_ref(), cancel)?,
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
            let layer = self.filtered(scene, stratum, group, layer, region, scale, cancel)?;
            let opacity = scene.groups[group as usize].opacity;
            let blend_mode = blend_mode(scene.groups[group as usize].blend);
            let group_mask = self.composite_mask(scene, group, scale, region, cancel)?;
            let target: &mut Pixmap = match layers.last_mut() {
                Some(l) => &mut l.1,
                None => &mut base,
            };
            composite(target, &layer, opacity, blend_mode, group_mask.as_ref());
        }
        Ok(Tile { region, pixmap: base })
    }
    /// A group's layer over this tile through its `filter`: blurred (its
    /// content painted again over the tile and as far round as the blur
    /// reaches, at a lower resolution for a wide blur, blurred and cut to
    /// the tile), then its colour matrix.
    #[allow(clippy::too_many_arguments)]
    fn filtered(&mut self, scene: &Scene, stratum: usize, group: u32, mut layer: Pixmap, region: DeviceRect, scale: f64, cancel: Option<&AtomicBool>) -> Result<Pixmap> {
        let g = scene.groups[group as usize];
        if g.blur > 0. {
            let sigma = f64::from(g.blur) * scale;
            // A wide blur at a fraction of the resolution (one in `k`),
            // within the tile size limit: what it blurs away it never needed
            // (`blur_step`).
            let mut k = blur_step(sigma);
            loop {
                let margin = (3. * sigma / k).ceil();
                let w = (f64::from(region.width) / k).ceil() + 2. * margin;
                let h = (f64::from(region.height) / k).ceil() + 2. * margin;
                if w * h <= self.limits().tile_pixels as f64 || k >= 64. {
                    break;
                }
                k *= 2.;
            }
            let margin = (3. * sigma / k).ceil() as i32;
            let small = DeviceRect {
                x: (f64::from(region.x) / k).floor() as i32 - margin,
                y: (f64::from(region.y) / k).floor() as i32 - margin,
                width: (f64::from(region.width) / k).ceil() as u32 + 2 * margin as u32 + 1,
                height: (f64::from(region.height) / k).ceil() as u32 + 2 * margin as u32 + 1,
            };
            if let Ok(tile) = self.raster_layer(scene, stratum, small, false, Some(group), scale / k, cancel) {
                let mut content = tile.pixmap;
                blur(&mut content, sigma / k);
                let mut out = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
                // Back to the tile's pixels: the small region's origin at
                // its place, scaled by k (bilinear: it is all blurred).
                let ox = f64::from(small.x) * k - f64::from(region.x);
                let oy = f64::from(small.y) * k - f64::from(region.y);
                upscale(&content, k, ox, oy, &mut out);
                layer = out;
            }
        }
        if let Some(m) = &g.filter {
            filter_colors(&mut layer, m);
        }
        if let Some(d) = g.shadow {
            layer = self.drop_shadow(scene, stratum, group, layer, d, region, scale, cancel)?;
        }
        if let Some(m) = &g.after {
            filter_colors(&mut layer, m);
        }
        Ok(layer)
    }
    /// A `drop-shadow()` under a group's layer: the group's content painted
    /// again where the shadow comes from (the tile moved back by its offset,
    /// as far round as its blur reaches; at a fraction of the resolution for
    /// a wide blur), its alpha in the shadow's colour, blurred and moved.
    #[allow(clippy::too_many_arguments)]
    fn drop_shadow(&mut self, scene: &Scene, stratum: usize, group: u32, layer: Pixmap, d: super::scene::DropShadow, region: DeviceRect, scale: f64, cancel: Option<&AtomicBool>) -> Result<Pixmap> {
        if d.color[3] == 0 {
            return Ok(layer);
        }
        let sigma = f64::from(d.sigma) * scale;
        let (dx, dy) = (f64::from(d.dx) * scale, f64::from(d.dy) * scale);
        let mut k = blur_step(sigma);
        loop {
            let margin = (3. * sigma / k).ceil();
            let w = (f64::from(region.width) / k).ceil() + 2. * margin;
            let h = (f64::from(region.height) / k).ceil() + 2. * margin;
            if w * h <= self.limits().tile_pixels as f64 || k >= 64. {
                break;
            }
            k *= 2.;
        }
        let margin = (3. * sigma / k).ceil() as i32;
        // Where the shadow on this tile comes from, at 1/k.
        let small = DeviceRect {
            x: ((f64::from(region.x) - dx) / k).floor() as i32 - margin,
            y: ((f64::from(region.y) - dy) / k).floor() as i32 - margin,
            width: (f64::from(region.width) / k).ceil() as u32 + 2 * margin as u32 + 1,
            height: (f64::from(region.height) / k).ceil() as u32 + 2 * margin as u32 + 1,
        };
        let Ok(tile) = self.raster_layer(scene, stratum, small, false, Some(group), scale / k, cancel) else { return Ok(layer) };
        let mut shadow = tile.pixmap;
        // Its alpha in the colour (premultiplied).
        let c = d.color;
        for px in shadow.data_mut().chunks_exact_mut(4) {
            let a = u32::from(px[3]) * u32::from(c[3]) / 255;
            for i in 0..3 {
                px[i] = (u32::from(c[i]) * a / 255) as u8;
            }
            px[3] = a as u8;
        }
        blur(&mut shadow, sigma / k);
        let mut out = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
        let ox = f64::from(small.x) * k + dx - f64::from(region.x);
        let oy = f64::from(small.y) * k + dy - f64::from(region.y);
        upscale(&shadow, k, ox, oy, &mut out);
        out.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
        Ok(out)
    }
    /// What a group composites through: its mask, and, for a blurred or
    /// shadowed one (drawn unclipped for its filter), its clip.
    fn composite_mask(&mut self, scene: &Scene, group: u32, scale: f64, region: DeviceRect, cancel: Option<&AtomicBool>) -> Result<Option<Mask>> {
        let mut mask = self.group_mask(scene, group, scale, region, cancel)?;
        let g = &scene.groups[group as usize];
        if (g.blur > 0. || g.shadow.is_some()) && g.clip != u32::MAX {
            match mask.as_mut() {
                Some(m) => clip_into(m, scene, g.clip, scale, region, true),
                None => {
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    clip_into(&mut m, scene, g.clip, scale, region, false);
                    mask = Some(m);
                }
            }
        }
        Ok(mask)
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
/// A `filter`'s colour matrix (rows R, G, B, A over unpremultiplied R, G,
/// B, A, 1) applied to every pixel of a group's layer.
fn filter_colors(layer: &mut Pixmap, m: &[f32; 20]) {
    for px in layer.data_mut().chunks_exact_mut(4) {
        let a = f32::from(px[3]) / 255.;
        // A transparent pixel stays so unless the matrix gives alpha from
        // nothing (an alpha offset), which no colour function does.
        if a == 0. && m[19] == 0. {
            continue;
        }
        let c = if a > 0. { [f32::from(px[0]) / 255. / a, f32::from(px[1]) / 255. / a, f32::from(px[2]) / 255. / a, a] } else { [0., 0., 0., 0.] };
        let row = |r: usize| (m[r * 5] * c[0] + m[r * 5 + 1] * c[1] + m[r * 5 + 2] * c[2] + m[r * 5 + 3] * c[3] + m[r * 5 + 4]).clamp(0., 1.);
        let alpha = row(3);
        for i in 0..3 {
            px[i] = (row(i) * alpha * 255. + 0.5) as u8;
        }
        px[3] = (alpha * 255. + 0.5) as u8;
    }
}
/// A group's layer onto what is under it, the same size and place: through
/// its opacity and mask, source-over in integers (tiny-skia's pipeline
/// for a masked pixmap gathered every pixel as through a transform: a
/// tenth of a page of blurred, clipped pictures); other blend modes by
/// tiny-skia.
fn composite(target: &mut Pixmap, layer: &Pixmap, opacity: f32, blend_mode: tiny_skia::BlendMode, mask: Option<&Mask>) {
    if blend_mode != tiny_skia::BlendMode::SourceOver
        || target.width() != layer.width()
        || target.height() != layer.height()
        || mask.is_some_and(|m| m.width() != layer.width() || m.height() != layer.height())
    {
        target.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint { opacity, blend_mode, ..Default::default() }, Transform::identity(), mask);
        return;
    }
    let o = (opacity.clamp(0., 1.) * 255. + 0.5) as u32;
    if o == 0 {
        return;
    }
    let mask = mask.map(|m| m.data());
    for (i, (d, s)) in target.data_mut().chunks_exact_mut(4).zip(layer.data().chunks_exact(4)).enumerate() {
        if s[3] == 0 && s[0] == 0 && s[1] == 0 && s[2] == 0 {
            continue;
        }
        // Coverage out of 255 × 255: the opacity times the mask's.
        let k = o * mask.map_or(255, |m| u32::from(m[i]));
        if k == 0 {
            continue;
        }
        let mut px = [0u32; 4];
        for c in 0..4 {
            px[c] = if k == 65025 { u32::from(s[c]) } else { (u32::from(s[c]) * k + 32512) / 65025 };
        }
        if px[3] == 255 {
            d.copy_from_slice(&[px[0] as u8, px[1] as u8, px[2] as u8, 255]);
            continue;
        }
        let inverse = 255 - px[3];
        for c in 0..4 {
            d[c] = (px[c] + (u32::from(d[c]) * inverse + 127) / 255).min(255) as u8;
        }
    }
}
/// Clip `clip`'s shape over a tile at `region` (device pixels at `scale`)
/// drawn into `mask`, or (`intersect`) cutting what it holds.
fn clip_into(mask: &mut Mask, scene: &Scene, clip: u32, scale: f64, region: DeviceRect, intersect: bool) {
    let clip = &scene.clips[clip as usize];
    let (rx, ry) = (f64::from(region.x), f64::from(region.y));
    // Its transform in device space less the tile origin (as the commands').
    let ct = if clip.transform == u32::MAX {
        Transform::identity()
    } else {
        let m = scene.transforms[clip.transform as usize];
        Transform::from_row(m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, (m[0] * rx + m[2] * ry + scale * m[4] - rx) as f32, (m[1] * rx + m[3] * ry + scale * m[5] - ry) as f32)
    };
    let (path, anti_alias) = if let Some(radii) = &clip.radii {
        (super::decoration::path(clip.rect, radii, scale, region), true)
    } else if clip.transform != u32::MAX {
        let r = clip.rect;
        (tiny_skia::Rect::from_xywh((r.x * scale - rx) as f32, (r.y * scale - ry) as f32, (r.width * scale) as f32, (r.height * scale) as f32).map(PathBuilder::from_rect), true)
    } else {
        // Untransformed: on whole device pixels, within the tile.
        let r = clip.rect;
        let x = (r.x * scale).round().max(rx);
        let y = (r.y * scale).round().max(ry);
        let right = ((r.x + r.width) * scale).round().min(rx + f64::from(region.width));
        let bottom = ((r.y + r.height) * scale).round().min(ry + f64::from(region.height));
        let rect = if right <= x || bottom <= y { None } else { tiny_skia::Rect::from_ltrb((x - rx) as f32, (y - ry) as f32, (right - rx) as f32, (bottom - ry) as f32) };
        (Some(rect.map(PathBuilder::from_rect).unwrap_or_else(|| PathBuilder::from_rect(tiny_skia::Rect::from_xywh(-2., -2., 1., 1.).expect("a unit rect")))), false)
    };
    match (path, intersect) {
        (Some(path), false) => mask.fill_path(&path, FillRule::Winding, anti_alias, ct),
        (Some(path), true) => mask.intersect_path(&path, FillRule::Winding, anti_alias, ct),
        // Nothing of it: nothing shows through.
        (None, _) => mask.clear(),
    }
}
/// How coarse a blur of `sigma` device pixels is drawn: one pixel in `k`
/// each way, `k` a whole number so its weights repeat. Not coarser than a
/// twelfth of sigma: the content is drawn at that resolution before it is
/// blurred, and gradients there are sampled at points, so a sixth moved a
/// striped backdrop's mean colour by 7 in 255.
fn blur_step(sigma: f64) -> f64 {
    (sigma / 12.).max(1.).floor()
}
/// A Gaussian blur of standard deviation `sigma` (pixels) over a
/// premultiplied layer, as three box blurs each way (Filter Effects 1
/// §13.1's approximation): linear in the pixels whatever the radius.
fn blur(layer: &mut Pixmap, sigma: f64) {
    let d = (sigma * 3. * (2. * std::f64::consts::PI).sqrt() / 4. + 0.5).floor() as usize;
    if d < 2 {
        return;
    }
    // Each pass as (pixels before, pixels after) the one it writes.
    let passes: [(usize, usize); 3] = if d % 2 == 1 { [(d / 2, d / 2); 3] } else { [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)] };
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    let data = layer.data_mut();
    // Both ways as passes down columns, a row at a time, every column's
    // window summed side by side (walking one column, or one row's pixels,
    // at a time was a cache line or a bounds check per pixel): across
    // through the layer turned on its side, then down it.
    let mut a = vec![0u8; data.len()];
    let mut b = vec![0u8; data.len()];
    let mut sums = vec![0u32; w.max(h) * 4];
    transpose(data, &mut a, w, h);
    for &(before, after) in &passes {
        box_columns(&a, &mut b, h * 4, w, before, after, &mut sums[..h * 4]);
        std::mem::swap(&mut a, &mut b);
    }
    transpose(&a, &mut b, h, w);
    for (i, &(before, after)) in passes.iter().enumerate() {
        let out: &mut [u8] = if i == passes.len() - 1 { data } else { &mut a };
        box_columns(&b, out, w * 4, h, before, after, &mut sums[..w * 4]);
        if i < passes.len() - 1 {
            std::mem::swap(&mut a, &mut b);
        }
    }
}
/// RGBA pixels `w` wide and `h` high into `out` turned on their side (`h`
/// wide, `w` high).
fn transpose(src: &[u8], out: &mut [u8], w: usize, h: usize) {
    const BLOCK: usize = 16;
    for y0 in (0..h).step_by(BLOCK) {
        for x0 in (0..w).step_by(BLOCK) {
            for y in y0..(y0 + BLOCK).min(h) {
                for x in x0..(x0 + BLOCK).min(w) {
                    out[(x * h + y) * 4..(x * h + y) * 4 + 4].copy_from_slice(&src[(y * w + x) * 4..(y * w + x) * 4 + 4]);
                }
            }
        }
    }
}
/// `sum / size`, rounded, as a multiply: (sum · ⌈2¹⁶ / size⌉ + 2¹⁵) >> 16,
/// within a 255th of the division for any window a blur takes.
fn box_scale(size: usize) -> u32 {
    (65536 + size as u32 - 1) / size as u32
}
/// One box blur down every column of `rows` rows `stride` bytes wide, from
/// `data` into `out`: `sums` holds each byte column's window.
fn box_columns(data: &[u8], out: &mut [u8], stride: usize, rows: usize, before: usize, after: usize, sums: &mut [u32]) {
    let scale = box_scale(before + after + 1);
    sums.fill(0);
    for row in data.chunks_exact(stride).take(after + 1) {
        for (s, &v) in sums.iter_mut().zip(row) {
            *s += u32::from(v);
        }
    }
    for i in 0..rows {
        for (o, &s) in out[i * stride..(i + 1) * stride].iter_mut().zip(sums.iter()) {
            *o = ((s * scale + 32768) >> 16) as u8;
        }
        if i + after + 1 < rows {
            for (s, &v) in sums.iter_mut().zip(&data[(i + after + 1) * stride..(i + after + 2) * stride]) {
                *s += u32::from(v);
            }
        }
        if i >= before {
            for (s, &v) in sums.iter_mut().zip(&data[(i - before) * stride..(i - before + 1) * stride]) {
                *s -= u32::from(v);
            }
        }
    }
}
/// `src` scaled up by the whole number `k` onto `out`, its origin at
/// (`ox`, `oy`) in `out`'s pixels, bilinear (as tiny-skia samples, pixel
/// centres to pixel centres), in fixed point and a row at a time: a
/// blurred layer back at full resolution without the general pipeline,
/// which was half the cost of a blurred tile.
fn upscale(src: &Pixmap, k: f64, ox: f64, oy: f64, out: &mut Pixmap) {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (w, h) = (out.width() as usize, out.height() as usize);
    if sw == 0 || sh == 0 {
        return;
    }
    // Where an output pixel's centre falls: the source pixel before it and
    // the next one's weight out of 256, both clamped to the source.
    let place = |at: usize, origin: f64, len: usize| -> (usize, usize, u32) {
        let u = (at as f64 + 0.5 - origin) / k - 0.5;
        let i = u.floor();
        let f = ((u - i) * 256.).round() as u32;
        let last = len as f64 - 1.;
        (i.clamp(0., last) as usize, (i + 1.).clamp(0., last) as usize, f.min(256))
    };
    let columns: Vec<(usize, usize, u32)> = (0..w).map(|x| place(x, ox, sw)).collect();
    let src = src.data();
    // Source rows scaled across (×256), the two the current output row
    // reads, kept while the rows go down.
    let across = |r: usize, into: &mut [u16]| {
        let row = &src[r * sw * 4..(r + 1) * sw * 4];
        for (o, &(a, b, f)) in into.chunks_exact_mut(4).zip(&columns) {
            for c in 0..4 {
                o[c] = (u32::from(row[a * 4 + c]) * (256 - f) + u32::from(row[b * 4 + c]) * f) as u16;
            }
        }
    };
    let mut upper = vec![0u16; w * 4];
    let mut lower = vec![0u16; w * 4];
    let mut have: (usize, usize) = (usize::MAX, usize::MAX);
    let data = out.data_mut();
    for y in 0..h {
        let (a, b, f) = place(y, oy, sh);
        if have.0 != a {
            if have.1 == a {
                std::mem::swap(&mut upper, &mut lower);
            } else {
                across(a, &mut upper);
            }
            have.0 = a;
        }
        if have.1 != b {
            across(b, &mut lower);
            have.1 = b;
        }
        for ((o, &t), &l) in data[y * w * 4..(y + 1) * w * 4].iter_mut().zip(&upper).zip(&lower) {
            *o = ((u32::from(t) * (256 - f) + u32::from(l) * f + 32768) >> 16) as u8;
        }
    }
}
/// A group's `mix-blend-mode` (its index in the scene's list) as the
/// raster composites with it.
fn blend_mode(code: u8) -> tiny_skia::BlendMode {
    use tiny_skia::BlendMode as B;
    match code {
        1 => B::Multiply,
        2 => B::Screen,
        3 => B::Overlay,
        4 => B::Darken,
        5 => B::Lighten,
        6 => B::ColorDodge,
        7 => B::ColorBurn,
        8 => B::HardLight,
        9 => B::SoftLight,
        10 => B::Difference,
        11 => B::Exclusion,
        12 => B::Hue,
        13 => B::Saturation,
        14 => B::Color,
        15 => B::Luminosity,
        16 => B::Plus,
        _ => B::SourceOver,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A layer of `w` × `h` with a few opaque shapes on it.
    fn shapes(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for (x, y, s, c) in [(5., 7., 20., [220, 40, 40]), (30., 18., 9., [30, 60, 230]), (12., 33., 25., [250, 210, 0])] {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(c[0], c[1], c[2], 255);
            p.fill_rect(tiny_skia::Rect::from_xywh(x, y, s, s * 0.7).unwrap(), &paint, Transform::identity(), None);
        }
        p
    }
    /// The blur as it was written first: a division per value, one column
    /// at a time.
    fn reference_blur(layer: &mut Pixmap, sigma: f64) {
        let d = (sigma * 3. * (2. * std::f64::consts::PI).sqrt() / 4. + 0.5).floor() as usize;
        let passes: [(usize, usize); 3] = if d % 2 == 1 { [(d / 2, d / 2); 3] } else { [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)] };
        let (w, h) = (layer.width() as usize, layer.height() as usize);
        let line = |get: &dyn Fn(usize, usize) -> u8, n: usize, before: usize, after: usize| -> Vec<[u8; 4]> {
            (0..n)
                .map(|i| {
                    let mut px = [0u8; 4];
                    for (c, v) in px.iter_mut().enumerate() {
                        let sum: u32 = (i.saturating_sub(before)..=(i + after).min(n - 1)).map(|j| u32::from(get(j, c))).sum();
                        let size = (before + after + 1) as u32;
                        *v = ((sum + size / 2) / size) as u8;
                    }
                    px
                })
                .collect()
        };
        for &(before, after) in &passes {
            for y in 0..h {
                let row = {
                    let data = layer.data();
                    line(&|x, c| data[(y * w + x) * 4 + c], w, before, after)
                };
                for (x, px) in row.iter().enumerate() {
                    layer.data_mut()[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(px);
                }
            }
        }
        for &(before, after) in &passes {
            for x in 0..w {
                let column = {
                    let data = layer.data();
                    line(&|y, c| data[(y * w + x) * 4 + c], h, before, after)
                };
                for (y, px) in column.iter().enumerate() {
                    layer.data_mut()[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(px);
                }
            }
        }
    }
    fn max_difference(a: &Pixmap, b: &Pixmap) -> u8 {
        a.data().iter().zip(b.data()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }
    #[test]
    fn blur_matches_the_direct_box_blurs() {
        for (w, h, sigma) in [(48, 40, 3.), (61, 37, 6.5), (40, 64, 12.)] {
            let mut fast = shapes(w, h);
            let mut slow = fast.clone();
            blur(&mut fast, sigma);
            reference_blur(&mut slow, sigma);
            // A multiply for each division: within rounding, pass by pass.
            assert!(max_difference(&fast, &slow) <= 2, "{w}×{h} at {sigma}: {}", max_difference(&fast, &slow));
        }
    }
    #[test]
    fn upscale_matches_bilinear_drawing() {
        let mut src = shapes(48, 40);
        blur(&mut src, 3.);
        for (k, ox, oy) in [(2., -6., -9.), (3., -4., -2.), (5., -11.5, -3.25)] {
            let mut fast = Pixmap::new(96, 80).unwrap();
            upscale(&src, k, ox, oy, &mut fast);
            let mut drawn = Pixmap::new(96, 80).unwrap();
            let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
            drawn.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_row(k as f32, 0., 0., k as f32, ox as f32, oy as f32), None);
            // Where the source covers the pixel (a blur's margin always
            // does): past its edge tiny-skia fades out, the upscale clamps.
            let inside = |at: usize, origin: f64, len: u32| {
                let u = (at as f64 + 0.5 - origin) / k - 0.5;
                u >= 0. && u <= f64::from(len) - 1.
            };
            let mut worst = 0;
            for y in (0..80).filter(|&y| inside(y, oy, src.height())) {
                for x in (0..96).filter(|&x| inside(x, ox, src.width())) {
                    let i = (y * 96 + x) * 4;
                    for c in 0..4 {
                        worst = worst.max(fast.data()[i + c].abs_diff(drawn.data()[i + c]));
                    }
                }
            }
            assert!(worst <= 2, "×{k} at ({ox}, {oy}): {worst}");
        }
    }
}
