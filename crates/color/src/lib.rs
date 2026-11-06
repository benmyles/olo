//! Colour science for Olo: colour spaces by their primaries and white
//! point, conversions between them (linear light, the white adapted by
//! Bradford, as ICC's relative colorimetric intent does), transfer
//! functions (sRGB, gamma 2.2, PQ), and how much of one gamut another
//! covers. app_server converts what it shows into each display's colours
//! with it; Preferences says what a display can show.
//!
//! Matrices are row-major and act on column vectors: `m.apply([r, g, b])`.

use serde::{Deserialize, Serialize};

/// A colour's place in the CIE 1931 xy chromaticity diagram.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Chromaticity {
    pub x: f64,
    pub y: f64,
}

const fn xy(x: f64, y: f64) -> Chromaticity {
    Chromaticity { x, y }
}

/// D65, the white of sRGB, Display P3 and BT.2020.
pub const D65: Chromaticity = xy(0.3127, 0.3290);

/// An RGB colour space's primaries and white.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Primaries {
    pub red: Chromaticity,
    pub green: Chromaticity,
    pub blue: Chromaticity,
    pub white: Chromaticity,
}

impl Primaries {
    /// sRGB and BT.709: what everything is drawn in unless it says.
    pub const SRGB: Primaries = Primaries { red: xy(0.640, 0.330), green: xy(0.300, 0.600), blue: xy(0.150, 0.060), white: D65 };
    /// Display P3 (DCI-P3's primaries, D65): Apple's displays, and most
    /// wide-gamut laptop panels.
    pub const DISPLAY_P3: Primaries = Primaries { red: xy(0.680, 0.320), green: xy(0.265, 0.690), blue: xy(0.150, 0.060), white: D65 };
    /// BT.2020: HDR video's container.
    pub const BT2020: Primaries = Primaries { red: xy(0.708, 0.292), green: xy(0.170, 0.797), blue: xy(0.131, 0.046), white: D65 };
    /// Adobe RGB (1998).
    pub const ADOBE_RGB: Primaries = Primaries { red: xy(0.640, 0.330), green: xy(0.210, 0.710), blue: xy(0.150, 0.060), white: D65 };

    /// The standard gamuts, smallest first, by name.
    pub const NAMED: [(&'static str, Primaries); 4] = [("sRGB", Primaries::SRGB), ("Adobe RGB", Primaries::ADOBE_RGB), ("Display P3", Primaries::DISPLAY_P3), ("BT.2020", Primaries::BT2020)];

    /// Whether these are plausible primaries (a display's EDID may hold
    /// zeros or nonsense): each inside the visible range, the triangle
    /// not flat, the white inside it.
    pub fn plausible(&self) -> bool {
        let all = [self.red, self.green, self.blue, self.white];
        // (A monochromatic red has x + y = 1: P3's does.)
        let inside = all.iter().all(|c| c.x > 0.0 && c.y > 0.0 && c.x + c.y <= 1.0 + 1e-9);
        inside && area(&self.triangle_uv()) > 1e-3 && contains(&self.triangle_uv(), uv(self.white))
    }

    /// Linear RGB to CIE XYZ (Y of the white 1).
    pub fn to_xyz(&self) -> Mat3 {
        let col = |c: Chromaticity| [c.x / c.y, 1.0, (1.0 - c.x - c.y) / c.y];
        let (r, g, b) = (col(self.red), col(self.green), col(self.blue));
        let p = Mat3([[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]]);
        let w = col(self.white);
        let s = p.inverse().apply(w);
        Mat3([[r[0] * s[0], g[0] * s[1], b[0] * s[2]], [r[1] * s[0], g[1] * s[1], b[1] * s[2]], [r[2] * s[0], g[2] * s[1], b[2] * s[2]]])
    }

    /// Linear RGB in these to linear RGB in `to`, the white adapted
    /// (Bradford): white stays white, (1, 1, 1) to (1, 1, 1).
    pub fn to(&self, to: &Primaries) -> Mat3 {
        to.to_xyz().inverse().mul(&bradford(self.white, to.white)).mul(&self.to_xyz())
    }

    fn triangle_uv(&self) -> [(f64, f64); 3] {
        [uv(self.red), uv(self.green), uv(self.blue)]
    }

    /// How much of `of`'s gamut these cover (0–1), by area in CIE 1976
    /// u′v′ (more even to the eye than xy).
    pub fn coverage(&self, of: &Primaries) -> f64 {
        let reference = of.triangle_uv();
        let total = area(&reference);
        if total <= 0.0 {
            return 0.0;
        }
        area(&clip(&self.triangle_uv(), &reference)) / total
    }

    /// The widest standard gamut these cover (at least 95% of it), with
    /// how much; None when not even most of sRGB.
    pub fn widest_covered(&self) -> Option<(&'static str, f64)> {
        Primaries::NAMED.iter().rev().map(|(n, p)| (*n, self.coverage(p))).find(|(_, c)| *c >= 0.95)
    }

    /// Whether these are notably wider than sRGB (worth converting into).
    pub fn wide(&self) -> bool {
        let srgb = self.coverage(&Primaries::SRGB);
        let own = area(&self.triangle_uv());
        // Covering sRGB, and at least 10% more besides.
        srgb >= 0.95 && own >= area(&Primaries::SRGB.triangle_uv()) * 1.10
    }
}

/// A 3 × 3 matrix, row-major.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
    }

    pub fn mul(&self, o: &Mat3) -> Mat3 {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Mat3(r)
    }

    pub fn inverse(&self) -> Mat3 {
        let m = &self.0;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let d = 1.0 / det;
        Mat3([
            [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d],
            [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d],
            [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d],
        ])
    }

    /// Column-major, as GL's `mat3` uniforms take it.
    pub fn to_gl(&self) -> [f32; 9] {
        let m = &self.0;
        [m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2]].map(|v| v as f32)
    }

    pub fn is_identity(&self, tolerance: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (self.0[i][j] - Mat3::IDENTITY.0[i][j]).abs() <= tolerance))
    }
}

/// Bradford's chromatic adaptation, from one white to another, in XYZ.
pub fn bradford(from: Chromaticity, to: Chromaticity) -> Mat3 {
    const B: Mat3 = Mat3([[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]]);
    let xyz = |c: Chromaticity| [c.x / c.y, 1.0, (1.0 - c.x - c.y) / c.y];
    let (s, d) = (B.apply(xyz(from)), B.apply(xyz(to)));
    let scale = Mat3([[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]]);
    B.inverse().mul(&scale).mul(&B)
}

/// How encoded values become light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transfer {
    /// sRGB's piecewise curve (IEC 61966-2-1).
    Srgb,
    /// A pure power of 2.2: what sRGB displays actually do, and what Olo
    /// converts between displays' colours with.
    Gamma22,
    /// SMPTE ST 2084 (PQ): HDR10's, absolute, 0–10,000 cd/m².
    Pq,
    /// Light itself.
    Linear,
}

impl Transfer {
    /// Encoded (0–1) to light (relative; PQ: of 10,000 cd/m²).
    pub fn decode(self, v: f64) -> f64 {
        match self {
            Transfer::Srgb => {
                let a = v.abs();
                let l = if a <= 0.04045 { a / 12.92 } else { ((a + 0.055) / 1.055).powf(2.4) };
                l.copysign(v)
            }
            Transfer::Gamma22 => v.abs().powf(2.2).copysign(v),
            Transfer::Pq => pq_decode(v.max(0.0)),
            Transfer::Linear => v,
        }
    }

    /// Light to encoded: `decode`'s inverse.
    pub fn encode(self, l: f64) -> f64 {
        match self {
            Transfer::Srgb => {
                let a = l.abs();
                let v = if a <= 0.0031308 { a * 12.92 } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
                v.copysign(l)
            }
            Transfer::Gamma22 => l.abs().powf(1.0 / 2.2).copysign(l),
            Transfer::Pq => pq_encode(l.max(0.0)),
            Transfer::Linear => l,
        }
    }
}

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// PQ's signal (0–1) to light, as a fraction of 10,000 cd/m².
pub fn pq_decode(e: f64) -> f64 {
    let p = e.powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

/// Light (a fraction of 10,000 cd/m²) to PQ's signal.
pub fn pq_encode(y: f64) -> f64 {
    let p = y.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * p) / (1.0 + PQ_C3 * p)).powf(PQ_M2)
}

const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 1.0 - 4.0 * HLG_A;
const HLG_C: f64 = 0.559_910_73;

/// HLG's signal (0–1) to scene light (0–1): its OETF's inverse
/// (ITU-R BT.2100).
pub fn hlg_decode(e: f64) -> f64 {
    let e = e.clamp(0.0, 1.0);
    if e <= 0.5 { e * e / 3.0 } else { (((e - HLG_C) / HLG_A).exp() + HLG_B) / 12.0 }
}

/// Scene light (0–1) to HLG's signal: its OETF.
pub fn hlg_encode(l: f64) -> f64 {
    let l = l.clamp(0.0, 1.0);
    if l <= 1.0 / 12.0 { (3.0 * l).sqrt() } else { HLG_A * (12.0 * l - HLG_B).ln() + HLG_C }
}

/// The reference white of SDR in HDR (ITU-R BT.2408), cd/m².
pub const SDR_WHITE_NITS: f64 = 203.0;

/// CIE 1976 u′v′ of a chromaticity.
fn uv(c: Chromaticity) -> (f64, f64) {
    let d = -2.0 * c.x + 12.0 * c.y + 3.0;
    (4.0 * c.x / d, 9.0 * c.y / d)
}

/// A polygon's area (absolute), by the shoelace formula.
fn area(poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| poly[i].0 * poly[(i + 1) % n].1 - poly[(i + 1) % n].0 * poly[i].1).sum::<f64>().abs() / 2.0
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// The triangle wound anticlockwise.
fn ccw(t: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut v = t.to_vec();
    if v.len() == 3 && cross(v[0], v[1], v[2]) < 0.0 {
        v.swap(1, 2);
    }
    v
}

fn contains(t: &[(f64, f64); 3], p: (f64, f64)) -> bool {
    let t = ccw(t);
    (0..3).all(|i| cross(t[i], t[(i + 1) % 3], p) >= 0.0)
}

/// `subject` clipped to the convex `clip` (Sutherland–Hodgman).
fn clip(subject: &[(f64, f64)], clip: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let clip = ccw(clip);
    let mut out = ccw(subject);
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let input = std::mem::take(&mut out);
        if input.is_empty() {
            break;
        }
        let inside = |p: (f64, f64)| cross(a, b, p) >= 0.0;
        let meet = |p: (f64, f64), q: (f64, f64)| {
            let (d1, d2) = (cross(a, b, p), cross(a, b, q));
            let t = d1 / (d1 - d2);
            (p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1))
        };
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(meet(p, q)),
                (false, true) => {
                    out.push(meet(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn srgb_to_xyz_is_the_standards() {
        // IEC 61966-2-1's matrix (to four places).
        let m = Primaries::SRGB.to_xyz();
        let want = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        for i in 0..3 {
            for j in 0..3 {
                assert!(close(m.0[i][j], want[i][j], 5e-4), "{m:?}");
            }
        }
    }

    #[test]
    fn srgb_into_display_p3() {
        // The well-known matrix (as Apple's and CSS Color 4's).
        let m = Primaries::SRGB.to(&Primaries::DISPLAY_P3);
        let want = [[0.8225, 0.1774, 0.0000], [0.0332, 0.9669, 0.0000], [0.0171, 0.0724, 0.9108]];
        for i in 0..3 {
            for j in 0..3 {
                assert!(close(m.0[i][j], want[i][j], 1e-3), "{m:?}");
            }
        }
        // White stays white, and back again is where it started.
        let w = m.apply([1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| close(*v, 1.0, 1e-9)), "{w:?}");
        let back = Primaries::DISPLAY_P3.to(&Primaries::SRGB).mul(&m);
        assert!(back.is_identity(1e-9));
        assert!(Primaries::SRGB.to(&Primaries::SRGB).is_identity(1e-9));
    }

    #[test]
    fn whites_adapt() {
        // D65 to D50 by Bradford (Lindbloom's).
        let d50 = xy(0.3457, 0.3585);
        let m = bradford(D65, d50);
        assert!(close(m.0[0][0], 1.0479, 1e-3) && close(m.0[1][1], 0.9904, 1e-3) && close(m.0[2][2], 0.7519, 1e-3), "{m:?}");
    }

    #[test]
    fn transfers_round_trip() {
        for t in [Transfer::Srgb, Transfer::Gamma22, Transfer::Pq, Transfer::Linear] {
            for i in 0..=100 {
                let v = i as f64 / 100.0;
                // (PQ's 0 encodes as 7e-7: c1 to the power m2.)
                assert!(close(t.encode(t.decode(v)), v, 1e-6), "{t:?} {v}");
            }
        }
        // PQ's landmarks: 100 cd/m² is 0.508, 203 is 0.581, 1000 is 0.752.
        assert!(close(pq_encode(100.0 / 10_000.0), 0.5081, 1e-3));
        assert!(close(pq_encode(SDR_WHITE_NITS / 10_000.0), 0.5807, 1e-3));
        assert!(close(pq_encode(1000.0 / 10_000.0), 0.7518, 1e-3));
        assert!(close(Transfer::Srgb.decode(0.5), 0.2140, 1e-4));
    }

    #[test]
    fn coverage_of_the_standard_gamuts() {
        let p3 = Primaries::DISPLAY_P3;
        assert!(close(p3.coverage(&p3), 1.0, 1e-9));
        assert!(close(p3.coverage(&Primaries::SRGB), 1.0, 1e-9));
        // In u′v′ sRGB is about 80% of P3 (P3 "25% more colours"); P3
        // about 73% of BT.2020.
        assert!(close(Primaries::SRGB.coverage(&p3), 0.80, 0.02), "{}", Primaries::SRGB.coverage(&p3));
        assert!(close(p3.coverage(&Primaries::BT2020), 0.73, 0.02), "{}", p3.coverage(&Primaries::BT2020));
        assert!(p3.wide() && !Primaries::SRGB.wide());
        assert_eq!(p3.widest_covered().map(|(n, _)| n), Some("Display P3"));
        assert_eq!(Primaries::SRGB.widest_covered().map(|(n, _)| n), Some("sRGB"));
        assert!(p3.plausible());
        assert!(!Primaries { red: xy(0.0, 0.0), ..p3 }.plausible());
    }

    #[test]
    fn the_thinkpads_oled() {
        // Samsung ATNA60KA02's EDID (a ThinkPad P1 Gen 9): a little wider
        // than P3 in the greens.
        let panel = Primaries { red: xy(0.6807, 0.3184), green: xy(0.2197, 0.7246), blue: xy(0.1426, 0.0391), white: xy(0.3135, 0.3291) };
        assert!(panel.plausible() && panel.wide());
        assert!(panel.coverage(&Primaries::DISPLAY_P3) > 0.99, "{}", panel.coverage(&Primaries::DISPLAY_P3));
        assert_eq!(panel.widest_covered().map(|(n, _)| n), Some("Display P3"));
        // sRGB's red, as the panel must be driven to show it: less red,
        // some green, white still white.
        let m = Primaries::SRGB.to(&panel);
        let red = m.apply([1.0, 0.0, 0.0]);
        assert!(red[0] < 0.85 && red[1] > 0.03 && red.iter().all(|v| *v >= 0.0), "{red:?}");
        assert!(m.apply([1.0, 1.0, 1.0]).iter().all(|v| close(*v, 1.0, 1e-9)));
    }
}
