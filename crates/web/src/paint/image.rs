//! Bilinear sampling anchored in global device coordinates. Tile-local inverse
//! f32 transforms change interpolation rounding at tile boundaries.
use super::*;
use crate::layout::Rect;
use tiny_skia::{Mask, Pixmap};
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
    coverage: f64,
}
fn tap(at: f64, origin: f64, extent: f64, pixels: u32) -> Tap {
    let source = ((at + 0.5 - origin) * (f64::from(pixels) / extent) - 0.5).clamp(0., f64::from(pixels - 1));
    let a = source.floor() as usize;
    Tap { a, b: (a + 1).min(pixels as usize - 1), f: ((source - a as f64) * 256.).round() / 256., coverage: ((at + 1.).min(origin + extent) - at.max(origin)).clamp(0., 1.) }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn raster(out: &mut Pixmap, image: &crate::image::Image, bounds: Rect, scale: f64, region: DeviceRect, mask: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
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
    // Only columns intersecting this bounded output tile, never a full resized
    // image. Work is cached in the caller's retained raster tiles.
    let mut xs = Vec::with_capacity((x1 - x0) as usize);
    for x in 0..(x1 - x0) as usize {
        check(cancel)?;
        xs.push(tap(x0 + x as f64, left, width, image.width()));
    }
    let stride = image.width() as usize * 4;
    let source = image.rgba();
    let out_stride = region.width as usize * 4;
    let data = out.data_mut();
    for gy in y0 as i64..y1 as i64 {
        check(cancel)?;
        let y = tap(gy as f64, top, height, image.height());
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
            let coverage = x.coverage * y.coverage * f64::from(m) / 255.;
            let mut sample = [0u32; 4];
            for (c, value) in sample.iter_mut().enumerate() {
                let a = TEXELS[source[y.a * stride + x.a * 4 + c] as usize];
                let b = TEXELS[source[y.a * stride + x.b * 4 + c] as usize];
                let d = TEXELS[source[y.b * stride + x.a * 4 + c] as usize];
                let e = TEXELS[source[y.b * stride + x.b * 4 + c] as usize];
                let upper = a + (b - a) * x.f;
                let lower = d + (e - d) * x.f;
                *value = ((upper + (lower - upper) * y.f) * coverage).round().clamp(0., 255.) as u32;
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
