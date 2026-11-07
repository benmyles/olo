//! Bilinear sampling anchored in global device coordinates. Tile-local inverse
//! f32 transforms change interpolation rounding at tile boundaries.
use super::mipmap::{Cache, Plane};
use super::*;
use crate::layout::Rect;
use std::sync::Arc;
use tiny_skia::{FilterQuality, Mask, Paint, Pattern, Pixmap, SpreadMode, Transform};
// Normalized binary16 texels, retained as a tiny lookup over the original
// RGBA8 storage. Native Chrome observations distinguish this from rounding
// only the final filtered value. No expanded copy of an image is retained.
const fn texels() -> [f64; 256] {
    let mut table = [0.; 256];
    let mut i = 1;
    while i < 256 {
        let quantum = 1u32 << (11 + (i as u8).leading_zeros());
        let normalized = (i as u32 * quantum + 127) / 255;
        table[i] = normalized as f64 * 255. / quantum as f64;
        i += 1;
    }
    table
}
const TEXELS: [f64; 256] = texels();
struct Tap {
    a: usize,
    b: usize,
    f: f64,
}
fn tap(at: f64, origin: f64, extent: f64, pixels: u32) -> Tap {
    let source = ((at + 0.5 - origin) * (f64::from(pixels) / extent) - 0.5).clamp(0., f64::from(pixels - 1));
    let a = source.floor() as usize;
    Tap { a, b: (a + 1).min(pixels as usize - 1), f: ((source - a as f64) * 256.).round() / 256. }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn raster(
    engine: &mut Engine,
    out: &mut Pixmap,
    image: &Arc<crate::image::Image>,
    bounds: Rect,
    scale: f64,
    region: DeviceRect,
    mask: Option<&Mask>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    // Geometry lives on a 1/64 device-pixel layout grid. Recover that grid
    // before snapping, so arithmetic around a half-pixel cannot lose a row.
    // Negative ties round toward +infinity, as positive layout snapping does.
    let snap = |v: f64| (((v * scale * 64.).round() / 64.) + 0.5).floor();
    let (left, top, right, bottom) = (snap(bounds.x), snap(bounds.y), snap(bounds.x + bounds.width), snap(bounds.y + bounds.height));
    let (width, height) = (right - left, bottom - top);
    if ![left, top, width, height, left + width, top + height].into_iter().all(f64::is_finite) {
        return Err(Error::Invalid);
    }
    if width <= 0. || height <= 0. {
        return Ok(());
    }
    let x0 = left.floor().max(f64::from(region.x));
    let y0 = top.floor().max(f64::from(region.y));
    let x1 = (left + width).ceil().min(f64::from(region.x) + f64::from(region.width));
    let y1 = (top + height).ceil().min(f64::from(region.y) + f64::from(region.height));
    if x0 >= x1 || y0 >= y1 {
        return Ok(());
    }
    // Source-pixel mipmaps, with the sharpening bias used by the reference
    // backend. This is a filtering policy, not a promise of GPU LOD bit identity.
    let lod =
        if width < f64::from(image.width()) && height < f64::from(image.height()) { ((f64::from(image.width()) / width).max(f64::from(image.height()) / height).log2() - 0.5).max(0.) } else { 0. };
    let max_level = 31 - image.width().max(image.height()).leading_zeros();
    let lod = lod.min(f64::from(max_level));
    let (mut upper_level, mut lower_level, mut weight) = (None, None, 0.);
    if lod > 0. {
        let limits = engine.limits();
        let cache = engine.image_mips.get_or_insert_with(|| Box::new(Cache::new(limits.image_mip_bytes, limits.image_mip_sources)));
        let upper = lod.floor() as u32;
        let lower = (upper + 1).min(max_level);
        let get = |cache: &mut Cache, level| match cache.get(image, level, cancel) {
            Ok(level) => Ok(Some(level)),
            Err(Error::Limit) => Ok(None), // bounded lower-quality fallback
            Err(error) => Err(error),
        };
        if upper != 0 {
            upper_level = get(cache, upper)?;
        }
        if upper == 0 || upper_level.is_some() {
            lower_level = get(cache, lower)?;
            if lower_level.is_some() {
                weight = lod - f64::from(upper);
            }
        }
    }
    let upper = upper_level.as_ref().map_or_else(|| Plane::from(image.as_ref()), |p| p.plane());
    let lower = lower_level.as_ref().map(|p| p.plane());
    let frame = Frame { left, top, width, height, x0, y0, x1, y1 };
    if let Some(lower) = lower { draw::<true>(out, upper, lower, weight, frame, region, mask, cancel) } else { draw::<false>(out, upper, upper, 0., frame, region, mask, cancel) }
}
struct Frame {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}
#[allow(clippy::too_many_arguments)]
fn draw<const BLEND: bool>(out: &mut Pixmap, upper: Plane<'_>, lower: Plane<'_>, weight: f64, frame: Frame, region: DeviceRect, mask: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
    let Frame { left, top, width, height, x0, y0, x1, y1 } = frame;
    // Only columns intersecting this bounded output tile, never a full resized
    // image. Work is cached in the caller's retained raster tiles.
    let mut xs = Vec::with_capacity((x1 - x0) as usize);
    for x in 0..(x1 - x0) as usize {
        check(cancel)?;
        xs.push(tap(x0 + x as f64, left, width, upper.width));
    }
    let lower_xs = if BLEND { (0..xs.len()).map(|x| tap(x0 + x as f64, left, width, lower.width)).collect::<Vec<_>>() } else { Vec::new() };
    let out_stride = region.width as usize * 4;
    let data = out.data_mut();
    for gy in y0 as i64..y1 as i64 {
        check(cancel)?;
        let y = tap(gy as f64, top, height, upper.height);
        let lower_y = if BLEND { tap(gy as f64, top, height, lower.height) } else { Tap { a: 0, b: 0, f: 0. } };
        let row = (gy - i64::from(region.y)) as usize;
        for (i, x) in xs.iter().enumerate() {
            if i % 128 == 0 {
                check(cancel)?;
            }
            let col = (x0 - f64::from(region.x)) as usize + i;
            let m = mask.map_or(255, |m| m.data()[row * region.width as usize + col]);
            if m == 0 {
                continue;
            }
            // Snapped image edges and integer tile coordinates cover complete
            // device pixels. Only the clip mask contributes edge coverage.
            let coverage = f64::from(m) / 255.;
            let mut sample = [0u32; 4];
            for (c, value) in sample.iter_mut().enumerate() {
                let a = channel(upper, x, &y, c);
                let filtered = if BLEND { a + (channel(lower, &lower_xs[i], &lower_y, c) - a) * weight } else { a };
                *value = (filtered * coverage).round().clamp(0., 255.) as u32;
            }
            let at = row * out_stride + col * 4;
            let inverse = 255 - sample[3];
            for c in 0..4 {
                data[at + c] = (sample[c] + (u32::from(data[at + c]) * inverse + 127) / 255).min(255) as u8;
            }
        }
    }
    Ok(())
}
#[inline]
fn channel(p: Plane<'_>, x: &Tap, y: &Tap, c: usize) -> f64 {
    let stride = p.width as usize * 4;
    let a = TEXELS[p.rgba[y.a * stride + x.a * 4 + c] as usize];
    let b = TEXELS[p.rgba[y.a * stride + x.b * 4 + c] as usize];
    let d = TEXELS[p.rgba[y.b * stride + x.a * 4 + c] as usize];
    let e = TEXELS[p.rgba[y.b * stride + x.b * 4 + c] as usize];
    let upper = a + (b - a) * x.f;
    let lower = d + (e - d) * x.f;
    upper + (lower - upper) * y.f
}

/// Draw an image through a CSS transform: the source (at the mip level
/// nearest its drawn size) sampled bilinearly as a pattern over its box,
/// composited in one fill. Anchored in device space like the axis-aligned
/// path, but resampled: transformed pixels have no device grid to snap to.
#[allow(clippy::too_many_arguments)]
pub(super) fn transformed(
    engine: &mut Engine,
    out: &mut Pixmap,
    image: &Arc<crate::image::Image>,
    bounds: Rect,
    scale: f64,
    region: DeviceRect,
    transform: Transform,
    mask: Option<&Mask>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let Some(rect) =
        tiny_skia::Rect::from_xywh((bounds.x * scale - f64::from(region.x)) as f32, (bounds.y * scale - f64::from(region.y)) as f32, (bounds.width * scale) as f32, (bounds.height * scale) as f32)
    else {
        return Ok(());
    };
    // The drawn size of the image, for its mip level: the box's size through
    // the matrix's scale, so a shrunk image is averaged, not skipped through.
    let (sx, sy) = ((transform.sx.hypot(transform.ky)) as f64, (transform.kx.hypot(transform.sy)) as f64);
    let (width, height) = (bounds.width * scale * sx, bounds.height * scale * sy);
    if !(width.is_finite() && height.is_finite()) {
        return Err(Error::Invalid);
    }
    if width <= 0. || height <= 0. {
        return Ok(());
    }
    let pixmap = surface(engine, image, width, height, cancel)?;
    let pattern = Transform::from_scale(rect.width() / pixmap.width() as f32, rect.height() / pixmap.height() as f32).post_translate(rect.x(), rect.y());
    let paint = Paint { shader: Pattern::new((*pixmap).as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1., pattern), anti_alias: true, ..Default::default() };
    out.fill_rect(rect, &paint, transform, mask);
    Ok(())
}
/// The image as a tiny-skia surface at the mip level nearest `width` ×
/// `height` device pixels, for drawing through a pattern shader.
pub(super) fn surface(engine: &mut Engine, image: &Arc<crate::image::Image>, width: f64, height: f64, cancel: Option<&AtomicBool>) -> Result<Arc<Pixmap>> {
    let max_level = 31 - image.width().max(image.height()).leading_zeros();
    let lod =
        if width < f64::from(image.width()) && height < f64::from(image.height()) { ((f64::from(image.width()) / width).max(f64::from(image.height()) / height).log2() - 0.5).max(0.) } else { 0. };
    let level = (lod.round() as u32).min(max_level);
    let source = if level == 0 {
        None
    } else {
        let limits = engine.limits();
        let cache = engine.image_mips.get_or_insert_with(|| Box::new(Cache::new(limits.image_mip_bytes, limits.image_mip_sources)));
        match cache.get(image, level, cancel) {
            Ok(level) => Some(level),
            Err(Error::Limit) => None,
            Err(error) => return Err(error),
        }
    };
    let plane = source.as_ref().map_or_else(|| Plane::from(image.as_ref()), |p| p.plane());
    engine.premultiplied(image, level, plane, cancel)
}
impl Engine {
    /// The image's pixels at a mip level as a tiny-skia surface (the same
    /// premultiplied RGBA8), kept for the next tile that draws the same one.
    fn premultiplied(&mut self, image: &Arc<crate::image::Image>, level: u32, plane: Plane<'_>, cancel: Option<&AtomicBool>) -> Result<Arc<Pixmap>> {
        let key = (Arc::as_ptr(image) as usize, level);
        if let Some((k, p)) = &self.transformed_image
            && *k == key
        {
            return Ok(p.clone());
        }
        check(cancel)?;
        let size = tiny_skia::IntSize::from_wh(plane.width, plane.height).ok_or(Error::Invalid)?;
        let pixmap = Pixmap::from_vec(plane.rgba.to_vec(), size).ok_or(Error::Invalid)?;
        let pixmap = Arc::new(pixmap);
        self.transformed_image = Some((key, pixmap.clone()));
        Ok(pixmap)
    }
}
