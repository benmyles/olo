//! Axis-aligned solid borders: disjoint strips and analytical corner coverage.
//! Each corner mixes premultiplied side colours before one source-over, so
//! adjacent translucent sides do not accumulate alpha along their join.
use super::*;
use crate::layout::Rect;
use tiny_skia::{Mask, Paint, Pixmap, PremultipliedColorU8, Transform};
#[derive(Clone, Copy)]
struct Edges {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
impl Edges {
    fn new(r: Rect, scale: f64, region: DeviceRect) -> Self {
        Self {
            left: (r.x * scale).round() - f64::from(region.x),
            top: (r.y * scale).round() - f64::from(region.y),
            right: ((r.x + r.width) * scale).round() - f64::from(region.x),
            bottom: ((r.y + r.height) * scale).round() - f64::from(region.y),
        }
    }
}
fn fill(out: &mut Pixmap, e: Edges, color: [u8; 4], mask: Option<&Mask>, transform: Transform) {
    if color[3] == 0 || e.left >= e.right || e.top >= e.bottom {
        return;
    }
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    if !transform.is_identity() {
        // Transformed: the strip lands where the matrix says; the tile clips.
        let Some(rect) = tiny_skia::Rect::from_ltrb(e.left as f32, e.top as f32, e.right as f32, e.bottom as f32) else { return };
        out.fill_rect(rect, &p, transform, mask);
        return;
    }
    if e.right <= 0. || e.bottom <= 0. || e.left >= f64::from(out.width()) || e.top >= f64::from(out.height()) {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_ltrb(e.left.max(0.) as f32, e.top.max(0.) as f32, e.right.min(f64::from(out.width())) as f32, e.bottom.min(f64::from(out.height())) as f32) else { return };
    p.anti_alias = false;
    out.fill_rect(rect, &p, Transform::identity(), mask);
}
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}
fn premul(c: [u8; 4]) -> [u32; 4] {
    [div255(u32::from(c[0]) * u32::from(c[3])), div255(u32::from(c[1]) * u32::from(c[3])), div255(u32::from(c[2]) * u32::from(c[3])), u32::from(c[3])]
}
#[allow(clippy::too_many_arguments)] // One bounded corner and its orientation/colours.
fn corner(out: &mut Pixmap, e: Edges, flip_x: bool, flip_y: bool, horizontal: [u8; 4], vertical: [u8; 4], mask: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
    if horizontal == vertical {
        fill(out, e, horizontal, mask, Transform::identity());
        return Ok(());
    }
    let width = e.right - e.left;
    let height = e.bottom - e.top;
    if width <= 0. || height <= 0. {
        return Ok(());
    }
    let from_x = e.left.max(0.) as usize;
    let to_x = e.right.min(f64::from(out.width())).max(0.) as usize;
    let from_y = e.top.max(0.) as usize;
    let to_y = e.bottom.min(f64::from(out.height())).max(0.) as usize;
    let a = premul(horizontal);
    let b = premul(vertical);
    let slope = height / width;
    let stride = out.width() as usize;
    for y in from_y..to_y {
        check(cancel)?;
        let v = if flip_y { e.bottom - y as f64 - 1. } else { y as f64 - e.top };
        for x in from_x..to_x {
            if (x - from_x).is_multiple_of(256) {
                check(cancel)?;
            }
            let at = y * stride + x;
            let clip = mask.map_or(255, |m| u32::from(m.data()[at]));
            if clip == 0 {
                continue;
            }
            let u = if flip_x { e.right - x as f64 - 1. } else { x as f64 - e.left };
            // Integral of clamp(slope*x-v, 0, 1) across this unit pixel.
            let start = (v / slope).clamp(u, u + 1.);
            let end = ((v + 1.) / slope).clamp(u, u + 1.);
            let area = ((slope * (start + end) * 0.5 - v) * (end - start) + (u + 1. - end)).clamp(0., 1.);
            let coverage = (area * 255.) as u32;
            let mut mixed = [0u32; 4];
            for i in 0..4 {
                mixed[i] = div255(div255(a[i] * coverage + b[i] * (255 - coverage)) * clip);
            }
            let dst = out.pixels_mut()[at];
            let inv = 255 - mixed[3];
            out.pixels_mut()[at] = PremultipliedColorU8::from_rgba(
                (mixed[0] + div255(u32::from(dst.red()) * inv)).min(255) as u8,
                (mixed[1] + div255(u32::from(dst.green()) * inv)).min(255) as u8,
                (mixed[2] + div255(u32::from(dst.blue()) * inv)).min(255) as u8,
                255,
            )
            .ok_or(Error::Invalid)?;
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Tile, device geometry, colours and cancellation.
pub(super) fn raster(
    out: &mut Pixmap,
    outer: Rect,
    inner: Rect,
    colors: [[u8; 4]; 4],
    scale: f64,
    region: DeviceRect,
    mask: Option<&Mask>,
    transform: Transform,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let o = Edges::new(outer, scale, region);
    let mut i = Edges::new(inner, scale, region);
    if ![o.left, o.top, o.right, o.bottom, i.left, i.top, i.right, i.bottom].into_iter().all(f64::is_finite) {
        return Err(Error::Invalid);
    }
    i.left = i.left.clamp(o.left, o.right);
    i.right = i.right.clamp(i.left, o.right);
    i.top = i.top.clamp(o.top, o.bottom);
    i.bottom = i.bottom.clamp(i.top, o.bottom);
    let top = Edges { left: i.left, top: o.top, right: i.right, bottom: i.top };
    let bottom = Edges { left: i.left, top: i.bottom, right: i.right, bottom: o.bottom };
    if colors.iter().all(|c| *c == colors[0]) || !transform.is_identity() {
        // One colour, or transformed (the corner coverage is analytic in
        // device space, so each side's colour fills its strip whole).
        fill(out, Edges { left: o.left, top: o.top, right: i.left, bottom: o.bottom }, colors[3], mask, transform);
        fill(out, Edges { left: i.right, top: o.top, right: o.right, bottom: o.bottom }, colors[1], mask, transform);
        fill(out, top, colors[0], mask, transform);
        fill(out, bottom, colors[2], mask, transform);
        return Ok(());
    }
    fill(out, top, colors[0], mask, transform);
    fill(out, bottom, colors[2], mask, transform);
    fill(out, Edges { left: o.left, top: i.top, right: i.left, bottom: i.bottom }, colors[3], mask, transform);
    fill(out, Edges { left: i.right, top: i.top, right: o.right, bottom: i.bottom }, colors[1], mask, transform);
    corner(out, Edges { left: o.left, top: o.top, right: i.left, bottom: i.top }, false, false, colors[0], colors[3], mask, cancel)?;
    corner(out, Edges { left: i.right, top: o.top, right: o.right, bottom: i.top }, true, false, colors[0], colors[1], mask, cancel)?;
    corner(out, Edges { left: i.right, top: i.bottom, right: o.right, bottom: o.bottom }, true, true, colors[2], colors[1], mask, cancel)?;
    corner(out, Edges { left: o.left, top: i.bottom, right: i.left, bottom: o.bottom }, false, true, colors[2], colors[3], mask, cancel)
}
