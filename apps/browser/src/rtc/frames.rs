//! What flows through a call: pictures as I420 (the encoder's and the
//! decoder's own layout) and sound as 16-bit samples, with the
//! conversions from what cameras give and to what Servo draws.

use std::sync::Arc;

/// A picture in I420: the Y plane, then U and V at half size each way.
/// Width and height are even.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Sound, interleaved.
pub struct Sound {
    pub rate: u32,
    pub channels: u8,
    pub samples: Vec<i16>,
}

#[derive(Clone)]
pub enum Frame {
    Picture(Arc<Picture>),
    Sound(Arc<Sound>),
}

impl Picture {
    pub fn black(width: u32, height: u32) -> Picture {
        let (w, h) = (width as usize & !1, height as usize & !1);
        let mut data = vec![16u8; w * h];
        data.resize(w * h * 3 / 2, 128);
        Picture { width: w as u32, height: h as u32, data }
    }

    pub fn planes(&self) -> (&[u8], &[u8], &[u8]) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (y, uv) = self.data.split_at(w * h);
        let (u, v) = uv.split_at(w * h / 4);
        (y, u, v)
    }

    /// From NV12 (Y, then interleaved UV) with rows `stride` bytes apart.
    pub fn from_nv12(src: &[u8], width: u32, height: u32, stride: u32) -> Option<Picture> {
        let (w, h, stride) = (width as usize & !1, height as usize & !1, (stride as usize).max(width as usize));
        let uv_at = stride * height as usize;
        if src.len() < uv_at + stride * (h / 2) {
            return None;
        }
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        for row in 0..h {
            data.extend_from_slice(&src[row * stride..row * stride + w]);
        }
        let mut v = Vec::with_capacity(w * h / 4);
        for row in 0..h / 2 {
            let line = &src[uv_at + row * stride..uv_at + row * stride + w];
            for pair in line.as_chunks::<2>().0 {
                data.push(pair[0]);
                v.push(pair[1]);
            }
        }
        data.extend_from_slice(&v);
        Some(Picture { width: w as u32, height: h as u32, data })
    }

    /// From YUYV (4:2:2, Y0 U Y1 V).
    pub fn from_yuyv(src: &[u8], width: u32, height: u32, stride: u32) -> Option<Picture> {
        let (w, h, stride) = (width as usize & !1, height as usize & !1, (stride as usize).max(width as usize * 2));
        if src.len() < stride * h {
            return None;
        }
        let mut y = Vec::with_capacity(w * h);
        let mut u = Vec::with_capacity(w * h / 4);
        let mut v = Vec::with_capacity(w * h / 4);
        for row in 0..h {
            let line = &src[row * stride..row * stride + w * 2];
            for q in line.as_chunks::<4>().0 {
                y.push(q[0]);
                y.push(q[2]);
                if row % 2 == 0 {
                    u.push(q[1]);
                    v.push(q[3]);
                }
            }
        }
        y.extend_from_slice(&u);
        y.extend_from_slice(&v);
        Some(Picture { width: w as u32, height: h as u32, data: y })
    }

    /// From interleaved full-size YCbCr (a JPEG decoded without colour
    /// conversion), averaging chroma over each 2×2 block.
    pub fn from_ycbcr(src: &[u8], width: u32, height: u32) -> Option<Picture> {
        let (sw, w, h) = (width as usize, width as usize & !1, height as usize & !1);
        if src.len() < sw * height as usize * 3 {
            return None;
        }
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        for row in 0..h {
            data.extend(src[row * sw * 3..].as_chunks::<3>().0[..w].iter().map(|p| p[0]));
        }
        for c in 1..3 {
            for row in (0..h).step_by(2) {
                let (a, b) = (&src[row * sw * 3..], &src[(row + 1) * sw * 3..]);
                for x in (0..w).step_by(2) {
                    let s = a[x * 3 + c] as u32 + a[x * 3 + 3 + c] as u32 + b[x * 3 + c] as u32 + b[x * 3 + 3 + c] as u32;
                    data.push(((s + 2) / 4) as u8);
                }
            }
        }
        Some(Picture { width: w as u32, height: h as u32, data })
    }

    /// As BGRA for Servo (BT.601, limited range, as cameras and H.264
    /// streams mostly are).
    pub fn to_bgra(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (yp, up, vp) = self.planes();
        out.clear();
        out.resize(w * h * 4, 255);
        for row in 0..h {
            let (yr, cr) = (&yp[row * w..row * w + w], row / 2 * (w / 2));
            let line = &mut out[row * w * 4..(row + 1) * w * 4];
            for (x, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let y = (yr[x] as i32 - 16) * 298;
                let (u, v) = (up[cr + x / 2] as i32 - 128, vp[cr + x / 2] as i32 - 128);
                let clamp = |c: i32| ((c + 128) >> 8).clamp(0, 255) as u8;
                px[0] = clamp(y + 516 * u);
                px[1] = clamp(y - 100 * u - 208 * v);
                px[2] = clamp(y + 409 * v);
            }
        }
    }

    /// Scaled (nearest, for pictures far bigger than they're sent) to fit
    /// within `max_w`×`max_h`, keeping the shape.
    pub fn fit(&self, max_w: u32, max_h: u32) -> Option<Picture> {
        if self.width <= max_w && self.height <= max_h {
            return None;
        }
        let k = (max_w as f64 / self.width as f64).min(max_h as f64 / self.height as f64);
        let (w, h) = (((self.width as f64 * k) as usize).max(2) & !1, ((self.height as f64 * k) as usize).max(2) & !1);
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (yp, up, vp) = self.planes();
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        for row in 0..h {
            let sr = row * sh / h;
            data.extend((0..w).map(|x| yp[sr * sw + x * sw / w]));
        }
        for plane in [up, vp] {
            for row in 0..h / 2 {
                let sr = row * sh / h;
                data.extend((0..w / 2).map(|x| plane[sr * (sw / 2) + x * sw / w]));
            }
        }
        Some(Picture { width: w as u32, height: h as u32, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_pixels() {
        // A 2×2 NV12 picture: four greys, one chroma pair.
        let nv12 = [16, 235, 126, 126, 90, 240];
        let p = Picture::from_nv12(&nv12, 2, 2, 2).unwrap();
        assert_eq!(p.data, vec![16, 235, 126, 126, 90, 240]);
        let mut bgra = Vec::new();
        Picture::black(2, 2).to_bgra(&mut bgra);
        assert_eq!(&bgra[..4], &[0, 0, 0, 255]);
        let yuyv = [100, 90, 110, 240, 120, 91, 130, 241];
        let p = Picture::from_yuyv(&yuyv, 2, 2, 4).unwrap();
        assert_eq!(p.data, vec![100, 110, 120, 130, 90, 240]);
        let ycc = [50, 100, 200, 60, 102, 202, 70, 104, 204, 80, 106, 206];
        let p = Picture::from_ycbcr(&ycc, 2, 2).unwrap();
        assert_eq!(p.data, vec![50, 60, 70, 80, 103, 203]);
        let big = Picture::black(640, 480);
        let small = big.fit(320, 320).unwrap();
        assert_eq!((small.width, small.height), (320, 240));
        assert_eq!(small.data.len(), 320 * 240 * 3 / 2);
    }
}
