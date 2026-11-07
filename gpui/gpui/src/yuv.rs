//! Olo: a video's picture as its decoder made it (Y'CbCr, 4:2:0), drawn
//! by the GPU (`Window::paint_yuv`): its planes uploaded as two textures
//! (luma, and chroma interleaved), turned into RGB by the surface shader
//! with the picture's own matrix and range. A 4K picture is 12 MB of
//! planes rather than 33 MB of RGBA, and nothing converts it on the CPU.

use std::sync::atomic::{AtomicU64, Ordering};

/// How a picture's Y'CbCr became RGB (its matrix coefficients).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[allow(missing_docs)]
pub enum YuvMatrix {
    /// SD video's (and JPEG's).
    Bt601,
    /// HD video's.
    #[default]
    Bt709,
    /// UHD video's (an SDR picture of it; HDR is drawn otherwise).
    Bt2020,
}

impl YuvMatrix {
    /// Kr and Kb.
    fn k(self) -> (f32, f32) {
        match self {
            YuvMatrix::Bt601 => (0.299, 0.114),
            YuvMatrix::Bt709 => (0.2126, 0.0722),
            YuvMatrix::Bt2020 => (0.2627, 0.0593),
        }
    }
}

/// A picture's planes: luma (`width` × `height` samples) and chroma (Cb
/// and Cr interleaved, half as wide and high, rounded up), rows packed.
/// 8-bit samples, or 16-bit (`deep`: 10- or 12-bit video, each sample in
/// the high bits, as P010 has it), little-endian.
#[derive(Debug)]
pub struct YuvFrame {
    /// The stream it's from ([`YuvFrame::new_stream`]): its textures are
    /// kept and filled again for each new picture.
    pub stream: u64,
    /// Its size in pixels (the luma plane's).
    pub width: u32,
    /// (As `width`.)
    pub height: u32,
    /// 16-bit samples (10- or 12-bit video), else 8-bit.
    pub deep: bool,
    /// The luma plane, rows packed.
    pub y: Vec<u8>,
    /// The chroma plane, Cb and Cr interleaved, rows packed.
    pub uv: Vec<u8>,
    /// How its Y'CbCr is made RGB.
    pub matrix: YuvMatrix,
    /// Full range (0–255), else video's limited range (16–235, 16–240).
    pub full_range: bool,
}

static STREAMS: AtomicU64 = AtomicU64::new(1);

impl YuvFrame {
    /// An id for a new stream of pictures.
    pub fn new_stream() -> u64 {
        STREAMS.fetch_add(1, Ordering::Relaxed)
    }

    /// The chroma planes' size (samples).
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// RGB from (Y', Cb, Cr, 1) as the textures give them (normalized),
    /// a 4 × 4 matrix in columns, as the shader takes it.
    pub fn rgb_matrix(&self) -> [[f32; 4]; 4] {
        rgb_matrix(self.matrix, self.full_range, self.deep)
    }
}

/// [`YuvFrame::rgb_matrix`] for a matrix, range and sample size.
pub fn rgb_matrix(matrix: YuvMatrix, full_range: bool, deep: bool) -> [[f32; 4]; 4] {
    // One code value of an 8-bit sample, in what the texture gives (a
    // 16-bit one holds it shifted up eight bits, or two for 10-bit,
    // which is the same in the high bits).
    let unit = if deep { 256.0 / 65535.0 } else { 1.0 / 255.0 };
    let (y0, yr, cm, cr) = if full_range {
        (0.0, 255.0 * unit, 128.0 * unit, 255.0 * unit)
    } else {
        (16.0 * unit, 219.0 * unit, 128.0 * unit, 224.0 * unit)
    };
    let (kr, kb) = matrix.k();
    let kg = 1.0 - kr - kb;
    // R = Y + a·Pr; G = Y − b·Pb − c·Pr; B = Y + d·Pb (Y, Pb, Pr normalized).
    let (a, d) = (2.0 * (1.0 - kr), 2.0 * (1.0 - kb));
    let (b, c) = (2.0 * kb * (1.0 - kb) / kg, 2.0 * kr * (1.0 - kr) / kg);
    let ys = 1.0 / yr;
    let cs = 1.0 / cr;
    // Columns: Y's, Cb's, Cr's coefficients, then the constants.
    let y_col = [ys, ys, ys, 0.0];
    let cb_col = [0.0, -b * cs, d * cs, 0.0];
    let cr_col = [a * cs, -c * cs, 0.0, 0.0];
    let k = [
        -y0 * ys - a * cm * cs,
        -y0 * ys + (b + c) * cm * cs,
        -y0 * ys - d * cm * cs,
        1.0,
    ];
    [y_col, cb_col, cr_col, k]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [[f32; 4]; 4], v: [f32; 3]) -> [f32; 3] {
        let v = [v[0], v[1], v[2], 1.0];
        let mut out = [0.0; 3];
        for (r, o) in out.iter_mut().enumerate() {
            *o = (0..4).map(|c| m[c][r] * v[c]).sum();
        }
        out
    }

    /// Black, white and the primaries come out where they should.
    #[test]
    fn video_s_numbers_become_rgb() {
        let m = rgb_matrix(YuvMatrix::Bt709, false, false);
        let n = |v: f32| v / 255.0;
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
        assert!(close(
            apply(m, [n(16.0), n(128.0), n(128.0)]),
            [0.0, 0.0, 0.0]
        ));
        assert!(close(
            apply(m, [n(235.0), n(128.0), n(128.0)]),
            [1.0, 1.0, 1.0]
        ));
        // BT.709's red, limited range: Y 63, Cb 102, Cr 240.
        let red = apply(m, [n(63.0), n(102.0), n(240.0)]);
        assert!(close(red, [1.0, 0.0, 0.0]), "{red:?}");
        // Full range: 0 and 255 the ends.
        let f = rgb_matrix(YuvMatrix::Bt601, true, false);
        assert!(close(apply(f, [1.0, n(128.0), n(128.0)]), [1.0, 1.0, 1.0]));
        // 10-bit in 16: 64 (black) and 940 (white), shifted up six.
        let d = rgb_matrix(YuvMatrix::Bt709, false, true);
        let s = |v: f32| v * 64.0 / 65535.0;
        assert!(close(
            apply(d, [s(64.0), s(512.0), s(512.0)]),
            [0.0, 0.0, 0.0]
        ));
        assert!(close(
            apply(d, [s(940.0), s(512.0), s(512.0)]),
            [1.0, 1.0, 1.0]
        ));
    }
}
