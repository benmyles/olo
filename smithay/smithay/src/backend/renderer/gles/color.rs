//! Colour management in the renderer (Olo's): what a frame draws into
//! (a [`ColorTarget`]: an SDR display's colours, HDR's blending buffer,
//! an HDR display's PQ signal) and what a texture holds (a
//! [`ColorSource`]: sRGB unless it says). When they differ, textures are
//! drawn through the colour program (`COLOR` in `texture.frag`): decoded
//! to light, converted between primaries (white adapted), scaled between
//! luminances, clipped to what the target holds, encoded again; solid
//! colours are converted on the CPU. Without a target and a source,
//! nothing changes and nothing costs more.
//!
//! Light is reckoned relative to SDR's white: sRGB's 1.0 is `sdr_nits`
//! on an HDR display, and PQ's signal is absolute (10,000 cd/m² at 1.0).

pub use olo_color::{Mat3, Primaries, Transfer};

use super::{Uniform, UniformName, UniformType, UniformValue};

/// What a frame draws into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTarget {
    /// The target's primaries and white.
    pub primaries: Primaries,
    /// How it encodes light: `Gamma22` for an SDR display (and for HDR's
    /// blending buffer, so blending looks as on SDR), `Pq` for an HDR
    /// display's signal.
    pub transfer: Transfer,
    /// The light, cd/m², SDR's white is shown at (what 1.0 means in a
    /// `Gamma22` target; PQ's 1.0 is 10,000).
    pub sdr_nits: f32,
    /// The most light it takes, relative to SDR's white, clipped above
    /// (1.0 for an SDR display's buffer; HDR's: the display's peak).
    pub max: f32,
}

impl ColorTarget {
    /// An SDR display with these primaries.
    pub fn sdr(primaries: Primaries) -> ColorTarget {
        ColorTarget {
            primaries,
            transfer: Transfer::Gamma22,
            sdr_nits: 203.0,
            max: 1.0,
        }
    }

    /// What everything is drawn for without colour management.
    pub const SRGB: ColorTarget = ColorTarget {
        primaries: Primaries::SRGB,
        transfer: Transfer::Gamma22,
        sdr_nits: 203.0,
        max: 1.0,
    };
}

/// What a texture holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSource {
    /// Its primaries and white.
    pub primaries: Primaries,
    /// How it encodes light. sRGB content is taken as gamma 2.2, as a
    /// display shows it.
    pub transfer: Transfer,
}

impl ColorSource {
    /// What nearly everything is drawn in.
    pub const SRGB: ColorSource = ColorSource {
        primaries: Primaries::SRGB,
        transfer: Transfer::Gamma22,
    };
}

/// The colour program's uniforms for drawing `source` into `target`;
/// None when nothing changes.
pub(super) fn uniforms(source: &ColorSource, target: &ColorTarget) -> Option<Vec<Uniform<'static>>> {
    let same_primaries = source.primaries == target.primaries;
    if same_primaries
        && source.transfer == target.transfer
        && target.transfer != Transfer::Pq
        && target.max >= 1.0
    {
        return None;
    }
    let sdr = f64::from(target.sdr_nits.max(1.0));
    // Into light relative to SDR's white.
    let scale_in = if source.transfer == Transfer::Pq {
        10_000.0 / sdr
    } else {
        1.0
    };
    // Out of it, into the target's encoding.
    let scale_out = if target.transfer == Transfer::Pq {
        sdr / 10_000.0
    } else {
        1.0
    };
    let m = if same_primaries {
        Mat3::IDENTITY
    } else {
        source.primaries.to(&target.primaries)
    };
    let m = Mat3(m.0.map(|row| row.map(|v| v * scale_in)));
    Some(vec![
        Uniform::new(
            "color_matrix",
            UniformValue::Matrix3x3 {
                matrices: vec![m.to_gl()],
                transpose: false,
            },
        ),
        Uniform::new("tf_in", code(source.transfer)),
        Uniform::new("tf_out", code(target.transfer)),
        Uniform::new("out_scale", scale_out as f32),
        Uniform::new("out_max", target.max),
    ])
}

/// The program's uniforms, by name and type.
pub(super) fn uniform_names() -> [UniformName<'static>; 5] {
    [
        UniformName::new("color_matrix", UniformType::Matrix3x3),
        UniformName::new("tf_in", UniformType::_1f),
        UniformName::new("tf_out", UniformType::_1f),
        UniformName::new("out_scale", UniformType::_1f),
        UniformName::new("out_max", UniformType::_1f),
    ]
}

/// A transfer function as the shader knows it.
fn code(t: Transfer) -> f32 {
    match t {
        Transfer::Gamma22 => 0.0,
        Transfer::Pq => 1.0,
        Transfer::Linear => 2.0,
        Transfer::Srgb => 3.0,
    }
}

/// A premultiplied colour (`[r, g, b, a]`) from `source` into `target`,
/// as the colour program converts a texture's pixels.
pub fn convert(color: [f32; 4], source: &ColorSource, target: &ColorTarget) -> [f32; 4] {
    if uniforms(source, target).is_none() || color[3] <= 0.0 {
        return color;
    }
    let a = f64::from(color[3]);
    let sdr = f64::from(target.sdr_nits.max(1.0));
    let scale_in = if source.transfer == Transfer::Pq {
        10_000.0 / sdr
    } else {
        1.0
    };
    let scale_out = if target.transfer == Transfer::Pq {
        sdr / 10_000.0
    } else {
        1.0
    };
    let lin = [0, 1, 2].map(|i| source.transfer.decode(f64::from(color[i]) / a) * scale_in);
    let m = if source.primaries == target.primaries {
        Mat3::IDENTITY
    } else {
        source.primaries.to(&target.primaries)
    };
    let out = m.apply(lin).map(|v| {
        target
            .transfer
            .encode(v.clamp(0.0, f64::from(target.max)) * scale_out)
            * a
    });
    [out[0] as f32, out[1] as f32, out[2] as f32, color[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_changes_for_srgb() {
        assert!(uniforms(&ColorSource::SRGB, &ColorTarget::SRGB).is_none());
        assert_eq!(
            convert([0.5, 0.25, 0.1, 1.0], &ColorSource::SRGB, &ColorTarget::SRGB),
            [0.5, 0.25, 0.1, 1.0]
        );
    }

    #[test]
    fn srgb_into_p3_and_into_hdr() {
        let p3 = ColorTarget::sdr(Primaries::DISPLAY_P3);
        // Greys stay as they are; red less red, a little green.
        let grey = convert([0.5, 0.5, 0.5, 1.0], &ColorSource::SRGB, &p3);
        assert!(grey.iter().take(3).all(|v| (v - 0.5).abs() < 1e-5), "{grey:?}");
        let red = convert([1.0, 0.0, 0.0, 1.0], &ColorSource::SRGB, &p3);
        assert!(red[0] < 0.95 && red[1] > 0.1 && red[2] > 0.0, "{red:?}");
        // Premultiplied stays premultiplied: half of the same red.
        let half = convert([0.5, 0.0, 0.0, 0.5], &ColorSource::SRGB, &p3);
        assert!((half[0] * 2.0 - red[0]).abs() < 1e-5 && half[3] == 0.5);
        // SDR's white into PQ at 203 cd/m²: 0.58.
        let hdr = ColorTarget {
            primaries: Primaries::BT2020,
            transfer: Transfer::Pq,
            sdr_nits: 203.0,
            max: 1600.0 / 203.0,
        };
        let white = convert([1.0, 1.0, 1.0, 1.0], &ColorSource::SRGB, &hdr);
        assert!(
            white.iter().take(3).all(|v| (v - 0.5807).abs() < 1e-3),
            "{white:?}"
        );
        // PQ's 1000 cd/m² through HDR's blending buffer (gamma 2.2,
        // relative to SDR's white) and out again.
        let blend = ColorTarget {
            primaries: Primaries::BT2020,
            transfer: Transfer::Gamma22,
            sdr_nits: 203.0,
            max: 1600.0 / 203.0,
        };
        let pq = olo_color::pq_encode(0.1) as f32;
        let mid = convert(
            [pq, pq, pq, 1.0],
            &ColorSource {
                primaries: Primaries::BT2020,
                transfer: Transfer::Pq,
            },
            &blend,
        );
        assert!(mid[0] > 1.0, "{mid:?}");
        let out = convert(
            mid,
            &ColorSource {
                primaries: Primaries::BT2020,
                transfer: Transfer::Gamma22,
            },
            &hdr,
        );
        assert!((out[0] - pq).abs() < 1e-3, "{out:?} {pq}");
    }
}
