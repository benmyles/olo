//! Box decoration: box shadows, backgrounds (colour, gradients) and borders
//! with rounded corners, drawn from the box's computed style at raster time.
//! Everything is clipped to the tile; shadows blur on an alpha layer.
use super::*;
use crate::{
    css::{
        math,
        properties::{Direction, Image, Keyword as K, Property as P, RadialSize, Stop},
        style::{Style, Value},
    },
    layout::Rect,
};
use tiny_skia::{BlendMode, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pattern, Pixmap, Point, RadialGradient, Shader, SpreadMode, Transform};
/// Each corner's horizontal and vertical radii in CSS pixels, top-left first.
pub type Radii = [[f64; 2]; 4];
struct Basis(f64);
impl math::Context for Basis {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        if unit == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
    }
}
fn resolve(n: &crate::css::properties::Numeric, basis: f64) -> f64 {
    resolve_any(n, basis).max(0.)
}
/// A computed length-percentage in CSS pixels, percentages of `basis`.
pub(super) fn resolve_any(n: &crate::css::properties::Numeric, basis: f64) -> f64 {
    // A keyword's constant percentage (`center` is 50%) is of the basis too.
    if let Some(q) = n.expression().constant()
        && q.ty.kind() == Some(math::Dimension::Percentage)
    {
        return q.value / 100. * basis;
    }
    n.resolve(&Basis(basis), &mut math::Scratch::default(), Default::default(), None).unwrap_or(0.)
}
/// The corner radii of a box, percentages against its size, scaled down
/// together when adjacent radii would overlap (CSS Backgrounds 3 §5.5).
pub fn radii(s: &Style, outer: Rect) -> Radii {
    let mut radii = [[0.; 2]; 4];
    for (i, p) in [P::BorderTopLeftRadius, P::BorderTopRightRadius, P::BorderBottomRightRadius, P::BorderBottomLeftRadius].into_iter().enumerate() {
        if let Value::Radius(r) = s.value(p) {
            radii[i] = [resolve(&r.x, outer.width), resolve(&r.y, outer.height)];
        }
    }
    let mut f: f64 = 1.;
    for (sum, side) in [(radii[0][0] + radii[1][0], outer.width), (radii[3][0] + radii[2][0], outer.width), (radii[0][1] + radii[3][1], outer.height), (radii[1][1] + radii[2][1], outer.height)] {
        if sum > side && sum > 0. {
            f = f.min(side / sum);
        }
    }
    if f < 1. {
        for r in &mut radii {
            r[0] *= f;
            r[1] *= f;
        }
    }
    radii
}
pub fn rounded(radii: &Radii) -> bool {
    radii.iter().any(|r| r[0] > 0. && r[1] > 0.)
}
/// A rounded rectangle path in device space.
pub fn path(r: Rect, radii: &Radii, scale: f64, region: DeviceRect) -> Option<Path> {
    let x0 = (r.x * scale - f64::from(region.x)) as f32;
    let y0 = (r.y * scale - f64::from(region.y)) as f32;
    let x1 = ((r.x + r.width) * scale - f64::from(region.x)) as f32;
    let y1 = ((r.y + r.height) * scale - f64::from(region.y)) as f32;
    if !(x1 > x0 && y1 > y0) || ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    if !rounded(radii) {
        return PathBuilder::from_rect(tiny_skia::Rect::from_ltrb(x0, y0, x1, y1)?).into();
    }
    let s = scale as f32;
    let rx = |i: usize| (radii[i][0] * f64::from(s)) as f32;
    let ry = |i: usize| (radii[i][1] * f64::from(s)) as f32;
    const K: f32 = 0.552_284_8;
    let mut b = PathBuilder::new();
    b.move_to(x0 + rx(0), y0);
    b.line_to(x1 - rx(1), y0);
    b.cubic_to(x1 - rx(1) + K * rx(1), y0, x1, y0 + ry(1) - K * ry(1), x1, y0 + ry(1));
    b.line_to(x1, y1 - ry(2));
    b.cubic_to(x1, y1 - ry(2) + K * ry(2), x1 - rx(2) + K * rx(2), y1, x1 - rx(2), y1);
    b.line_to(x0 + rx(3), y1);
    b.cubic_to(x0 + rx(3) - K * rx(3), y1, x0, y1 - ry(3) + K * ry(3), x0, y1 - ry(3));
    b.line_to(x0, y0 + ry(0));
    b.cubic_to(x0, y0 + ry(0) - K * ry(0), x0 + rx(0) - K * rx(0), y0, x0 + rx(0), y0);
    b.close();
    b.finish()
}
/// Radii of the box inset by its border (or any insets): each corner's
/// radii shrink by the adjacent sides, to zero.
pub fn inset_radii(radii: &Radii, insets: crate::layout::Edges) -> Radii {
    [
        [(radii[0][0] - insets.left).max(0.), (radii[0][1] - insets.top).max(0.)],
        [(radii[1][0] - insets.right).max(0.), (radii[1][1] - insets.top).max(0.)],
        [(radii[2][0] - insets.right).max(0.), (radii[2][1] - insets.bottom).max(0.)],
        [(radii[3][0] - insets.left).max(0.), (radii[3][1] - insets.bottom).max(0.)],
    ]
}
fn paint_for(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p
}
fn used_stops(stops: &[Stop<crate::css::color::Computed>], current: crate::css::color::Color, length: f64) -> Vec<(f32, [u8; 4])> {
    // Positions in [0, 1] of the gradient line; missing ones are spaced
    // evenly between their neighbours, and none goes before an earlier one.
    let mut out: Vec<(Option<f64>, [u8; 4])> = stops
        .iter()
        .map(|s| {
            let color = s.color.used(current).srgb8_clipped();
            let position = s.position.as_ref().map(|p| p.resolve(&Basis(length), &mut math::Scratch::default(), Default::default(), None).unwrap_or(0.) / length.max(1e-9));
            (position, color)
        })
        .collect();
    if out.is_empty() {
        return Vec::new();
    }
    if out[0].0.is_none() {
        out[0].0 = Some(0.);
    }
    let last = out.len() - 1;
    if out[last].0.is_none() {
        out[last].0 = Some(1.);
    }
    let mut floor = 0f64;
    for stop in &mut out {
        if let Some(p) = stop.0.as_mut() {
            *p = p.max(floor);
            floor = *p;
        }
    }
    let mut i = 0;
    while i < out.len() {
        if out[i].0.is_none() {
            let start = i - 1;
            let mut end = i;
            while out[end].0.is_none() {
                end += 1;
            }
            let (a, b) = (out[start].0.unwrap(), out[end].0.unwrap());
            for (k, stop) in out[start + 1..end].iter_mut().enumerate() {
                stop.0 = Some(a + (b - a) * (k + 1) as f64 / (end - start) as f64);
            }
            i = end;
        }
        i += 1;
    }
    out.into_iter().map(|(p, c)| (p.unwrap_or(0.).clamp(0., 1.) as f32, c)).collect()
}
fn gradient_stops(stops: Vec<(f32, [u8; 4])>) -> Vec<GradientStop> {
    stops.into_iter().map(|(p, c)| GradientStop::new(p, tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3]))).collect()
}
/// A gradient's shader over its positioning area, in device space.
fn shader(image: &Image<crate::css::color::Computed>, current: crate::css::color::Color, area: Rect, scale: f64, region: DeviceRect) -> Option<Shader<'static>> {
    let to_device = |x: f64, y: f64| Point::from_xy((x * scale - f64::from(region.x)) as f32, (y * scale - f64::from(region.y)) as f32);
    match image {
        Image::Linear { direction, stops, repeating } => {
            let (w, h) = (area.width, area.height);
            let angle = match direction {
                Direction::Angle(a) => {
                    let q = a.expression().constant()?;
                    // Computed angles are in degrees.
                    q.value.to_radians()
                }
                Direction::To { x, y } => match (x, y) {
                    (0, -1) => 0.,
                    (1, 0) => std::f64::consts::FRAC_PI_2,
                    (0, 1) => std::f64::consts::PI,
                    (-1, 0) => 3. * std::f64::consts::FRAC_PI_2,
                    (1, -1) => h.atan2(w),
                    (1, 1) => h.atan2(-w),
                    (-1, 1) => (-h).atan2(-w),
                    _ => (-h).atan2(w),
                },
            };
            let (sin, cos) = angle.sin_cos();
            let length = (w * sin).abs() + (h * cos).abs();
            let (cx, cy) = (area.x + w / 2., area.y + h / 2.);
            let dir = (sin, -cos);
            let start = to_device(cx - dir.0 * length / 2., cy - dir.1 * length / 2.);
            let end = to_device(cx + dir.0 * length / 2., cy + dir.1 * length / 2.);
            let stops = gradient_stops(used_stops(stops, current, length));
            if stops.len() < 2 {
                return None;
            }
            LinearGradient::new(start, end, stops, if *repeating { SpreadMode::Repeat } else { SpreadMode::Pad }, Transform::identity())
        }
        Image::Radial { circle, size, explicit, at, stops, repeating } => {
            let (w, h) = (area.width, area.height);
            let cx = area.x + resolve(&at.0, w);
            let cy = area.y + resolve(&at.1, h);
            let (dx0, dx1) = ((cx - area.x).abs(), (area.x + w - cx).abs());
            let (dy0, dy1) = ((cy - area.y).abs(), (area.y + h - cy).abs());
            let (rx, ry) = if let Some((ew, eh)) = explicit {
                (resolve(ew, w), resolve(eh, h))
            } else {
                let (sx, sy) = match size {
                    RadialSize::ClosestSide | RadialSize::ClosestCorner => (dx0.min(dx1), dy0.min(dy1)),
                    _ => (dx0.max(dx1), dy0.max(dy1)),
                };
                if *circle {
                    let r = match size {
                        RadialSize::ClosestSide => sx.min(sy),
                        RadialSize::FarthestSide => sx.max(sy),
                        RadialSize::ClosestCorner => [(dx0, dy0), (dx1, dy0), (dx0, dy1), (dx1, dy1)].into_iter().map(|(x, y)| x.hypot(y)).fold(f64::INFINITY, f64::min),
                        RadialSize::FarthestCorner => [(dx0, dy0), (dx1, dy0), (dx0, dy1), (dx1, dy1)].into_iter().map(|(x, y)| x.hypot(y)).fold(0., f64::max),
                    };
                    (r, r)
                } else if matches!(size, RadialSize::ClosestCorner | RadialSize::FarthestCorner) {
                    (sx * std::f64::consts::SQRT_2, sy * std::f64::consts::SQRT_2)
                } else {
                    (sx, sy)
                }
            };
            let (rx, ry) = (rx.max(1e-3), ry.max(1e-3));
            let stops = gradient_stops(used_stops(stops, current, rx));
            if stops.len() < 2 {
                return None;
            }
            let centre = to_device(cx, cy);
            // An ellipse is a circle scaled vertically about its centre.
            let transform = Transform::from_translate(centre.x, centre.y).pre_scale(1., (ry / rx) as f32).pre_translate(-centre.x, -centre.y);
            RadialGradient::new(centre, 0., centre, (rx * scale) as f32, stops, if *repeating { SpreadMode::Repeat } else { SpreadMode::Pad }, transform)
        }
        Image::Url(_) => None,
    }
}
/// Three box blurs approximate a Gaussian of this sigma (in device pixels).
fn blur(mask: &mut [u8], width: usize, height: usize, sigma: f64, cancel: Option<&AtomicBool>) -> Result<()> {
    if sigma <= 0.3 {
        return Ok(());
    }
    // Box sizes for three passes (Gwosdek et al.), odd widths.
    let w_ideal = (12. * sigma * sigma / 3. + 1.).sqrt();
    let mut wl = w_ideal.floor() as usize;
    if wl.is_multiple_of(2) {
        wl = wl.saturating_sub(1);
    }
    let wu = wl + 2;
    let m = ((12. * sigma * sigma - 3. * (wl * wl) as f64 - 12. * (wl as f64) - 9.) / (-4. * wl as f64 - 4.)).round().clamp(0., 3.) as usize;
    let mut scratch = vec![0u8; mask.len()];
    for pass in 0..3 {
        check(cancel)?;
        let size = if pass < m { wl } else { wu };
        let r = size / 2;
        if r == 0 {
            continue;
        }
        // Horizontal.
        for y in 0..height {
            let row = &mask[y * width..(y + 1) * width];
            let out = &mut scratch[y * width..(y + 1) * width];
            let mut sum: u32 = (0..=r.min(width.saturating_sub(1))).map(|x| u32::from(row[x])).sum();
            for x in 0..width {
                out[x] = (sum / (size as u32)).min(255) as u8;
                if x + r + 1 < width {
                    sum += u32::from(row[x + r + 1]);
                }
                if x >= r {
                    sum -= u32::from(row[x - r]);
                }
            }
        }
        // Vertical.
        for x in 0..width {
            let mut sum: u32 = (0..=r.min(height.saturating_sub(1))).map(|y| u32::from(scratch[y * width + x])).sum();
            for y in 0..height {
                mask[y * width + x] = (sum / (size as u32)).min(255) as u8;
                if y + r + 1 < height {
                    sum += u32::from(scratch[(y + r + 1) * width + x]);
                }
                if y >= r {
                    sum -= u32::from(scratch[(y - r) * width + x]);
                }
            }
        }
    }
    Ok(())
}
thread_local! {
    /// OLO_TILE_PROFILE: time in shadows, background colour, image layers,
    /// gradients and borders, taken per tile.
    pub(super) static PROFILE: std::cell::Cell<[f64; 8]> = const { std::cell::Cell::new([0.; 8]) };
}
fn charge(slot: usize, started: Option<std::time::Instant>) {
    if let Some(t) = started {
        PROFILE.with(|p| {
            let mut v = p.get();
            v[slot] += t.elapsed().as_secs_f64() * 1000.;
            p.set(v);
        });
    }
}
/// Draw a box's shadows, backgrounds and borders.
#[allow(clippy::too_many_arguments)]
pub(super) fn raster(
    out: &mut Pixmap,
    s: &Style,
    g: &crate::layout::Geometry,
    outer: Rect,
    scale: f64,
    region: DeviceRect,
    mask: Option<&Mask>,
    transform: Transform,
    engine: &mut Engine,
    tree: &crate::layout::Tree,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let limits = engine.limits();
    let radii = radii(s, outer);
    let Some(border_path) = path(outer, &radii, scale, region) else { return Ok(()) };
    let current = s.used_color(P::Color)?;
    let profiling = std::env::var_os("OLO_TILE_PROFILE").is_some();
    let now = || if profiling { Some(std::time::Instant::now()) } else { None };
    // Outer shadows, last declared first, each outside the border box.
    let started = now();
    if let Value::Shadows(shadows) = s.value(P::BoxShadow)
        && shadows.0.iter().any(|sh| !sh.inset)
    {
        let (w, h) = (region.width as usize, region.height as usize);
        if w.saturating_mul(h).saturating_mul(5) > limits.layer_bytes {
            return Err(Error::Limit);
        }
        // An axis-aligned transform (a scaled, moved container holding its
        // cards) scales and moves the shadow: the cached coverage, made at
        // the device size, still serves.
        let axis_aligned = if transform.is_identity() {
            Some((1f64, 1f64, 0f64, 0f64))
        } else if transform.kx == 0. && transform.ky == 0. && transform.sx > 0. && transform.sy > 0. && (transform.sx - transform.sy).abs() < 1e-3 {
            Some((f64::from(transform.sx), f64::from(transform.sy), f64::from(transform.tx), f64::from(transform.ty)))
        } else {
            None
        };
        let (ax, ay, tx, ty) = axis_aligned.unwrap_or((1., 1., 0., 0.));
        // Outer shadows paint only outside the border box: a tile wholly
        // inside it (past the corners) shows none.
        let largest = radii.iter().flat_map(|r| r.iter()).copied().fold(0f64, f64::max);
        let inner_left = (outer.x + largest) * scale * ax + tx;
        let inner_top = (outer.y + largest) * scale * ay + ty;
        let inner_right = (outer.x + outer.width - largest) * scale * ax + tx;
        let inner_bottom = (outer.y + outer.height - largest) * scale * ay + ty;
        let covered = axis_aligned.is_some()
            && f64::from(region.x) >= inner_left
            && f64::from(region.y) >= inner_top
            && f64::from(region.x) + f64::from(region.width) <= inner_right
            && f64::from(region.y) + f64::from(region.height) <= inner_bottom;
        // What is inside the box, cut from every shadow (made once).
        let mut inside: Option<Mask> = None;
        for shadow in shadows.0.iter().rev() {
            check(cancel)?;
            if shadow.inset || covered {
                continue;
            }
            let constant = |n: &crate::css::properties::Numeric| n.expression().constant().map_or(0., |q| q.value);
            let (dx, dy, blur_radius, spread) = (constant(&shadow.x), constant(&shadow.y), constant(&shadow.blur).max(0.), constant(&shadow.spread));
            let color = shadow.color.as_ref().map_or(current, |c| c.used(current)).srgb8_clipped();
            if color[3] == 0 {
                continue;
            }
            let rect = Rect { x: outer.x + dx - spread, y: outer.y + dy - spread, width: (outer.width + 2. * spread).max(0.), height: (outer.height + 2. * spread).max(0.) };
            let shadow_radii: Radii = radii.map(|r| [if r[0] > 0. { (r[0] + spread).max(0.) } else { 0. }, if r[1] > 0. { (r[1] + spread).max(0.) } else { 0. }]);
            let sigma = blur_radius * scale / 2.;
            // In device pixels, through the axis-aligned transform.
            let sigma_device = sigma * ax;
            let (dev_w, dev_h) = (rect.width * scale * ax, rect.height * scale * ay);
            if sigma_device > 0.3 && axis_aligned.is_some() && dev_w <= 8192. && dev_h <= 8192. {
                // A blurred shadow's coverage is the same in every tile and
                // frame that draws it: made once at its device size, and
                // composited here over the pixels it reaches, less the box.
                let margin = (sigma_device * 3.).ceil() as u32 + 1;
                let (dw, dh) = (dev_w.round().max(1.) as u32, dev_h.round().max(1.) as u32);
                let device_radii: Radii = shadow_radii.map(|r| [r[0] * scale * ax, r[1] * scale * ay]);
                let key = (dw, dh, device_radii.map(|r| [(r[0] * 64.).round() as i32, (r[1] * 64.).round() as i32]), sigma_device.to_bits());
                let cached = engine.shadow_mask(key, || {
                    let (width, height) = (dw + 2 * margin, dh + 2 * margin);
                    if (width as usize).saturating_mul(height as usize) > limits.layer_bytes {
                        return Err(Error::Limit);
                    }
                    let mut mask = Mask::new(width, height).ok_or(Error::Limit)?;
                    let whole = DeviceRect { x: 0, y: 0, width, height };
                    let at = Rect { x: f64::from(margin), y: f64::from(margin), width: f64::from(dw), height: f64::from(dh) };
                    if let Some(p) = path(at, &device_radii, 1., whole) {
                        mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
                    }
                    blur(mask.data_mut(), width as usize, height as usize, sigma_device, cancel)?;
                    Ok(super::glyph::ShadowMask { width, height, margin, data: mask.data().into() })
                })?;
                let ox = (rect.x * scale * ax + tx).round() as i64 - i64::from(cached.margin);
                let oy = (rect.y * scale * ay + ty).round() as i64 - i64::from(cached.margin);
                let (cw, ch) = (i64::from(cached.width), i64::from(cached.height));
                let (rx, ry) = (i64::from(region.x), i64::from(region.y));
                let x_start = (ox - rx).max(0);
                let x_end = (ox + cw - rx).min(w as i64);
                let y_start = (oy - ry).max(0);
                let y_end = (oy + ch - ry).min(h as i64);
                if x_end <= x_start || y_end <= y_start {
                    continue;
                }
                if inside.is_none() {
                    let t = now();
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    m.fill_path(&border_path, FillRule::Winding, true, transform);
                    inside = Some(m);
                    charge(5, t);
                }
                charge(6, Some(std::time::Instant::now()).filter(|_| profiling).map(|t| t - std::time::Duration::from_micros(1)));
                let inside = inside.as_ref().ok_or(Error::Invalid)?;
                let (cr, cg, cb, ca) = (u32::from(color[0]), u32::from(color[1]), u32::from(color[2]), u32::from(color[3]));
                let data = out.data_mut();
                // Only the ring outside the box's interior (inset by its
                // corner radii) can show a shadow: rows and columns inside
                // it are skipped whole.
                let (il, it, ir, ib) = (inner_left.ceil() as i64 - rx, inner_top.ceil() as i64 - ry, inner_right.floor() as i64 - rx, inner_bottom.floor() as i64 - ry);
                for ty in y_start..y_end {
                    let sy = (ry + ty - oy) as usize;
                    let src_row = sy * cached.width as usize;
                    let row = ty as usize * w;
                    let spans: [(i64, i64); 2] = if ty >= it && ty < ib { [(x_start, x_end.min(il)), (x_start.max(ir), x_end)] } else { [(x_start, x_end), (0, 0)] };
                    for (a0, a1) in spans {
                        if a1 <= a0 {
                            continue;
                        }
                        let src_x0 = (rx + a0 - ox) as usize;
                        let src = &cached.data[src_row + src_x0..src_row + src_x0 + (a1 - a0) as usize];
                        let inside_row = &inside.data()[row + a0 as usize..row + a1 as usize];
                        let clip_row = mask.map(|m| &m.data()[row + a0 as usize..row + a1 as usize]);
                        let dst = &mut data[(row + a0 as usize) * 4..(row + a1 as usize) * 4];
                        for (i, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                            let mut c = u32::from(src[i]);
                            if c == 0 {
                                continue;
                            }
                            c = c * (255 - u32::from(inside_row[i])) / 255;
                            if let Some(clip) = clip_row {
                                c = c * u32::from(clip[i]) / 255;
                            }
                            let a = (ca * c + 127) / 255;
                            if a == 0 {
                                continue;
                            }
                            let inverse = 255 - a;
                            d[0] = (((cr * a + 127) / 255) + (u32::from(d[0]) * inverse + 127) / 255).min(255) as u8;
                            d[1] = (((cg * a + 127) / 255) + (u32::from(d[1]) * inverse + 127) / 255).min(255) as u8;
                            d[2] = (((cb * a + 127) / 255) + (u32::from(d[2]) * inverse + 127) / 255).min(255) as u8;
                            d[3] = (a + (u32::from(d[3]) * inverse + 127) / 255).min(255) as u8;
                        }
                    }
                }
            } else if sigma_device <= 0.3 {
                // A hard shadow: its shape filled outside the box, through
                // the box's inverted coverage (and the clip).
                charge(7, Some(std::time::Instant::now()).filter(|_| profiling).map(|t| t - std::time::Duration::from_micros(1)));
                let Some(shadow_path) = path(rect, &shadow_radii, scale, region) else { continue };
                if inside.is_none() {
                    let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                    m.fill_path(&border_path, FillRule::Winding, true, transform);
                    inside = Some(m);
                }
                let mut outside = inside.as_ref().ok_or(Error::Invalid)?.clone();
                outside.invert();
                if let Some(clip) = mask {
                    for (o, c) in outside.data_mut().iter_mut().zip(clip.data()) {
                        *o = ((u32::from(*o) * u32::from(*c) + 127) / 255) as u8;
                    }
                }
                let mut p = paint_for(color);
                p.anti_alias = true;
                out.fill_path(&shadow_path, &p, FillRule::Winding, transform, Some(&outside));
            } else {
                charge(7, Some(std::time::Instant::now()).filter(|_| profiling).map(|t| t - std::time::Duration::from_micros(1)));
                if profiling {
                    eprintln!("  shadow fallback: sigma {sigma:.2}, blur {blur_radius}, transform {:?}, size {}x{}", transform, rect.width * scale, rect.height * scale);
                }
                let Some(shadow_path) = path(rect, &shadow_radii, scale, region) else { continue };
                let mut coverage = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
                coverage.fill_path(&shadow_path, FillRule::Winding, true, transform);
                if blur_radius > 0. {
                    blur(coverage.data_mut(), w, h, sigma, cancel)?;
                }
                let mut layer = Pixmap::new(region.width, region.height).ok_or(Error::Limit)?;
                layer.fill_rect(tiny_skia::Rect::from_xywh(0., 0., region.width as f32, region.height as f32).ok_or(Error::Invalid)?, &paint_for(color), Transform::identity(), Some(&coverage));
                let cut = Paint { blend_mode: BlendMode::DestinationOut, anti_alias: true, ..Default::default() };
                layer.fill_path(&border_path, &cut, FillRule::Winding, transform, None);
                out.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), mask);
            }
        }
    }
    charge(0, started);
    // Backgrounds: the colour under the images, each clipped to its box.
    let clip_box = match s.value(P::BackgroundClip) {
        Value::Keyword(K::PaddingBox) => 1,
        Value::Keyword(K::ContentBox) => 2,
        Value::Keyword(K::Text) => 3,
        _ => 0,
    };
    let padding_box = Rect {
        x: outer.x + g.border_width.left,
        y: outer.y + g.border_width.top,
        width: (outer.width - g.border_width.horizontal()).max(0.),
        height: (outer.height - g.border_width.vertical()).max(0.),
    };
    let content_box = Rect {
        x: padding_box.x + g.padding.left,
        y: padding_box.y + g.padding.top,
        width: (padding_box.width - g.padding.horizontal()).max(0.),
        height: (padding_box.height - g.padding.vertical()).max(0.),
    };
    let clip_path = match clip_box {
        1 => path(padding_box, &inset_radii(&radii, g.border_width), scale, region),
        2 => path(
            content_box,
            &inset_radii(
                &radii,
                crate::layout::Edges {
                    top: g.border_width.top + g.padding.top,
                    right: g.border_width.right + g.padding.right,
                    bottom: g.border_width.bottom + g.padding.bottom,
                    left: g.border_width.left + g.padding.left,
                },
            ),
            scale,
            region,
        ),
        3 => None, // Clipped to the text: not painted until glyph masks exist.
        _ => Some(border_path.clone()),
    };
    if let Some(clip_path) = &clip_path {
        let started = now();
        let bg = s.used_color(P::BackgroundColor)?.srgb8_clipped();
        if bg[3] != 0 {
            let mut p = paint_for(bg);
            p.anti_alias = rounded(&radii) || !transform.is_identity();
            out.fill_path(clip_path, &p, FillRule::Winding, transform, mask);
        }
        charge(1, started);
        if let Value::Images(images) = s.value(P::BackgroundImage) {
            let origin_box = match s.value(P::BackgroundOrigin) {
                Value::Keyword(K::BorderBox) => outer,
                Value::Keyword(K::ContentBox) => content_box,
                _ => padding_box,
            };
            for (layer, image) in images.0.iter().enumerate().rev() {
                check(cancel)?;
                let Some(image) = image else { continue };
                if let Image::Url(url) = image {
                    let started = now();
                    if let Some(bitmap) = tree.background(url) {
                        image_layer(out, s, (P::BackgroundSize, P::BackgroundPosition, P::BackgroundRepeat), layer, bitmap, origin_box, clip_path, scale, region, mask, transform, engine, cancel)?;
                    }
                    charge(2, started);
                    continue;
                }
                let started = now();
                let Some(shader) = shader(image, current, origin_box, scale, region) else { continue };
                let p = Paint { shader, anti_alias: rounded(&radii) || !transform.is_identity(), ..Default::default() };
                out.fill_path(clip_path, &p, FillRule::Winding, transform, mask);
                charge(3, started);
            }
        }
    }
    // Borders: one colour and solid or painted as solid → the ring between
    // the outer and inner rounded rectangles; otherwise the straight painter.
    let widths = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left];
    let started = now();
    if widths.iter().any(|w| *w > 0.) {
        let colors: Vec<[u8; 4]> =
            [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor].iter().map(|p| s.used_color(*p).map(|c| c.srgb8_clipped())).collect::<std::result::Result<_, _>>()?;
        let visible: Vec<usize> = (0..4).filter(|&i| widths[i] > 0. && colors[i][3] != 0).collect();
        let uniform = visible.iter().all(|&i| colors[i] == colors[visible[0]]) && visible.len() == 4 || visible.is_empty();
        if rounded(&radii) && uniform {
            if let Some(&i) = visible.first() {
                let inner = Rect { x: padding_box.x, y: padding_box.y, width: padding_box.width, height: padding_box.height };
                let mut b = PathBuilder::new();
                b.push_path(&border_path);
                if let Some(inner_path) = path(inner, &inset_radii(&radii, g.border_width), scale, region) {
                    b.push_path(&inner_path);
                }
                if let Some(ring) = b.finish() {
                    let mut p = paint_for(colors[i]);
                    p.anti_alias = true;
                    out.fill_path(&ring, &p, FillRule::EvenOdd, transform, mask);
                }
            }
        } else {
            let inner = Rect {
                x: outer.x + g.border_width.left,
                y: outer.y + g.border_width.top,
                width: (outer.width - g.border_width.horizontal()).max(0.),
                height: (outer.height - g.border_width.vertical()).max(0.),
            };
            super::border::raster(out, outer, inner, [colors[0], colors[1], colors[2], colors[3]], scale, region, mask, transform, cancel)?;
        }
    }
    charge(4, started);
    Ok(())
}
/// One `background-image: url()` layer (CSS Backgrounds 3 §3): sized by
/// `background-size` against its positioning area, placed by
/// `background-position`, tiled by `background-repeat`, clipped to the
/// box's clip path. Drawn as a pattern of the image's nearest mip level.
#[allow(clippy::too_many_arguments)]
fn image_layer(
    out: &mut Pixmap,
    s: &Style,
    (size_property, position_property, repeat_property): (P, P, P),
    layer: usize,
    image: &std::sync::Arc<crate::image::Image>,
    area: Rect,
    clip_path: &Path,
    scale: f64,
    region: DeviceRect,
    mask: Option<&Mask>,
    transform: Transform,
    engine: &mut Engine,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    use crate::css::properties::{RepeatAxis, Size};
    let (natural_w, natural_h) = image.natural();
    if natural_w <= 0. || natural_h <= 0. || area.width <= 0. || area.height <= 0. {
        return Ok(());
    }
    let ratio = natural_w / natural_h;
    let nth = |n: usize| if n == 0 { 0 } else { layer % n };
    let (w, h) = match s.value(size_property) {
        Value::Sizes(sizes) if !sizes.0.is_empty() => match &sizes.0[nth(sizes.0.len())] {
            Size::Auto => (natural_w, natural_h),
            Size::Cover => {
                let f = (area.width / natural_w).max(area.height / natural_h);
                (natural_w * f, natural_h * f)
            }
            Size::Contain => {
                let f = (area.width / natural_w).min(area.height / natural_h);
                (natural_w * f, natural_h * f)
            }
            Size::Explicit(w, h) => {
                let w = w.as_ref().map(|w| resolve_any(w, area.width));
                let h = h.as_ref().map(|h| resolve_any(h, area.height));
                match (w, h) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w / ratio),
                    (None, Some(h)) => (h * ratio, h),
                    (None, None) => (natural_w, natural_h),
                }
            }
        },
        _ => (natural_w, natural_h),
    };
    if !(w.is_finite() && h.is_finite()) || w <= 0. || h <= 0. {
        return Ok(());
    }
    // Percentages of background-position are of the free space.
    let (x, y) = match s.value(position_property) {
        Value::Positions(p) if !p.0.is_empty() => {
            let (px, py) = &p.0[nth(p.0.len())];
            (resolve_any(px, area.width - w), resolve_any(py, area.height - h))
        }
        _ => (0., 0.),
    };
    let (rx, ry) = match s.value(repeat_property) {
        Value::Repeats(r) if !r.0.is_empty() => r.0[nth(r.0.len())],
        _ => (RepeatAxis::Repeat, RepeatAxis::Repeat),
    };
    let repeat_x = rx != RepeatAxis::NoRepeat;
    let repeat_y = ry != RepeatAxis::NoRepeat;
    let surface = super::image::surface(engine, image, w * scale, h * scale, cancel)?;
    let Some(pixmap) = surface.pixmap() else { return Ok(()) };
    // The tile's origin and size in untransformed device space.
    let origin = Point::from_xy(((area.x + x) * scale - f64::from(region.x)) as f32, ((area.y + y) * scale - f64::from(region.y)) as f32);
    let (tile_w, tile_h) = ((w * scale) as f32, (h * scale) as f32);
    let pattern = Transform::from_scale(tile_w / pixmap.width() as f32, tile_h / pixmap.height() as f32).post_translate(origin.x, origin.y);
    let spread = if repeat_x || repeat_y { SpreadMode::Repeat } else { SpreadMode::Pad };
    let paint = Paint { shader: Pattern::new(pixmap, spread, FilterQuality::Bilinear, 1., pattern), anti_alias: true, ..Default::default() };
    if repeat_x && repeat_y {
        out.fill_path(clip_path, &paint, FillRule::Winding, transform, mask);
        return Ok(());
    }
    // Without repeat along an axis, only the tile's band along it paints.
    let far = 1e7f32;
    let Some(band) =
        tiny_skia::Rect::from_xywh(if repeat_x { -far } else { origin.x }, if repeat_y { -far } else { origin.y }, if repeat_x { 2. * far } else { tile_w }, if repeat_y { 2. * far } else { tile_h })
    else {
        return Ok(());
    };
    if (region.width as usize).saturating_mul(region.height as usize) > engine.limits().layer_bytes {
        return Err(Error::Limit);
    }
    let mut m = match mask {
        Some(mask) => mask.clone(),
        None => {
            let mut m = Mask::new(region.width, region.height).ok_or(Error::Limit)?;
            m.data_mut().fill(255);
            m
        }
    };
    m.intersect_path(&PathBuilder::from_rect(band), FillRule::Winding, true, transform);
    out.fill_path(clip_path, &paint, FillRule::Winding, transform, Some(&m));
    Ok(())
}
/// The box's `mask-image` layers drawn into `out` (a transparent surface
/// the size of the tile): their alpha is the mask (CSS Masking 1 §7),
/// each sized, placed and tiled like a background over the border box.
#[allow(clippy::too_many_arguments)]
pub(super) fn mask_layers(
    out: &mut Pixmap,
    s: &Style,
    outer: Rect,
    scale: f64,
    region: DeviceRect,
    transform: Transform,
    engine: &mut Engine,
    tree: &crate::layout::Tree,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let Value::Images(images) = s.value(P::MaskImage) else { return Ok(()) };
    let Some(clip_path) = path(outer, &[[0.; 2]; 4], scale, region) else { return Ok(()) };
    let current = s.used_color(P::Color)?;
    for (layer, image) in images.0.iter().enumerate().rev() {
        check(cancel)?;
        match image {
            Some(Image::Url(url)) => {
                if let Some(bitmap) = tree.background(url) {
                    image_layer(out, s, (P::MaskSize, P::MaskPosition, P::MaskRepeat), layer, bitmap, outer, &clip_path, scale, region, None, transform, engine, cancel)?;
                }
            }
            // A gradient mask: its alpha over the border box (mask-origin
            // and mask-clip's default), which also bounds what the group
            // shows, as mask-clip does.
            Some(gradient) => {
                let Some(shader) = shader(gradient, current, outer, scale, region) else { continue };
                let p = Paint { shader, anti_alias: !transform.is_identity(), ..Default::default() };
                out.fill_path(&clip_path, &p, FillRule::Winding, transform, None);
            }
            None => {}
        }
    }
    Ok(())
}
/// How far a box's outer shadows extend past its border box, per side.
pub fn shadow_extent(s: &Style) -> crate::layout::Edges {
    let mut e = crate::layout::Edges::default();
    if let Value::Shadows(shadows) = s.value(P::BoxShadow) {
        for shadow in shadows.0.iter().filter(|s| !s.inset) {
            let constant = |n: &crate::css::properties::Numeric| n.expression().constant().map_or(0., |q| q.value);
            let (dx, dy, blur, spread) = (constant(&shadow.x), constant(&shadow.y), constant(&shadow.blur).max(0.), constant(&shadow.spread));
            let reach = blur + spread;
            e.left = e.left.max(reach - dx);
            e.right = e.right.max(reach + dx);
            e.top = e.top.max(reach - dy);
            e.bottom = e.bottom.max(reach + dy);
        }
    }
    e
}
