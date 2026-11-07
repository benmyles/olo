//! Isolated SVG effects. Retained preorder ranges avoid a second shape tree.
//! Scratch is cropped to each group's intersection with its parent tile.
use super::*;
use crate::svg::{Viewport, clip};
use tiny_skia::{FillRule, Mask, Paint, Pixmap, PixmapPaint, Transform};
struct Surface {
    layer: usize,
    x: i32,
    y: i32,
    image: Pixmap,
}
fn fill(out: &mut Pixmap, shape: &crate::svg::Shape, used: Option<&crate::svg::used::Geometry>, transform: Transform, mask: Option<&Mask>) {
    if let Some(path) = shape.geometry.path(used) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(shape.color[0], shape.color[1], shape.color[2], shape.color[3]);
        out.fill_path(path, &paint, if shape.evenodd { FillRule::EvenOdd } else { FillRule::Winding }, transform, mask);
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn raster(
    out: &mut Pixmap,
    vector: &Viewport,
    used: Option<&crate::svg::used::Geometry>,
    clips: &[clip::Resolved],
    transform: Transform,
    mask: Option<&Mask>,
    limits: Limits,
    work: &mut usize,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let layers = vector.layers();
    if layers.is_empty() {
        for shape in &vector.shapes {
            check(cancel)?;
            fill(out, shape, used, transform, mask);
        }
        return Ok(());
    }
    let mut stack: Vec<Surface> = Vec::new();
    let mut bytes = 0usize;
    let mut shape = 0usize;
    let mut next = 0usize;
    loop {
        check(cancel)?;
        if stack.last().is_some_and(|s| layers[s.layer].end as usize == shape && next >= layers[s.layer].end_layer as usize) {
            let mut surface = stack.pop().ok_or(Error::Invalid)?;
            let layer = &layers[surface.layer];
            if layer.clip != clip::NO_CLIP {
                let pixels = surface.image.width() as usize * surface.image.height() as usize;
                if bytes.checked_add(pixels).ok_or(Error::Limit)? > limits.layer_bytes {
                    return Err(Error::Limit);
                }
                let clip = clips.get(layer.clip as usize).ok_or(Error::Invalid)?;
                if let Some(path) = clip.path.as_ref() {
                    let path_bytes = size_of_val(path.verbs()).checked_add(size_of_val(path.points())).ok_or(Error::Limit)?;
                    if bytes.checked_add(pixels).and_then(|n| n.checked_add(path_bytes)).ok_or(Error::Limit)? > limits.layer_bytes {
                        return Err(Error::Limit);
                    }
                    *work = work.checked_add(path.verbs().len()).ok_or(Error::Limit)?;
                    if *work > limits.layer_pixel_work {
                        return Err(Error::Limit);
                    }
                    let path = path.clone().transform(transform.post_translate(-surface.x as f32, -surface.y as f32)).ok_or(Error::Unsupported("SVG clip transform precision"))?;
                    let b = path.bounds();
                    if [b.left(), b.top(), b.right(), b.bottom()].iter().any(|v| v.abs() > f32::MAX * 0.25) {
                        return Err(Error::Unsupported("SVG clip raster precision"));
                    }
                    let mut clip_mask = Mask::new(surface.image.width(), surface.image.height()).ok_or(Error::Limit)?;
                    clip_mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
                    surface.image.apply_mask(&clip_mask);
                } else {
                    surface.image.data_mut().fill(0);
                }
            }
            let paint = PixmapPaint { opacity: layer.opacity, ..Default::default() };
            if let Some(parent) = stack.last_mut() {
                parent.image.draw_pixmap(surface.x - parent.x, surface.y - parent.y, surface.image.as_ref(), &paint, Transform::identity(), None);
            } else {
                out.draw_pixmap(surface.x, surface.y, surface.image.as_ref(), &paint, Transform::identity(), mask);
            }
            bytes -= surface.image.data().len();
            continue;
        }
        if let Some(layer) = layers.get(next)
            && layer.first as usize == shape
        {
            if layer.end < layer.first || layer.end as usize > vector.shapes.len() || layer.end_layer as usize <= next || layer.end_layer as usize > layers.len() {
                return Err(Error::Invalid);
            }
            let mut area = None;
            // Keep the complete ink bounds, even when the clip is smaller.
            // Cropping the curve before scan conversion changes tiny-skia's
            // edge subdivision. Empty clip resources can still skip all work.
            let ink = used.map_or(layer.bounds, |g| g.layers[next]);
            let bounds = if layer.clip == clip::NO_CLIP { ink } else { clips.get(layer.clip as usize).ok_or(Error::Invalid)?.path.as_ref().as_ref().and(ink) };
            if layer.opacity > 0.
                && let Some(bounds) = bounds
            {
                let b = bounds.transform(transform).ok_or(Error::Unsupported("SVG layer transform precision"))?;
                let (px, py, pw, ph) = stack.last().map_or((0, 0, out.width(), out.height()), |s| (s.x, s.y, s.image.width(), s.image.height()));
                // Clamp before integer conversion: enormous offscreen geometry
                // never causes a large allocation or overflowing tile offset.
                let x = b.left().floor().max(px as f32).min((i64::from(px) + i64::from(pw)) as f32) as i32;
                let y = b.top().floor().max(py as f32).min((i64::from(py) + i64::from(ph)) as f32) as i32;
                let right = b.right().ceil().max(px as f32).min((i64::from(px) + i64::from(pw)) as f32) as i32;
                let bottom = b.bottom().ceil().max(py as f32).min((i64::from(py) + i64::from(ph)) as f32) as i32;
                if right > x && bottom > y {
                    area = Some((x, y, (right - x) as u32, (bottom - y) as u32));
                }
            }
            let Some((x, y, w, h)) = area else {
                shape = layer.end as usize;
                next = layer.end_layer as usize;
                continue;
            };
            let pixels = (w as usize).checked_mul(h as usize).ok_or(Error::Limit)?;
            let amount = pixels.checked_mul(4).ok_or(Error::Limit)?;
            bytes = bytes.checked_add(amount).ok_or(Error::Limit)?;
            *work = work.checked_add(pixels.checked_mul(if layer.clip == clip::NO_CLIP { 2 } else { 4 }).ok_or(Error::Limit)?).ok_or(Error::Limit)?;
            if stack.len() >= 128 || bytes > limits.layer_bytes || *work > limits.layer_pixel_work {
                return Err(Error::Limit);
            }
            stack.push(Surface { layer: next, x, y, image: Pixmap::new(w, h).ok_or(Error::Limit)? });
            next += 1;
            continue;
        }
        if shape == vector.shapes.len() {
            if !stack.is_empty() || next != layers.len() {
                return Err(Error::Invalid);
            }
            break;
        }
        let path = &vector.shapes[shape];
        if let Some(surface) = stack.last_mut() {
            fill(&mut surface.image, path, used, transform.post_translate(-surface.x as f32, -surface.y as f32), None);
        } else {
            fill(out, path, used, transform, mask);
        }
        shape += 1;
    }
    Ok(())
}
