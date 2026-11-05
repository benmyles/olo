//! Original conversions, using CIE/standard transfer functions and the
//! normative matrix data. Arithmetic stays f64 until the destination packs.
use super::matrices::*;
use super::*;
const WHITE_D50: [f64; 3] = [0.3457 / 0.3585, 1., (1. - 0.3457 - 0.3585) / 0.3585];
const EPSILON: f64 = 216. / 24389.;
const KAPPA: f64 = 24389. / 27.;
#[inline]
pub(super) fn convert(color: &Color, target: Space) -> Color {
    if color.space == target {
        return Color::new(target, color.components(), color.alpha(), 0);
    }
    let components = color.components();
    // Avoid matrix round trips between polar/cartesian representations
    // and between sRGB's hue models and gamma-encoded RGB.
    let converted = match (color.space, target) {
        (Space::Lab, Space::Lch) | (Space::Oklab, Space::Oklch) => polar(components),
        (Space::Lch, Space::Lab) | (Space::Oklch, Space::Oklab) => cartesian(components),
        (Space::Hsl, Space::Srgb) => hsl_to_rgb(components),
        (Space::Hwb, Space::Srgb) => hwb_to_rgb(components),
        (Space::Srgb, Space::Hsl) => rgb_to_hsl(components),
        (Space::Srgb, Space::Hwb) => rgb_to_hwb(components),
        _ => from_xyz(to_xyz(components, color.space), target),
    };
    let (converted, missing) = powerless(converted, target);
    Color::new(target, converted, color.alpha(), missing)
}
#[cfg(feature = "baseline-color-conversion")]
pub(super) fn batch(_colors: &mut [Color], _target: Space) -> bool {
    false
}
pub(super) fn use_batch(source: Option<Space>, target: Space) -> bool {
    !cfg!(feature = "baseline-color-conversion") && (source == Some(target) || matches!(target, Space::SrgbLinear | Space::DisplayP3Linear | Space::XyzD50 | Space::XyzD65))
}
// Keep vector helper stack/register costs out of the nonlinear scalar loop.
#[inline(never)]
pub(super) fn scalar_step(colors: &mut [Color], target: Space, cancelled: Option<&AtomicBool>, converted: &mut usize) -> Result<()> {
    for colors in colors.chunks_mut(4) {
        check(cancelled)?;
        // Borrow directly: a by-value call copied each Color to the stack
        // in one release build, adding store-forwarding stalls. The
        // independently measured reference build happened to elide it.
        for color in colors.iter_mut() {
            *color = convert(color, target);
        }
        *converted += colors.len();
    }
    Ok(())
}
/// Four independent colors, retaining scalar operation order and f64
/// arithmetic. `wide` uses safe AVX, paired SSE2/NEON or scalar fallback
/// according to build target features. No target-specific unsafe engine code.
#[cfg(not(feature = "baseline-color-conversion"))]
pub(super) fn batch(colors: &mut [Color], target: Space) -> bool {
    let Ok(colors) = <&mut [Color; 4]>::try_from(colors) else {
        return false;
    };
    let source = colors[0].space;
    if colors.iter().any(|c| c.space != source) {
        return false;
    }
    if source == target {
        for color in colors {
            color.missing = 0;
        }
        return true;
    }
    let (Some(forward), Some(reverse)) = (linear_matrix(source, false), linear_matrix(target, true)) else {
        return false;
    };
    let values = std::array::from_fn(|axis| wide::f64x4::new(std::array::from_fn(|lane| unpack(colors[lane].channels[axis]))));
    let values = vector_dot(reverse, vector_dot(forward, values));
    let values = values.map(wide::f64x4::to_array);
    for (lane, color) in colors.iter_mut().enumerate() {
        color.channels[..3].copy_from_slice(&std::array::from_fn::<_, 3, _>(|axis| pack(values[axis][lane])));
        color.space = target;
        color.missing = 0;
    }
    true
}
#[cfg(not(feature = "baseline-color-conversion"))]
fn linear_matrix(space: Space, reverse: bool) -> Option<Option<[[f64; 3]; 3]>> {
    Some(match (space, reverse) {
        (Space::SrgbLinear, false) => Some(SRGB_TO_XYZ),
        (Space::SrgbLinear, true) => Some(XYZ_TO_SRGB),
        (Space::DisplayP3Linear, false) => Some(P3_TO_XYZ),
        (Space::DisplayP3Linear, true) => Some(XYZ_TO_P3),
        (Space::XyzD50, false) => Some(D50_TO_D65),
        (Space::XyzD50, true) => Some(D65_TO_D50),
        (Space::XyzD65, _) => None,
        _ => return None,
    })
}
#[cfg(not(feature = "baseline-color-conversion"))]
#[inline(always)]
fn vector_dot(matrix: Option<[[f64; 3]; 3]>, value: [wide::f64x4; 3]) -> [wide::f64x4; 3] {
    match matrix {
        Some(matrix) => {
            let row = |r: [f64; 3]| value[0] * wide::f64x4::splat(r[0]) + value[1] * wide::f64x4::splat(r[1]) + value[2] * wide::f64x4::splat(r[2]);
            [row(matrix[0]), row(matrix[1]), row(matrix[2])]
        }
        None => value,
    }
}
fn dot(matrix: [[f64; 3]; 3], value: [f64; 3]) -> [f64; 3] {
    matrix.map(|r| r[0] * value[0] + r[1] * value[1] + r[2] * value[2])
}
fn signed_power(value: f64, power: f64) -> f64 {
    value.abs().powf(power).copysign(value)
}
fn decode_srgb(v: f64) -> f64 {
    if v.abs() <= 0.04045 { v / 12.92 } else { ((v.abs() + 0.055) / 1.055).powf(2.4).copysign(v) }
}
fn encode_srgb(v: f64) -> f64 {
    if v.abs() <= 0.0031308 { v * 12.92 } else { (1.055 * v.abs().powf(1. / 2.4) - 0.055).copysign(v) }
}
fn decode_prophoto(v: f64) -> f64 {
    if v.abs() <= 16. / 512. { v / 16. } else { signed_power(v, 1.8) }
}
fn encode_prophoto(v: f64) -> f64 {
    if v.abs() < 1. / 512. { v * 16. } else { signed_power(v, 1. / 1.8) }
}
fn to_xyz(v: [f64; 3], space: Space) -> [f64; 3] {
    match space {
        Space::Srgb => dot(SRGB_TO_XYZ, v.map(decode_srgb)),
        Space::SrgbLinear => dot(SRGB_TO_XYZ, v),
        Space::DisplayP3 => dot(P3_TO_XYZ, v.map(decode_srgb)),
        Space::DisplayP3Linear => dot(P3_TO_XYZ, v),
        Space::A98Rgb => dot(A98_TO_XYZ, v.map(|x| signed_power(x, 563. / 256.))),
        Space::ProphotoRgb => dot(D50_TO_D65, dot(PROPHOTO_TO_XYZ, v.map(decode_prophoto))),
        // CSS Color 4's current reference EOTF is BT.1886 gamma 2.4.
        Space::Rec2020 => dot(REC2020_TO_XYZ, v.map(|x| signed_power(x, 2.4))),
        Space::XyzD65 => v,
        Space::XyzD50 => dot(D50_TO_D65, v),
        Space::Lab => dot(D50_TO_D65, lab_to_xyz(v)),
        Space::Lch => dot(D50_TO_D65, lab_to_xyz(cartesian(v))),
        Space::Oklab => dot(LMS_TO_XYZ, dot(OKLAB_TO_LMS, v).map(|x| x * x * x)),
        Space::Oklch => to_xyz(cartesian(v), Space::Oklab),
        Space::Hsl => to_xyz(hsl_to_rgb(v), Space::Srgb),
        Space::Hwb => to_xyz(hwb_to_rgb(v), Space::Srgb),
    }
}
fn from_xyz(v: [f64; 3], space: Space) -> [f64; 3] {
    match space {
        Space::Srgb => dot(XYZ_TO_SRGB, v).map(encode_srgb),
        Space::SrgbLinear => dot(XYZ_TO_SRGB, v),
        Space::DisplayP3 => dot(XYZ_TO_P3, v).map(encode_srgb),
        Space::DisplayP3Linear => dot(XYZ_TO_P3, v),
        Space::A98Rgb => dot(XYZ_TO_A98, v).map(|x| signed_power(x, 256. / 563.)),
        Space::ProphotoRgb => dot(XYZ_TO_PROPHOTO, dot(D65_TO_D50, v)).map(encode_prophoto),
        Space::Rec2020 => dot(XYZ_TO_REC2020, v).map(|x| signed_power(x, 1. / 2.4)),
        Space::XyzD65 => v,
        Space::XyzD50 => dot(D65_TO_D50, v),
        Space::Lab => xyz_to_lab(dot(D65_TO_D50, v)),
        Space::Lch => polar(xyz_to_lab(dot(D65_TO_D50, v))),
        Space::Oklab => dot(LMS_TO_OKLAB, dot(XYZ_TO_LMS, v).map(f64::cbrt)),
        Space::Oklch => polar(from_xyz(v, Space::Oklab)),
        Space::Hsl => rgb_to_hsl(from_xyz(v, Space::Srgb)),
        Space::Hwb => rgb_to_hwb(from_xyz(v, Space::Srgb)),
    }
}
fn lab_to_xyz([l, a, b]: [f64; 3]) -> [f64; 3] {
    let y = l / 116.;
    let offset = 16. / 116.;
    let x = y + a / 500.;
    let z = y - b / 200.;
    let inverse = |v: f64| if (v + offset).powi(3) > EPSILON { (v + offset).powi(3) } else { v * (116. / KAPPA) };
    [inverse(x) * WHITE_D50[0], if l > 8. { ((l + 16.) / 116.).powi(3) } else { l / KAPPA }, inverse(z) * WHITE_D50[2]]
}
fn xyz_to_lab(v: [f64; 3]) -> [f64; 3] {
    let f = std::array::from_fn::<_, 3, _>(|i| {
        let x = v[i] / WHITE_D50[i];
        if x > EPSILON { x.cbrt() } else { (KAPPA * x + 16.) / 116. }
    });
    [116. * f[1] - 16., 500. * (f[0] - f[1]), 200. * (f[1] - f[2])]
}
fn cartesian([l, c, h]: [f64; 3]) -> [f64; 3] {
    let (sin, cos) = h.to_radians().sin_cos();
    [l, c * cos, c * sin]
}
fn polar([l, a, b]: [f64; 3]) -> [f64; 3] {
    [l, a.hypot(b), hue(b.atan2(a).to_degrees())]
}
fn hsl_to_rgb([h, s, l]: [f64; 3]) -> [f64; 3] {
    let l = l / 100.;
    let chroma = (1. - (2. * l - 1.).abs()) * s / 100.;
    let x = chroma * (1. - ((hue(h) / 60.).rem_euclid(2.) - 1.).abs());
    let m = l - chroma / 2.;
    let rgb = match (hue(h) / 60.) as u8 {
        0 => [chroma, x, 0.],
        1 => [x, chroma, 0.],
        2 => [0., chroma, x],
        3 => [0., x, chroma],
        4 => [x, 0., chroma],
        _ => [chroma, 0., x],
    };
    rgb.map(|v| v + m)
}
fn hwb_to_rgb([h, w, b]: [f64; 3]) -> [f64; 3] {
    let w = w / 100.;
    let b = b / 100.;
    if w + b >= 1. { [w / (w + b); 3] } else { hsl_to_rgb([h, 100., 50.]).map(|x| x * (1. - w - b) + w) }
}
fn rgb_to_hsl(v: [f64; 3]) -> [f64; 3] {
    let min = v.into_iter().fold(f64::INFINITY, f64::min);
    let max = v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let delta = max - min;
    let l = (max + min) / 2.;
    if delta == 0. {
        return [0., 0., l * 100.];
    }
    let mut h = rgb_hue(v, min, max);
    let mut s = if l == 0. || l == 1. { 0. } else { (max - l) / l.min(1. - l) };
    if s < 0. {
        h = hue(h + 180.);
        s = -s;
    }
    [h, s * 100., l * 100.]
}
fn rgb_to_hwb(v: [f64; 3]) -> [f64; 3] {
    let min = v.into_iter().fold(f64::INFINITY, f64::min);
    let max = v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    [rgb_hue(v, min, max), min * 100., (1. - max) * 100.]
}
fn rgb_hue(v: [f64; 3], min: f64, max: f64) -> f64 {
    let d = max - min;
    if d == 0. {
        return 0.;
    }
    let sector = if max == v[0] {
        (v[1] - v[2]) / d
    } else if max == v[1] {
        (v[2] - v[0]) / d + 2.
    } else {
        (v[0] - v[1]) / d + 4.
    };
    hue(sector * 60.)
}
fn powerless(mut values: [f64; 3], space: Space) -> ([f64; 3], u8) {
    let (axis, chroma, epsilon) = match space {
        Space::Hsl => (0, 1, 0.001),
        Space::Lch => (2, 1, 0.0015),
        Space::Oklch => (2, 1, 0.000004),
        Space::Hwb => {
            if values[1] + values[2] >= 99.999 {
                if values[1] + values[2] < 100. {
                    values[2] = 100. - values[1];
                }
                values[0] = 0.;
                return (values, 1);
            }
            return (values, 0);
        }
        _ => return (values, 0),
    };
    if values[chroma] <= epsilon {
        values[axis] = 0.;
        if values[chroma] > 0. {
            values[chroma] = 0.;
        }
        (values, 1 << axis)
    } else {
        (values, 0)
    }
}
