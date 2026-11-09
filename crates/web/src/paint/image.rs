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
/// The same, single precision, as the sampler filters them: exact to well
/// within the rounding of its result.
const TEXELS32: [f32; 256] = {
    let mut table = [0f32; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = TEXELS[i] as f32;
        i += 1;
    }
    table
};
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
thread_local! {
    /// OLO_TILE_PROFILE: images drawn per path (copied, blended, masked,
    /// sampled, transformed), each a count and milliseconds.
    pub(super) static PROFILE: std::cell::Cell<[(u32, f64); 5]> = const { std::cell::Cell::new([(0, 0.); 5]) };
}
fn profiled(path: usize, started: Option<std::time::Instant>) {
    if let Some(t) = started {
        PROFILE.with(|p| {
            let mut v = p.get();
            v[path].0 += 1;
            v[path].1 += t.elapsed().as_secs_f64() * 1000.;
            p.set(v);
        });
    }
}
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
    placed(engine, out, image, [left, top, right - left, bottom - top], false, region, mask, cancel)
}
/// Draw an image upright at `[left, top, width, height]` in device pixels:
/// on whole pixels as `raster` snaps it, or (`fractional`, an image only
/// scaled and moved by a transform) where it falls, its edge pixels
/// covered by as much of them as it covers.
#[allow(clippy::too_many_arguments)]
fn placed(
    engine: &mut Engine,
    out: &mut Pixmap,
    image: &Arc<crate::image::Image>,
    [left, top, width, height]: [f64; 4],
    fractional: bool,
    region: DeviceRect,
    mask: Option<&Mask>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
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
    // Drawn at its own size on whole device pixels (a canvas, an icon at
    // 1:1): rows copied and composited, no sampling.
    let started = std::env::var_os("OLO_TILE_PROFILE").map(|_| std::time::Instant::now());
    let whole = !fractional || (left.fract() == 0. && top.fract() == 0.);
    if whole && width == f64::from(image.width()) && height == f64::from(image.height()) {
        let result = direct(out, image.rgba(), image.width() as usize, image.opaque(), Frame { left, top, width, height, x0, y0, x1, y1 }, region, mask, cancel);
        profiled(
            if mask.is_some() {
                2
            } else if image.opaque() {
                0
            } else {
                1
            },
            started,
        );
        return result;
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
    let edges = !whole || width.fract() != 0. || height.fract() != 0.;
    let result = match (lower, edges) {
        (Some(lower), false) => draw::<true, false>(out, upper, lower, weight, frame, region, mask, cancel),
        (Some(lower), true) => draw::<true, true>(out, upper, lower, weight, frame, region, mask, cancel),
        (None, false) => draw::<false, false>(out, upper, upper, 0., frame, region, mask, cancel),
        (None, true) => draw::<false, true>(out, upper, upper, 0., frame, region, mask, cancel),
    };
    profiled(3, started);
    result
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
/// One source row filtered across at the drawn columns (`Tap`s), each a
/// premultiplied pixel in texel units: what bilinear sampling needs of it,
/// kept for every drawn row that reads it.
fn across(p: Plane<'_>, row: usize, xs: &[Tap], out: &mut Vec<[f32; 4]>) {
    let stride = p.width as usize * 4;
    let src = &p.rgba[row * stride..(row + 1) * stride];
    out.clear();
    out.extend(xs.iter().map(|x| {
        let (a, b) = (&src[x.a * 4..x.a * 4 + 4], &src[x.b * 4..x.b * 4 + 4]);
        let f = x.f as f32;
        let mut v = [0f32; 4];
        for c in 0..4 {
            let (a, b) = (TEXELS32[a[c] as usize], TEXELS32[b[c] as usize]);
            v[c] = a + (b - a) * f;
        }
        v
    }));
}
/// A plane's two source rows a drawn row reads, filtered across, kept as
/// the rows go down: an image scaled up reads each source row for several
/// drawn ones, and shrunk (a mip level or two over) about twice.
struct Rows {
    at: [usize; 2],
    rows: [Vec<[f32; 4]>; 2],
}
impl Rows {
    fn new() -> Self {
        Rows { at: [usize::MAX; 2], rows: [Vec::new(), Vec::new()] }
    }
    fn get(&mut self, p: Plane<'_>, y: &Tap, xs: &[Tap]) -> (&[[f32; 4]], &[[f32; 4]]) {
        if self.at[0] != y.a {
            if self.at[1] == y.a {
                self.rows.swap(0, 1);
                self.at.swap(0, 1);
            } else {
                across(p, y.a, xs, &mut self.rows[0]);
                self.at[0] = y.a;
            }
        }
        if self.at[1] != y.b {
            across(p, y.b, xs, &mut self.rows[1]);
            self.at[1] = y.b;
        }
        (&self.rows[0], &self.rows[1])
    }
}
#[allow(clippy::too_many_arguments)]
fn draw<const BLEND: bool, const EDGES: bool>(
    out: &mut Pixmap,
    upper: Plane<'_>,
    lower: Plane<'_>,
    weight: f64,
    frame: Frame,
    region: DeviceRect,
    mask: Option<&Mask>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let Frame { left, top, width, height, x0, y0, x1, y1 } = frame;
    // How much of device pixel `at` the image's span from `start` covers.
    let covered = |at: f64, start: f64, extent: f64| ((at + 1.).min(start + extent) - at.max(start)).clamp(0., 1.);
    // Only columns intersecting this bounded output tile, never a full resized
    // image. Work is cached in the caller's retained raster tiles.
    check(cancel)?;
    let columns = (x1 - x0) as usize;
    let xs: Vec<Tap> = (0..columns).map(|x| tap(x0 + x as f64, left, width, upper.width)).collect();
    let lower_xs: Vec<Tap> = if BLEND { (0..columns).map(|x| tap(x0 + x as f64, left, width, lower.width)).collect() } else { Vec::new() };
    let column_covered: Vec<f32> = if EDGES { (0..columns).map(|x| covered(x0 + x as f64, left, width) as f32).collect() } else { Vec::new() };
    let (mut upper_rows, mut lower_rows) = (Rows::new(), Rows::new());
    let weight = weight as f32;
    let out_stride = region.width as usize * 4;
    let col0 = (x0 - f64::from(region.x)) as usize;
    let data = out.data_mut();
    for gy in y0 as i64..y1 as i64 {
        check(cancel)?;
        let y = tap(gy as f64, top, height, upper.height);
        let row = (gy - i64::from(region.y)) as usize;
        let row_covered = if EDGES { covered(gy as f64, top, height) as f32 } else { 1. };
        let fy = y.f as f32;
        let (a, b) = upper_rows.get(upper, &y, &xs);
        let (c, d, lower_fy) = if BLEND {
            let lower_y = tap(gy as f64, top, height, lower.height);
            let (c, d) = lower_rows.get(lower, &lower_y, &lower_xs);
            (c, d, lower_y.f as f32)
        } else {
            (a, b, 0.)
        };
        let mask_row = mask.map(|m| &m.data()[row * region.width as usize + col0..row * region.width as usize + col0 + columns]);
        let out_row = &mut data[row * out_stride + col0 * 4..row * out_stride + (col0 + columns) * 4];
        for (i, px) in out_row.chunks_exact_mut(4).enumerate() {
            let m = mask_row.map_or(255, |m| m[i]);
            if m == 0 {
                continue;
            }
            // Snapped image edges and integer tile coordinates cover complete
            // device pixels: only the clip mask contributes edge coverage,
            // and the image's own edges where they fall between pixels.
            let mut coverage = f32::from(m) / 255.;
            if EDGES {
                coverage *= row_covered * column_covered[i];
            }
            let mut sample = [0u32; 4];
            for (ch, value) in sample.iter_mut().enumerate() {
                let near = a[i][ch] + (b[i][ch] - a[i][ch]) * fy;
                let filtered = if BLEND {
                    let far = c[i][ch] + (d[i][ch] - c[i][ch]) * lower_fy;
                    near + (far - near) * weight
                } else {
                    near
                };
                // (Rounded as +0.5 and cut: the same for what is never
                // negative, without a call to roundf a channel.)
                *value = ((filtered * coverage + 0.5) as u32).min(255);
            }
            let inverse = 255 - sample[3];
            for ch in 0..4 {
                px[ch] = (sample[ch] + (u32::from(px[ch]) * inverse + 127) / 255).min(255) as u8;
            }
        }
    }
    Ok(())
}
/// Premultiplied source pixels over the tile, one to one: opaque rows
/// unmasked are copied; others composited, the mask's coverage applied.
#[allow(clippy::too_many_arguments)]
fn direct(out: &mut Pixmap, rgba: &[u8], stride_px: usize, opaque: bool, frame: Frame, region: DeviceRect, mask: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
    let Frame { left, top, x0, y0, x1, y1, .. } = frame;
    let out_stride = region.width as usize * 4;
    let data = out.data_mut();
    let columns = (x1 - x0) as usize;
    let col0 = (x0 - f64::from(region.x)) as usize;
    let src_x0 = (x0 - left) as usize;
    for gy in y0 as i64..y1 as i64 {
        check(cancel)?;
        let row = (gy - i64::from(region.y)) as usize;
        let src_row = (gy - top as i64) as usize * stride_px * 4;
        let src = &rgba[src_row + src_x0 * 4..src_row + (src_x0 + columns) * 4];
        let dst = &mut data[row * out_stride + col0 * 4..row * out_stride + (col0 + columns) * 4];
        if mask.is_none() {
            if opaque {
                dst.copy_from_slice(src);
            } else {
                for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                    let sa = u32::from(s[3]);
                    if sa == 255 {
                        *d = *s;
                    } else if sa != 0 {
                        let inverse = 255 - sa;
                        d[0] = (u32::from(s[0]) + (u32::from(d[0]) * inverse + 127) / 255).min(255) as u8;
                        d[1] = (u32::from(s[1]) + (u32::from(d[1]) * inverse + 127) / 255).min(255) as u8;
                        d[2] = (u32::from(s[2]) + (u32::from(d[2]) * inverse + 127) / 255).min(255) as u8;
                        d[3] = (sa + (u32::from(d[3]) * inverse + 127) / 255).min(255) as u8;
                    }
                }
            }
            continue;
        }
        let mask_row = mask.map(|m| &m.data()[row * region.width as usize + col0..row * region.width as usize + col0 + columns]);
        for (i, (d, s)) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0).enumerate() {
            let m = mask_row.map_or(255, |m| u32::from(m[i]));
            if m == 0 {
                continue;
            }
            let (mut sr, mut sg, mut sb, mut sa) = (u32::from(s[0]), u32::from(s[1]), u32::from(s[2]), u32::from(s[3]));
            if m != 255 {
                sr = (sr * m + 127) / 255;
                sg = (sg * m + 127) / 255;
                sb = (sb * m + 127) / 255;
                sa = (sa * m + 127) / 255;
            }
            if sa == 255 {
                *d = [sr as u8, sg as u8, sb as u8, 255];
            } else if sa != 0 {
                let inverse = 255 - sa;
                d[0] = (sr + (u32::from(d[0]) * inverse + 127) / 255).min(255) as u8;
                d[1] = (sg + (u32::from(d[1]) * inverse + 127) / 255).min(255) as u8;
                d[2] = (sb + (u32::from(d[2]) * inverse + 127) / 255).min(255) as u8;
                d[3] = (sa + (u32::from(d[3]) * inverse + 127) / 255).min(255) as u8;
            }
        }
    }
    Ok(())
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
    // Only scaled and moved (Reddit's backdrops at scale(1.2), anything
    // centred by translate(-50%, -50%)): still upright, so drawn as an
    // untransformed image where it lands (between pixels as it may: no
    // snapping, as Chrome resamples it), through the axis-aligned sampler
    // and its mip blending, not tiny-skia's float pattern pipeline.
    if transform.kx == 0. && transform.ky == 0. && transform.sx > 0. && transform.sy > 0. {
        let (sx, sy) = (f64::from(transform.sx), f64::from(transform.sy));
        let (tx, ty) = (f64::from(transform.tx), f64::from(transform.ty));
        let (rx, ry) = (f64::from(region.x), f64::from(region.y));
        let moved = [(bounds.x * scale - rx) * sx + tx + rx, (bounds.y * scale - ry) * sy + ty + ry, bounds.width * scale * sx, bounds.height * scale * sy];
        if moved.into_iter().all(f64::is_finite) {
            return placed(engine, out, image, moved, true, region, mask, cancel);
        }
    }
    let started = std::env::var_os("OLO_TILE_PROFILE").map(|_| std::time::Instant::now());
    let result = transformed_inner(engine, out, image, bounds, scale, region, transform, mask, cancel);
    profiled(4, started);
    result
}
#[allow(clippy::too_many_arguments)]
fn transformed_inner(
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
    let surface = surface(engine, image, width, height, cancel)?;
    let Some(pixmap) = surface.pixmap() else { return Ok(()) };
    let pattern = Transform::from_scale(rect.width() / pixmap.width() as f32, rect.height() / pixmap.height() as f32).post_translate(rect.x(), rect.y());
    let paint = Paint { shader: Pattern::new(pixmap, SpreadMode::Pad, FilterQuality::Bilinear, 1., pattern), anti_alias: true, ..Default::default() };
    out.fill_rect(rect, &paint, transform, mask);
    Ok(())
}
/// The image's pixels at the mip level nearest `width` × `height` device
/// pixels, borrowed as a tiny-skia surface for a pattern shader: no copy,
/// since an image is premultiplied RGBA8 as tiny-skia wants it.
pub(super) struct Surface {
    image: Arc<crate::image::Image>,
    level: Option<Arc<super::mipmap::Level>>,
}
impl Surface {
    pub fn pixmap(&self) -> Option<tiny_skia::PixmapRef<'_>> {
        let plane = self.level.as_ref().map_or_else(|| Plane::from(self.image.as_ref()), |l| l.plane());
        tiny_skia::PixmapRef::from_bytes(plane.rgba, plane.width, plane.height)
    }
}
pub(super) fn surface(engine: &mut Engine, image: &Arc<crate::image::Image>, width: f64, height: f64, cancel: Option<&AtomicBool>) -> Result<Surface> {
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
    Ok(Surface { image: image.clone(), level: source })
}
