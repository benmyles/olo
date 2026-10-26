//! Reading pictures for thumbnails, Quick Look and the like: everything the
//! `image` crate reads, plus JPEG XL (what Fedora ships its backgrounds
//! as), AVIF (avif-parse and the rav1d AV1 decoder) and HEIC / HEIF (heifer,
//! iPhone photos), all in Rust, the right way up and within a memory
//! limit.

use std::{io::Read, path::Path};

use image::{DynamicImage, ImageDecoder, ImageReader};

/// JPEG XL by its signature: a bare codestream or the ISO container.
pub fn is_jxl(head: &[u8]) -> bool {
    head.starts_with(&[0xff, 0x0a]) || head.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
}

/// The ISO base media brand of an AVIF or HEIF file, by its `ftyp` box.
pub fn heif_brand(head: &[u8]) -> Option<Heif> {
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return None;
    }
    match &head[8..12] {
        b"avif" | b"avis" => Some(Heif::Avif),
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some(Heif::Hevc),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heif {
    Avif,
    Hevc,
}

/// Largest picture decoded (pixels), as for other formats.
const MAX_PIXELS: u64 = (1 << 30) / 4;

fn avif(data: &[u8]) -> Result<DynamicImage, String> {
    let parsed = avif_parse::read_avif(&mut &data[..]).map_err(|e| format!("{e:?}"))?;
    let color = av1::decode(&parsed.primary_item)?;
    let (w, h) = (color.width, color.height);
    let mut rgba = color.to_rgba();
    if let Some(alpha) = &parsed.alpha_item {
        let alpha = av1::decode(alpha)?;
        if (alpha.width, alpha.height) != (w, h) {
            return Err("alpha plane size differs".into());
        }
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = alpha.luma((i % w as usize) as u32, (i / w as usize) as u32);
            px[3] = a;
            if parsed.premultiplied_alpha && a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                }
            }
        }
    }
    image::RgbaImage::from_raw(w, h, rgba).map(DynamicImage::ImageRgba8).ok_or_else(|| "bad image size".into())
}

/// AV1 stills through rav1d's dav1d API, and YUV to RGB.
mod av1 {
    use std::{mem::MaybeUninit, ptr::NonNull};

    use rav1d::include::dav1d::{data::Dav1dData, dav1d::Dav1dSettings, headers::*, picture::Dav1dPicture};
    use rav1d::src::lib::*;

    const EAGAIN: i32 = 11;

    /// One decoded frame's planes, copied out of rav1d.
    pub struct Frame {
        pub width: u32,
        pub height: u32,
        bpc: u32,
        /// Chroma subsampling shifts (x, y); `None` for monochrome.
        chroma: Option<(u32, u32)>,
        planes: [Vec<u16>; 3],
        stride: [usize; 3],
        matrix: u32,
        full_range: bool,
    }

    pub fn decode(obus: &[u8]) -> Result<Frame, String> {
        if obus.is_empty() {
            return Err("no AV1 data".into());
        }
        unsafe {
            let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            let mut settings = settings.assume_init();
            // Callers decode on a worker already.
            settings.n_threads = 1;
            settings.max_frame_delay = 1;
            settings.frame_size_limit = super::MAX_PIXELS as u32;
            let mut ctx = None;
            if dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut settings))).0 < 0 {
                return Err("AV1 decoder failed to start".into());
            }
            let mut data = Dav1dData::default();
            let buf = dav1d_data_create(Some(NonNull::from(&mut data)), obus.len());
            if buf.is_null() {
                dav1d_close(Some(NonNull::from(&mut ctx)));
                return Err("out of memory".into());
            }
            std::ptr::copy_nonoverlapping(obus.as_ptr(), buf, obus.len());
            let mut pic = Dav1dPicture::default();
            let mut result = Err("no picture in the AV1 data".to_string());
            // Bounded: a still is one temporal unit.
            for _ in 0..64 {
                if data.sz > 0 {
                    let r = dav1d_send_data(ctx, Some(NonNull::from(&mut data))).0;
                    if r < 0 && r != -EAGAIN {
                        result = Err(format!("AV1 data rejected ({r})"));
                        break;
                    }
                }
                let r = dav1d_get_picture(ctx, Some(NonNull::from(&mut pic))).0;
                if r == 0 {
                    result = Frame::copy(&pic);
                    dav1d_picture_unref(Some(NonNull::from(&mut pic)));
                    break;
                }
                if r != -EAGAIN {
                    result = Err(format!("AV1 decode failed ({r})"));
                    break;
                }
            }
            if data.sz > 0 {
                dav1d_data_unref(Some(NonNull::from(&mut data)));
            }
            dav1d_close(Some(NonNull::from(&mut ctx)));
            result
        }
    }

    impl Frame {
        unsafe fn copy(pic: &Dav1dPicture) -> Result<Frame, String> {
            let (w, h, bpc) = (pic.p.w.max(0) as usize, pic.p.h.max(0) as usize, pic.p.bpc as u32);
            if w == 0 || h == 0 {
                return Err("empty picture".into());
            }
            let chroma = match pic.p.layout {
                DAV1D_PIXEL_LAYOUT_I400 => None,
                DAV1D_PIXEL_LAYOUT_I420 => Some((1, 1)),
                DAV1D_PIXEL_LAYOUT_I422 => Some((1, 0)),
                _ => Some((0, 0)),
            };
            let (matrix, full_range) = match pic.seq_hdr {
                Some(s) => unsafe { (s.as_ref().mtrx, s.as_ref().color_range != 0) },
                None => (DAV1D_MC_UNKNOWN, false),
            };
            let mut planes: [Vec<u16>; 3] = Default::default();
            let mut stride = [0; 3];
            let n = if chroma.is_some() { 3 } else { 1 };
            for (i, plane) in planes.iter_mut().enumerate().take(n) {
                let (sx, sy) = if i == 0 { (0, 0) } else { chroma.unwrap() };
                let (pw, ph) = ((w + sx as usize) >> sx, (h + sy as usize) >> sy);
                let Some(base) = pic.data[i] else { return Err("missing plane".into()) };
                let line = pic.stride[(i > 0) as usize];
                plane.reserve_exact(pw * ph);
                for y in 0..ph {
                    let row = unsafe { base.as_ptr().cast::<u8>().offset(line * y as isize) };
                    if bpc > 8 {
                        plane.extend_from_slice(unsafe { std::slice::from_raw_parts(row.cast::<u16>(), pw) });
                    } else {
                        plane.extend(unsafe { std::slice::from_raw_parts(row, pw) }.iter().map(|&v| v as u16));
                    }
                }
                stride[i] = pw;
            }
            Ok(Frame { width: w as u32, height: h as u32, bpc, chroma, planes, stride, matrix, full_range })
        }

        fn sample(&self, plane: usize, x: u32, y: u32) -> f32 {
            let (sx, sy) = if plane == 0 { (0, 0) } else { self.chroma.unwrap_or((0, 0)) };
            self.planes[plane][(y >> sy) as usize * self.stride[plane] + (x >> sx) as usize] as f32
        }

        /// Luma as an 8-bit value: an alpha plane.
        pub fn luma(&self, x: u32, y: u32) -> u8 {
            let max = ((1u32 << self.bpc) - 1) as f32;
            let v = self.sample(0, x, y);
            let v = if self.full_range { v / max } else { (v - (16 << (self.bpc - 8)) as f32) / (219 << (self.bpc - 8)) as f32 };
            (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        }

        pub fn to_rgba(&self) -> Vec<u8> {
            let shift = (self.bpc - 8) as i32;
            let max = ((1u32 << self.bpc) - 1) as f32;
            let half = (1u32 << (self.bpc - 1)) as f32;
            let (y0, ys, cs) = if self.full_range {
                (0.0, max, max)
            } else {
                ((16 << shift) as f32, (219 << shift) as f32, (224 << shift) as f32)
            };
            // Kr, Kb; unspecified is BT.601, as libavif and browsers assume.
            let (kr, kb) = match self.matrix {
                DAV1D_MC_BT709 => (0.2126, 0.0722),
                DAV1D_MC_FCC => (0.30, 0.11),
                DAV1D_MC_SMPTE240 => (0.212, 0.087),
                DAV1D_MC_BT2020_NCL | DAV1D_MC_BT2020_CL => (0.2627, 0.0593),
                _ => (0.299, 0.114),
            };
            let kg = 1.0 - kr - kb;
            let (w, h) = (self.width, self.height);
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            for y in 0..h {
                for x in 0..w {
                    let l = (self.sample(0, x, y) - y0) / ys;
                    let (r, g, b) = match self.chroma {
                        None => (l, l, l),
                        Some(_) => {
                            let u = (self.sample(1, x, y) - half) / cs;
                            let v = (self.sample(2, x, y) - half) / cs;
                            match self.matrix {
                                // GBR: Y is green, U blue, V red.
                                DAV1D_MC_IDENTITY => {
                                    let s = |p: usize| (self.sample(p, x, y) - y0) / ys;
                                    (s(2), s(0), s(1))
                                }
                                DAV1D_MC_SMPTE_YCGCO => {
                                    let t = l - u;
                                    (t + v, l + u, t - v)
                                }
                                _ => {
                                    let r = l + 2.0 * (1.0 - kr) * v;
                                    let b = l + 2.0 * (1.0 - kb) * u;
                                    (r, (l - kr * r - kb * b) / kg, b)
                                }
                            }
                        }
                    };
                    out.extend_from_slice(&[byte(r), byte(g), byte(b), 255]);
                }
            }
            out
        }
    }
}

fn heic(data: &[u8]) -> Result<DynamicImage, String> {
    let img = heifer::decode(data).map_err(|e| e.to_string())?;
    if img.width as u64 * img.height as u64 > MAX_PIXELS {
        return Err("too large".into());
    }
    image::RgbaImage::from_raw(img.width, img.height, img.to_rgba8()).map(DynamicImage::ImageRgba8).ok_or_else(|| "bad image size".into())
}

/// Decode `path`, returning the picture and its format's name ("PNG").
pub fn open(path: &Path) -> Result<(DynamicImage, String), String> {
    let mut head = [0u8; 12];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| e.to_string())?;
    if is_jxl(&head[..n]) {
        let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
        let decoder = jxl_oxide::integration::JxlDecoder::new(file).map_err(|e| e.to_string())?;
        let img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        return Ok((img, "JPEG XL".into()));
    }
    if let Some(kind) = heif_brand(&head[..n]) {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        return match kind {
            Heif::Avif => avif(&data).map(|i| (i, "AVIF".into())),
            // Some HEIF files hold AV1 under a generic brand.
            Heif::Hevc => heic(&data).map(|i| (i, "HEIC".into())).or_else(|e| avif(&data).map(|i| (i, "AVIF".into())).map_err(|_| e)),
        };
    }
    let mut reader = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format().map(|f| format!("{f:?}").to_uppercase()).unwrap_or_else(|| "Image".into());
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok((img, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brands() {
        let ftyp = |b: &[u8; 4]| [&[0, 0, 0, 0x18][..], b"ftyp", b].concat();
        assert_eq!(heif_brand(&ftyp(b"avif")), Some(Heif::Avif));
        assert_eq!(heif_brand(&ftyp(b"heic")), Some(Heif::Hevc));
        assert_eq!(heif_brand(&ftyp(b"mif1")), Some(Heif::Hevc));
        assert_eq!(heif_brand(&ftyp(b"isom")), None);
        assert_eq!(heif_brand(b"\x89PNG\r\n\x1a\n\0\0\0\0"), None);
    }

    /// `TABULA_SAMPLES=dir cargo test -p tabula-images samples -- --ignored`
    /// with example.avif and example.heic (libheif's examples) in `dir`.
    #[test]
    #[ignore]
    fn samples() {
        let dir = std::path::PathBuf::from(std::env::var("TABULA_SAMPLES").expect("TABULA_SAMPLES"));
        for (name, format) in [("example.avif", "AVIF"), ("example.heic", "HEIC")] {
            let (img, f) = open(&dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(f, format);
            assert!(img.width() > 100 && img.height() > 100);
        }
    }
}
