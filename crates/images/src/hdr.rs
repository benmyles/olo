//! HDR and wide-gamut video shown on an ordinary (sRGB) screen: decoded
//! pictures in BT.2020 with PQ or HLG (HDR10, phones' HDR), or BT.2020
//! and Display P3 with SDR's curve, made sRGB a picture can show.
//!
//! Each sample's signal becomes light (PQ: absolute, cd/m²; HLG: scene
//! light through BT.2100's display mapping for a 1,000 cd/m² screen, its
//! gain by luminance), counted in SDR's white (BT.2408's 203 cd/m², what
//! an HDR picture's paper white is meant to be); BT.2020's colours are
//! converted to sRGB's in light; then highlights above SDR's white are
//! rolled off, by the brightest channel so colours keep their hue, from
//! four fifths of white up to the brightest the picture was mastered to
//! (1,000 cd/m² when it doesn't say: paper white then shows at about 90%,
//! room left for what's brighter); colours outside sRGB are clipped to
//! it (as players do: pulled towards their grey instead, BT.2020's green
//! turned mint); and it's encoded with sRGB's curve. Everything per sample is a table lookup or a few multiplies
//! (tables of 4096 steps, made once a picture): a 4K picture on four
//! threads.

/// H.273's colour primaries and transfer characteristics (ISO/IEC
/// 23091-2): those that matter here.
pub const PRI_BT709: u32 = 1;
pub const PRI_UNKNOWN: u32 = 2;
pub const PRI_BT2020: u32 = 9;
pub const PRI_P3_D65: u32 = 12;
pub const TRC_UNKNOWN: u32 = 2;
pub const TRC_PQ: u32 = 16;
pub const TRC_HLG: u32 = 18;

/// Steps in the tables between signal and light.
pub const STEPS: usize = 4096;

/// Whether pictures so described need converting (not plain sRGB-ish
/// video, which goes straight through).
pub fn needed(primaries: u32, transfer: u32) -> bool {
    matches!(transfer, TRC_PQ | TRC_HLG) || matches!(primaries, PRI_BT2020 | PRI_P3_D65)
}

/// How one picture's signal becomes sRGB.
pub struct Mapping {
    /// Signal (0–1 in `STEPS`) to light, in SDR whites (HLG: scene light).
    light: Vec<f32>,
    /// HLG's display gain by scene luminance (0–1 in `STEPS`), in SDR
    /// whites: BT.2100's OOTF for a 1,000 cd/m² screen.
    hlg_gain: Option<Vec<f32>>,
    /// The source's primaries to sRGB's, in light; None if the same.
    matrix: Option<[[f32; 3]; 3]>,
    /// Luminance weights in the source's primaries (for HLG's gain).
    weights: [f32; 3],
    /// Highlights: from `knee` (in SDR whites) rolled off to reach 1 at
    /// `peak`, as a scale for the brightest channel (`STEPS` from the
    /// knee to the peak: a multiply and a look, not two divisions).
    knee: f32,
    per_step: f32,
    roll: Vec<f32>,
    /// Light 0–1 (in `STEPS`) to sRGB's 8 bits.
    encode: Vec<u8>,
}

/// Whether pictures with these primaries are wider than sRGB (shown in
/// Display P3 where a window can draw in it, rather than clipped).
pub fn wide(primaries: u32) -> bool {
    matches!(primaries, PRI_BT2020 | PRI_P3_D65)
}

impl Mapping {
    /// For pictures with these primaries and transfer, mastered up to
    /// `peak` cd/m² (0 when it isn't said), to sRGB, or to Display P3
    /// (`p3`: its primaries, sRGB's curve, as Display P3 pictures are).
    pub fn new(primaries: u32, transfer: u32, peak: f32, p3: bool) -> Mapping {
        let white = olo_color::SDR_WHITE_NITS;
        let n = (STEPS - 1) as f64;
        let light: Vec<f32> = (0..STEPS)
            .map(|i| {
                let e = i as f64 / n;
                (match transfer {
                    TRC_PQ => olo_color::pq_decode(e) * 10_000.0 / white,
                    TRC_HLG => olo_color::hlg_decode(e),
                    // SDR video's curve, as the plain path shows it.
                    _ => olo_color::Transfer::Srgb.decode(e),
                }) as f32
            })
            .collect();
        let from = match primaries {
            PRI_BT2020 => olo_color::Primaries::BT2020,
            PRI_P3_D65 => olo_color::Primaries::DISPLAY_P3,
            _ => olo_color::Primaries::SRGB,
        };
        let m = from.to(if p3 { &olo_color::Primaries::DISPLAY_P3 } else { &olo_color::Primaries::SRGB });
        let matrix = (!m.is_identity(1e-6)).then(|| m.0.map(|row| row.map(|v| v as f32)));
        let xyz = from.to_xyz().0;
        let weights = [xyz[1][0] as f32, xyz[1][1] as f32, xyz[1][2] as f32];
        // HLG on a 1,000 cd/m² screen: Fd = Lw · Ys^(γ − 1) · E, γ 1.2.
        let hlg_gain = (transfer == TRC_HLG).then(|| (0..STEPS).map(|i| ((1000.0 / white) * (i as f64 / n).powf(0.2)) as f32).collect());
        let peak = match transfer {
            TRC_PQ | TRC_HLG => (if peak > 0.0 { peak as f64 } else { 1000.0 } / white).max(1.0) as f32,
            _ => 1.0,
        };
        let knee = 0.8f32.min(peak);
        // The curve's slope at the knee is 1 (no kink where it starts).
        let bend = if peak > knee { (peak - knee) / (1.0 - knee) } else { 1.0 };
        let span = (peak - knee).max(1e-6);
        let roll = (0..STEPS)
            .map(|i| {
                let t = i as f32 / (STEPS - 1) as f32;
                let top = knee + t * span;
                (knee + (1.0 - knee) * (bend * t) / (1.0 + (bend - 1.0) * t)) / top
            })
            .collect();
        let encode = (0..STEPS).map(|i| (olo_color::Transfer::Srgb.encode(i as f64 / n) * 255.0).round() as u8).collect();
        Mapping { light, hlg_gain, matrix, weights, knee, per_step: (STEPS - 1) as f32 / span, roll, encode }
    }

    /// One sample: its R′G′B′ signal as table steps (0..STEPS) to sRGB.
    /// (Written out, no closures over the channels: `[f32; 3]::map`'s
    /// weren't inlined, and calling them was half the time.)
    #[inline(always)]
    pub fn map(&self, r: usize, g: usize, b: usize) -> [u8; 3] {
        let (mut r, mut g, mut b) = (self.light[r], self.light[g], self.light[b]);
        if let Some(gain) = &self.hlg_gain {
            let ys = (self.weights[0] * r + self.weights[1] * g + self.weights[2] * b).clamp(0.0, 1.0);
            let k = gain[(ys * (STEPS - 1) as f32) as usize];
            (r, g, b) = (r * k, g * k, b * k);
        }
        if let Some(m) = &self.matrix {
            (r, g, b) = (m[0][0] * r + m[0][1] * g + m[0][2] * b, m[1][0] * r + m[1][1] * g + m[1][2] * b, m[2][0] * r + m[2][1] * g + m[2][2] * b);
        }
        // Highlights rolled off by the brightest channel (hue kept).
        let top = r.max(g).max(b);
        if top > self.knee {
            let i = ((top - self.knee) * self.per_step) as usize;
            // (Past the peak: scaled to 1.)
            let s = if i < STEPS { self.roll[i] } else { 1.0 / top };
            (r, g, b) = (r * s, g * s, b * s);
        }
        // Outside sRGB: clipped.
        let n = (STEPS - 1) as f32;
        let at = |v: f32| (v.clamp(0.0, 1.0) * n + 0.5) as usize;
        [self.encode[at(r)], self.encode[at(g)], self.encode[at(b)]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PQ signal for `nits`, as table steps.
    fn pq(nits: f64) -> usize {
        (olo_color::pq_encode(nits / 10_000.0) * (STEPS - 1) as f64).round() as usize
    }

    #[test]
    fn pq_paper_white_is_white_and_highlights_roll_off() {
        let m = Mapping::new(PRI_BT2020, TRC_PQ, 1000.0, false);
        let grey = |nits: f64| m.map(pq(nits), pq(nits), pq(nits))[0];
        // BT.2408's reference white shows nearly white (the roll-off
        // starts at four fifths, room left for highlights).
        assert!((240..=250).contains(&grey(203.0)), "{}", grey(203.0));
        // Darker greys as sRGB shows that light: 100 cd/m² is about half.
        let half = (olo_color::Transfer::Srgb.encode(100.0 / 203.0) * 255.0) as i32;
        assert!((i32::from(grey(100.0)) - half).abs() <= 2, "{} against {half}", grey(100.0));
        // Above white, steadily brighter up to the peak, never past 255.
        let steps: Vec<u8> = [120.0, 150.0, 180.0, 203.0].iter().map(|&n| grey(n)).collect();
        assert!(steps.windows(2).all(|w| w[0] < w[1]), "{steps:?}");
        assert_eq!(grey(1000.0), 255);
        assert_eq!(grey(4000.0), 255);
        // A bright red stays red, not pink or orange.
        let red = m.map(pq(600.0), pq(20.0), pq(20.0));
        assert!(red[0] > 200 && red[1] < 90 && red[2] < 90, "{red:?}");
    }

    #[test]
    fn hlg_reference_white_is_white() {
        // HLG's reference white: 75% signal (BT.2408), 203 cd/m² on a
        // 1,000 cd/m² screen.
        let m = Mapping::new(PRI_BT2020, TRC_HLG, 0.0, false);
        let s = (0.75 * (STEPS - 1) as f64) as usize;
        let w = m.map(s, s, s);
        assert!(w.iter().all(|&v| v >= 240), "{w:?}");
        let s = (0.40 * (STEPS - 1) as f64) as usize;
        let mid = m.map(s, s, s);
        assert!(mid.iter().all(|&v| (80..200).contains(&v)), "{mid:?}");
    }

    #[test]
    fn bt2020_green_fits_srgb_and_greys_stay_grey() {
        let m = Mapping::new(PRI_BT2020, 1, 0.0, false);
        let top = STEPS - 1;
        let g = m.map(0, top, 0);
        assert!(g[1] > 200 && g[0] < 120 && g[2] < 120, "{g:?}");
        let half = top / 2;
        let grey = m.map(half, half, half);
        assert!(grey[0].abs_diff(grey[1]) <= 1 && grey[1].abs_diff(grey[2]) <= 1, "{grey:?}");
        assert!(!needed(PRI_BT709, 1) && needed(PRI_BT2020, 1) && needed(PRI_BT709, TRC_PQ));
    }

    /// To Display P3, BT.2020's green keeps more of itself than in sRGB
    /// (P3's green is nearer it), and P3 pictures go through unchanged.
    #[test]
    fn to_display_p3() {
        let top = STEPS - 1;
        let (srgb, p3) = (Mapping::new(PRI_BT2020, 1, 0.0, false), Mapping::new(PRI_BT2020, 1, 0.0, true));
        let (a, b) = (srgb.map(top / 4, top, top / 4), p3.map(top / 4, top, top / 4));
        assert!(b[0] > a[0] || b[2] > a[2], "sRGB {a:?}, P3 {b:?}: in P3 it needs less clipping");
        let same = Mapping::new(PRI_P3_D65, 1, 0.0, true);
        let v = same.map(1000, 3000, 500);
        let enc = |i: usize| (olo_color::Transfer::Srgb.encode(olo_color::Transfer::Srgb.decode(i as f64 / top as f64)) * 255.0).round() as u8;
        assert!(v[0].abs_diff(enc(1000)) <= 1 && v[1].abs_diff(enc(3000)) <= 1 && v[2].abs_diff(enc(500)) <= 1, "{v:?}");
        assert!(wide(PRI_BT2020) && wide(PRI_P3_D65) && !wide(PRI_BT709));
    }
}
