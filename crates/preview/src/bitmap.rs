//! Decoded pictures as the previews keep them, in no toolkit's type: the
//! views make their textures from them.

/// A picture `size` pixels across and down, RGBA with the colours
/// premultiplied by alpha, row by row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bitmap {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// `v × a / 255`, rounded.
fn times(v: u8, a: u8) -> u8 {
    let p = u16::from(v) * u16::from(a) + 128;
    ((p + (p >> 8)) >> 8) as u8
}

impl Bitmap {
    /// From premultiplied RGBA.
    pub fn from_rgba_premultiplied(size: [usize; 2], rgba: Vec<u8>) -> Bitmap {
        debug_assert_eq!(rgba.len(), size[0] * size[1] * 4);
        Bitmap { size, rgba }
    }

    /// From straight (not premultiplied) RGBA.
    pub fn from_rgba_unmultiplied(size: [usize; 2], rgba: &[u8]) -> Bitmap {
        let mut out = rgba.to_vec();
        premultiply(&mut out);
        Bitmap::from_rgba_premultiplied(size, out)
    }

    /// From straight BGRA (a decoder's own order).
    pub fn from_bgra_unmultiplied(size: [usize; 2], bgra: &[u8]) -> Bitmap {
        let mut out = bgra.to_vec();
        for p in out.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        premultiply(&mut out);
        Bitmap::from_rgba_premultiplied(size, out)
    }

    pub fn width(&self) -> usize {
        self.size[0]
    }

    pub fn height(&self) -> usize {
        self.size[1]
    }

    /// Its bytes, as straight-alpha BGRA (what GPUI's images take).
    pub fn to_bgra_unmultiplied(&self) -> Vec<u8> {
        let mut out = self.rgba.clone();
        for p in out.as_chunks_mut::<4>().0 {
            let a = p[3];
            if a != 0 && a != 255 {
                let k = 255.0 / f32::from(a);
                for c in &mut p[..3] {
                    *c = (f32::from(*c) * k).round().min(255.0) as u8;
                }
            }
            p.swap(0, 2);
        }
        out
    }
}

/// Straight RGBA to premultiplied, in place.
pub fn premultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        match p[3] {
            255 => {}
            0 => *p = [0; 4],
            a => {
                p[0] = times(p[0], a);
                p[1] = times(p[1], a);
                p[2] = times(p[2], a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_rounding() {
        // v·a/255, rounded to the nearest.
        for v in 0..=255u8 {
            for a in 1..=254u8 {
                let want = (f32::from(v) * f32::from(a) / 255.0).round() as u8;
                assert_eq!(times(v, a), want, "{v} × {a}");
            }
        }
    }

    #[test]
    fn round_trips_through_bgra() {
        let b = Bitmap::from_rgba_unmultiplied([2, 1], &[10, 20, 30, 255, 255, 100, 50, 128]);
        assert_eq!(b.to_bgra_unmultiplied(), vec![30, 20, 10, 255, 50, 100, 255, 128]);
    }
}
