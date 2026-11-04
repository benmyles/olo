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

/// AV1 through rav1d's dav1d API (stills and video), and YUV to RGB.
pub mod av1 {
    use std::{mem::MaybeUninit, ptr::NonNull};

    use rav1d::include::dav1d::{
        data::Dav1dData,
        dav1d::{Dav1dContext, Dav1dSettings},
        headers::*,
        picture::Dav1dPicture,
    };
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

    /// A still: the first picture in `obus`.
    pub fn decode(obus: &[u8]) -> Result<Frame, String> {
        if obus.is_empty() {
            return Err("no AV1 data".into());
        }
        // Callers decode on a worker already.
        let mut dec = Decoder::new(1, 1)?;
        let mut out = Vec::new();
        dec.decode(obus, &mut out)?;
        if out.is_empty() {
            dec.flush(&mut out);
        }
        out.into_iter().next().ok_or_else(|| "no picture in the AV1 data".into())
    }

    /// A stream of AV1 temporal units (video) through rav1d.
    pub struct Decoder {
        ctx: Option<Dav1dContext>,
    }

    impl Decoder {
        /// `threads` 0 picks for the machine; `delay` is how many frames
        /// may be in flight (0 likewise, 1 for pictures out at once).
        pub fn new(threads: i32, delay: i32) -> Result<Decoder, String> {
            unsafe {
                let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
                dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
                let mut settings = settings.assume_init();
                settings.n_threads = threads;
                settings.max_frame_delay = delay;
                settings.frame_size_limit = super::MAX_PIXELS as u32;
                let mut ctx = None;
                if dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut settings))).0 < 0 || ctx.is_none() {
                    return Err("AV1 decoder failed to start".into());
                }
                Ok(Decoder { ctx })
            }
        }

        /// Send one temporal unit; pictures that come out join `out`.
        pub fn decode(&mut self, unit: &[u8], out: &mut Vec<Frame>) -> Result<(), String> {
            if unit.is_empty() {
                return Ok(());
            }
            unsafe {
                let mut data = Dav1dData::default();
                let buf = dav1d_data_create(Some(NonNull::from(&mut data)), unit.len());
                if buf.is_null() {
                    return Err("out of memory".into());
                }
                std::ptr::copy_nonoverlapping(unit.as_ptr(), buf, unit.len());
                let mut result = Ok(());
                while data.sz > 0 {
                    let r = dav1d_send_data(self.ctx, Some(NonNull::from(&mut data))).0;
                    if r < 0 && r != -EAGAIN {
                        result = Err(format!("AV1 data rejected ({r})"));
                        break;
                    }
                    // Full: take pictures out to make room, then resend.
                    if let Err(e) = self.pictures(out) {
                        result = Err(e);
                        break;
                    }
                }
                if data.sz > 0 {
                    dav1d_data_unref(Some(NonNull::from(&mut data)));
                }
                result
            }
        }

        /// At the end of the stream: the pictures still inside.
        pub fn flush(&mut self, out: &mut Vec<Frame>) {
            let _ = self.pictures(out);
        }

        fn pictures(&mut self, out: &mut Vec<Frame>) -> Result<(), String> {
            loop {
                let mut pic = Dav1dPicture::default();
                let r = unsafe { dav1d_get_picture(self.ctx, Some(NonNull::from(&mut pic))).0 };
                if r == -EAGAIN {
                    return Ok(());
                }
                if r < 0 {
                    return Err(format!("AV1 decode failed ({r})"));
                }
                let frame = unsafe { Frame::copy(&pic) };
                unsafe { dav1d_picture_unref(Some(NonNull::from(&mut pic))) };
                out.push(frame?);
            }
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
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

        /// A frame from planes another decoder made (VP9's): each plane
        /// row by row, `stride` samples a row; `matrix` as dav1d's
        /// `DAV1D_MC_*` (ISO/IEC 23091-4's matrix coefficients).
        #[allow(clippy::too_many_arguments)]
        pub(crate) fn from_planes(width: u32, height: u32, bpc: u32, chroma: Option<(u32, u32)>, planes: [Vec<u16>; 3], stride: [usize; 3], matrix: u32, full_range: bool) -> Frame {
            Frame { width, height, bpc, chroma, planes, stride, matrix, full_range }
        }

        #[cfg(test)]
        pub(crate) fn synthetic(width: u32, height: u32, bpc: u32, chroma: (u32, u32), planes: [Vec<u16>; 3], matrix: u32, full_range: bool) -> Frame {
            let cw = (width as usize + chroma.0 as usize) >> chroma.0;
            Frame { width, height, bpc, chroma: Some(chroma), planes, stride: [width as usize, cw, cw], matrix, full_range }
        }

        /// Kr and Kb; unspecified is BT.601, as libavif and browsers assume.
        fn coefficients(&self) -> (f32, f32) {
            match self.matrix {
                DAV1D_MC_BT709 => (0.2126, 0.0722),
                DAV1D_MC_FCC => (0.30, 0.11),
                DAV1D_MC_SMPTE240 => (0.212, 0.087),
                DAV1D_MC_BT2020_NCL | DAV1D_MC_BT2020_CL => (0.2627, 0.0593),
                _ => (0.299, 0.114),
            }
        }

        pub fn to_rgba(&self) -> Vec<u8> {
            let shift = (self.bpc - 8) as i32;
            let max = ((1u32 << self.bpc) - 1) as f32;
            let half = (1u32 << (self.bpc - 1)) as f32;
            let (y0, ys, cs) = if self.full_range { (0.0, max, max) } else { ((16 << shift) as f32, (219 << shift) as f32, (224 << shift) as f32) };
            let (kr, kb) = self.coefficients();
            let kg = 1.0 - kr - kb;
            let (w, h) = (self.width, self.height);
            if let Some((sx, sy)) = self.chroma
                && !matches!(self.matrix, DAV1D_MC_IDENTITY | DAV1D_MC_SMPTE_YCGCO)
            {
                // The usual case, fast enough for video: tables per code
                // value in 16.16 fixed point, rows at a time.
                let n = 1usize << self.bpc;
                let fix = |v: f32| (v * 255.0 * 65536.0).round() as i32;
                let luma: Vec<i32> = (0..n).map(|v| fix((v as f32 - y0) / ys) + 32768).collect();
                let c = |k: f32| -> Vec<i32> { (0..n).map(|v| fix(k * (v as f32 - half) / cs)).collect() };
                let (cr_r, cb_b) = (c(2.0 * (1.0 - kr)), c(2.0 * (1.0 - kb)));
                let (cr_g, cb_g) = (c(-2.0 * (1.0 - kr) * kr / kg), c(-2.0 * (1.0 - kb) * kb / kg));
                let top = n - 1;
                let byte = |v: i32| (v >> 16).clamp(0, 255) as u8;
                let mut out = vec![255u8; w as usize * h as usize * 4];
                for (y, row) in out.chunks_exact_mut(w as usize * 4).enumerate() {
                    let ly = &self.planes[0][y * self.stride[0]..][..w as usize];
                    let cy = (y >> sy) * self.stride[1];
                    let (us, vs) = (&self.planes[1][cy..], &self.planes[2][cy..]);
                    for (x, (px, &l)) in row.as_chunks_mut::<4>().0.iter_mut().zip(ly).enumerate() {
                        let (u, v) = (us[x >> sx] as usize & top, vs[x >> sx] as usize & top);
                        let l = luma[l as usize & top];
                        px[0] = byte(l + cr_r[v]);
                        px[1] = byte(l + cr_g[v] + cb_g[u]);
                        px[2] = byte(l + cb_b[u]);
                    }
                }
                return out;
            }
            self.to_rgba_exact()
        }

        /// The same in floating point, for every matrix and layout.
        pub(crate) fn to_rgba_exact(&self) -> Vec<u8> {
            let shift = (self.bpc - 8) as i32;
            let max = ((1u32 << self.bpc) - 1) as f32;
            let half = (1u32 << (self.bpc - 1)) as f32;
            let (y0, ys, cs) = if self.full_range { (0.0, max, max) } else { ((16 << shift) as f32, (219 << shift) as f32, (224 << shift) as f32) };
            let (kr, kb) = self.coefficients();
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

/// VP9 through filmcraft-vp9, a pure-Rust decoder (clean-room, from the
/// VP9 bitstream specification), as frames like AV1's: the same planes,
/// so the same conversion to RGBA and BGRA, posters and player.
pub mod vp9 {
    pub use super::av1::Frame;
    /// Whether a sample starts with a key frame, and its stream's facts.
    pub use filmcraft_vp9::{KeyframeInfo, is_keyframe, keyframe_info};
    use rav1d::include::dav1d::headers::*;

    /// A stream of VP9 frames (each sample a frame or a superframe, as
    /// WebM, Matroska and MP4 store them).
    pub struct Decoder(filmcraft_vp9::Decoder);

    impl Decoder {
        /// `threads` workers (0: as many as the machine has, up to eight;
        /// pictures may then come out a sample or two late).
        pub fn new(threads: usize) -> Decoder {
            let n = if threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()).min(8) } else { threads };
            Decoder(filmcraft_vp9::Decoder::with_threads(n))
        }

        /// One sample, shown at `pts`: the pictures finished so far join
        /// `out` in order, each with the `pts` of the sample that showed it.
        pub fn decode(&mut self, sample: &[u8], pts: i64, out: &mut Vec<(i64, Frame)>) -> Result<(), String> {
            let pics = self.0.decode(sample, pts).map_err(|e| format!("VP9: {e}"))?;
            out.extend(pics.iter().filter_map(|p| Some((p.pts, frame(p)?))));
            Ok(())
        }

        /// At the end of the stream: the pictures still inside.
        pub fn flush(&mut self, out: &mut Vec<(i64, Frame)>) {
            out.extend(self.0.flush().iter().filter_map(|p| Some((p.pts, frame(p)?))));
        }

        /// Forget the stream so far (after a seek, before its key frame),
        /// keeping the workers.
        pub fn reset(&mut self) {
            self.0.reset();
        }
    }

    /// A still: the first picture in `sample` (a key frame).
    pub fn decode(sample: &[u8]) -> Result<Frame, String> {
        // Callers decode on a worker already.
        let mut dec = Decoder::new(1);
        let mut out = Vec::new();
        dec.decode(sample, 0, &mut out)?;
        if out.is_empty() {
            dec.flush(&mut out);
        }
        out.into_iter().next().map(|(_, f)| f).ok_or_else(|| "no picture in the VP9 data".into())
    }

    /// VP9's colour space as dav1d's matrix (0 unknown: BT.601, as for
    /// AV1; 7 sRGB: the planes are G, B, R, as AV1's identity).
    fn matrix(space: u8) -> u32 {
        match space {
            2 => DAV1D_MC_BT709,
            4 => DAV1D_MC_SMPTE240,
            5 => DAV1D_MC_BT2020_NCL,
            7 => DAV1D_MC_IDENTITY,
            1 | 3 => DAV1D_MC_BT601,
            _ => DAV1D_MC_UNKNOWN,
        }
    }

    /// A picture's planes, cropped to its size, as a frame.
    fn frame(p: &filmcraft_vp9::Picture) -> Option<Frame> {
        let (w, h) = (p.width as usize, p.height as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let (cw, ch) = (p.chroma_width as usize, p.chroma_height as usize);
        let copy = |plane: &filmcraft_vp9::Plane, stride: usize, pw: usize, ph: usize| -> Option<Vec<u16>> {
            let mut out = Vec::with_capacity(pw * ph);
            for y in 0..ph {
                let row = y * stride;
                match plane {
                    filmcraft_vp9::Plane::U8(v) => out.extend(v.get(row..row + pw)?.iter().map(|&s| s as u16)),
                    filmcraft_vp9::Plane::U16(v) => out.extend_from_slice(v.get(row..row + pw)?),
                }
            }
            Some(out)
        };
        let planes = [copy(&p.y, p.y_stride, w, h)?, copy(&p.u, p.uv_stride, cw, ch)?, copy(&p.v, p.uv_stride, cw, ch)?];
        let chroma = Some((p.subsampling_x as u32, p.subsampling_y as u32));
        Some(Frame::from_planes(p.width, p.height, p.bit_depth, chroma, planes, [w, cw, cw], matrix(p.color.color_space), p.color.full_range))
    }
}

/// HEVC (H.265) video, decoded in process by rust_h265 (pure Rust: Main,
/// Main 10 and the range extensions, bit-exact on the JCT-VC conformance
/// streams), its pictures as AV1's [`Frame`]s.
pub mod hevc {
    pub use super::av1::Frame;
    use rav1d::include::dav1d::headers::*;

    /// What a track's HEVCDecoderConfigurationRecord says (MP4's `hvcC`,
    /// Matroska's CodecPrivate): each NAL unit's length's size, and the
    /// parameter sets (VPS, SPS, PPS) to start from.
    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub nal_len: u8,
        pub sets: Vec<Vec<u8>>,
    }

    /// The record `r` read, or None if it isn't one.
    pub fn config(r: &[u8]) -> Option<Config> {
        // 22 bytes of profile and stream facts (the length's size in the
        // last), the number of arrays, then each: its NAL type, how many,
        // and each one's length and bytes.
        let nal_len = (r.get(21)? & 3) + 1;
        let arrays = *r.get(22)? as usize;
        let mut at = 23;
        let mut sets = Vec::new();
        for _ in 0..arrays {
            let n = u16::from_be_bytes(r.get(at + 1..at + 3)?.try_into().ok()?) as usize;
            at += 3;
            for _ in 0..n {
                let len = u16::from_be_bytes(r.get(at..at + 2)?.try_into().ok()?) as usize;
                sets.push(r.get(at + 2..at + 2 + len)?.to_vec());
                at += 2 + len;
            }
        }
        Some(Config { nal_len, sets })
    }

    /// How the samples are coloured: H.273's matrix (MP4's `colr`, nclx;
    /// Matroska's MatrixCoefficients) and full range. Unknown, BT.709 for
    /// a picture 720 lines and up and BT.601 under, as players assume.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Colour {
        pub matrix: Option<u32>,
        pub full_range: bool,
    }

    /// Whether a sample (length-prefixed NAL units) holds a picture that
    /// starts the stream afresh (IRAP: BLA, IDR or CRA).
    pub fn is_keyframe(sample: &[u8], nal_len: u8) -> bool {
        rust_h265::parse_hvcc(sample, nal_len).iter().any(|n| n.nal_unit_type.is_irap())
    }

    /// A stream of HEVC pictures, in display order.
    pub struct Decoder {
        dec: rust_h265::Decoder,
        config: Config,
        colour: Colour,
        threads: usize,
    }

    impl Decoder {
        /// Starting from `config`'s parameter sets, on `threads` threads (0:
        /// as many as the machine has, up to four, past which 1080p goes no
        /// faster: pictures then come out a sample or two late).
        pub fn new(config: Config, colour: Colour, threads: usize) -> Decoder {
            let threads = if threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()).min(4) } else { threads };
            Decoder { dec: start(&config, threads), config, colour, threads }
        }

        /// One sample (length-prefixed NAL units): the pictures ready
        /// join `out`, in display order.
        pub fn decode(&mut self, sample: &[u8], out: &mut Vec<Frame>) -> Result<(), String> {
            let mut failed = None;
            for nal in rust_h265::parse_hvcc(sample, self.config.nal_len) {
                match self.dec.decode_nal(&nal) {
                    Ok(Some(f)) => out.extend(frame(&f, self.colour)),
                    Ok(None) => {}
                    Err(e) => failed = Some(format!("HEVC: {e:?}")),
                }
                while let Some(f) = self.dec.next_output() {
                    out.extend(frame(&f, self.colour));
                }
            }
            failed.map_or(Ok(()), Err)
        }

        /// At the end of the stream: the pictures still inside.
        pub fn flush(&mut self, out: &mut Vec<Frame>) {
            while let Some(f) = self.dec.flush() {
                out.extend(frame(&f, self.colour));
            }
        }

        /// Forget the stream so far (after a seek, before its key frame).
        pub fn reset(&mut self) {
            self.dec = start(&self.config, self.threads);
        }
    }

    fn start(config: &Config, threads: usize) -> rust_h265::Decoder {
        let mut dec = rust_h265::Decoder::new().with_display_order().with_threads(threads.max(1));
        for set in &config.sets {
            if let Some(nal) = rust_h265::parse_nal(set) {
                let _ = dec.decode_nal(&nal);
            }
        }
        dec
    }

    /// A picture's planes as a frame.
    fn frame(f: &rust_h265::Frame, colour: Colour) -> Option<Frame> {
        let (w, h) = (f.width as usize, f.height as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let (sx, sy) = match f.chroma_format_idc {
            1 => (1, 1),
            2 => (1, 0),
            3 => (0, 0),
            _ => return None,
        };
        let (cw, ch) = ((w + sx) >> sx, (h + sy) >> sy);
        let plane = |p: &rust_h265::PixelData, n: usize| -> Option<Vec<u16>> {
            match p {
                rust_h265::PixelData::U8(v) => Some(v.get(..n)?.iter().map(|&s| s as u16).collect()),
                rust_h265::PixelData::U16(v) => Some(v.get(..n)?.to_vec()),
            }
        };
        let planes = [plane(&f.y, w * h)?, plane(&f.u, cw * ch)?, plane(&f.v, cw * ch)?];
        let matrix = colour.matrix.unwrap_or(if h >= 720 { DAV1D_MC_BT709 } else { DAV1D_MC_BT601 });
        Some(Frame::from_planes(f.width, f.height, f.bit_depth as u32, Some((sx as u32, sy as u32)), planes, [w, cw, cw], matrix, colour.full_range))
    }
}

fn heic(data: &[u8]) -> Result<DynamicImage, String> {
    let img = heifer::decode(data).map_err(|e| e.to_string())?;
    if img.width as u64 * img.height as u64 > MAX_PIXELS {
        return Err("too large".into());
    }
    let rgba = image::RgbaImage::from_raw(img.width, img.height, img.to_rgba8()).map(DynamicImage::ImageRgba8).ok_or_else(|| "bad image size".to_owned())?;
    // (An iPhone's photos: Display P3, its profile embedded.)
    let icc = heifer::read_metadata(data).ok().and_then(|m| m.icc_profile);
    Ok(to_srgb(rgba, icc.as_deref()))
}

/// `img` from the colour profile its file embeds into sRGB, what Olo's
/// apps draw in (and what app_server shows accurately on any display):
/// a Display P3 photo's colours as they are, not washed out as they were
/// when its numbers were taken for sRGB's. Colours sRGB hasn't are mapped
/// in perceptually. Left as it is without a profile, with an sRGB one, or
/// one that isn't RGB or can't be read.
pub fn to_srgb(img: DynamicImage, icc: Option<&[u8]>) -> DynamicImage {
    use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};
    let Some(src) = icc.and_then(|b| ColorProfile::new_from_slice(b).ok()) else { return img };
    let srgb = ColorProfile::new_srgb();
    if src.color_space != DataColorSpace::Rgb {
        return img;
    }
    let near = |a: moxcms::Xyzd, b: moxcms::Xyzd| (a.x - b.x).abs() < 2e-3 && (a.y - b.y).abs() < 2e-3 && (a.z - b.z).abs() < 2e-3;
    if near(src.red_colorant, srgb.red_colorant) && near(src.green_colorant, srgb.green_colorant) && near(src.blue_colorant, srgb.blue_colorant) {
        return img;
    }
    let options = TransformOptions { rendering_intent: RenderingIntent::Perceptual, ..TransformOptions::default() };
    let Ok(t) = src.create_transform_8bit(Layout::Rgba, &srgb, Layout::Rgba, options) else { return img };
    let mut rgba = img.into_rgba8();
    let mut out = vec![0u8; rgba.len()];
    if t.transform(&rgba, &mut out).is_ok() {
        rgba.copy_from_slice(&out);
    }
    DynamicImage::ImageRgba8(rgba)
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
    let icc = decoder.icc_profile().ok().flatten();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok((to_srgb(img, icc.as_deref()), format))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed-point path matches the floating-point one.
    #[test]
    fn yuv_fast_path() {
        for (bpc, full, matrix, chroma) in [(8, false, 1, (1u32, 1u32)), (8, true, 6, (1, 0)), (10, false, 9, (1, 1)), (12, true, 1, (0, 0))] {
            let (w, h) = (37u32, 9u32);
            let (cw, ch) = ((w as usize + chroma.0 as usize) >> chroma.0, (h as usize + chroma.1 as usize) >> chroma.1);
            let top = (1u32 << bpc) - 1;
            let mut seed = 7u32;
            let mut noise = |n: usize| -> Vec<u16> {
                (0..n)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        ((seed >> 8) & top) as u16
                    })
                    .collect()
            };
            let f = av1::Frame::synthetic(w, h, bpc, chroma, [noise(w as usize * h as usize), noise(cw * ch), noise(cw * ch)], matrix, full);
            let (fast, exact) = (f.to_rgba(), f.to_rgba_exact());
            let worst = fast.iter().zip(&exact).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 1, "{bpc} bit, full {full}, matrix {matrix}: off by {worst}");
        }
    }

    /// A Display P3 PNG (its profile embedded) opens in sRGB's numbers;
    /// one without a profile is left as it is.
    #[test]
    fn p3_photos_open_in_srgb() {
        use image::{ImageEncoder, codecs::png::PngEncoder};
        let dir = std::path::Path::new("/var/tmp").join(format!("olo-images-p3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p3 = [200u8, 100, 50, 255];
        let write = |name: &str, icc: Option<Vec<u8>>| {
            let path = dir.join(name);
            let mut bytes = Vec::new();
            let mut e = PngEncoder::new(&mut bytes);
            if let Some(icc) = icc {
                e.set_icc_profile(icc).unwrap();
            }
            e.write_image(&p3, 1, 1, image::ExtendedColorType::Rgba8).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let tagged = open(&write("p3.png", Some(icc))).unwrap().0.into_rgba8();
        let plain = open(&write("plain.png", None)).unwrap().0.into_rgba8();
        assert_eq!(plain.get_pixel(0, 0).0, p3);
        // As olo-color works it out: P3's numbers into sRGB's (both sRGB's
        // curve), the orange a little redder in sRGB's numbers.
        let m = olo_color::Primaries::DISPLAY_P3.to(&olo_color::Primaries::SRGB);
        let lin = [0, 1, 2].map(|i| olo_color::Transfer::Srgb.decode(f64::from(p3[i]) / 255.0));
        let want = m.apply(lin).map(|v| (olo_color::Transfer::Srgb.encode(v.clamp(0.0, 1.0)) * 255.0).round() as i32);
        let got = tagged.get_pixel(0, 0).0;
        for c in 0..3 {
            assert!((i32::from(got[c]) - want[c]).abs() <= 2, "got {got:?}, want {want:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What converting a 12-megapixel photo costs
    /// (`cargo test --release -p olo-images p3_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn p3_cost() {
        let (w, h) = (4032u32, 3024u32);
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])));
        let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let t = std::time::Instant::now();
        let out = to_srgb(img, Some(&icc));
        eprintln!("{w} × {h} from Display P3 into sRGB: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(out.width(), w);
    }

    #[test]
    fn brands() {
        let ftyp = |b: &[u8; 4]| [&[0, 0, 0, 0x18][..], b"ftyp", b].concat();
        assert_eq!(heif_brand(&ftyp(b"avif")), Some(Heif::Avif));
        assert_eq!(heif_brand(&ftyp(b"heic")), Some(Heif::Hevc));
        assert_eq!(heif_brand(&ftyp(b"mif1")), Some(Heif::Hevc));
        assert_eq!(heif_brand(&ftyp(b"isom")), None);
        assert_eq!(heif_brand(b"\x89PNG\r\n\x1a\n\0\0\0\0"), None);
    }

    /// `OLO_SAMPLES=dir cargo test -p olo-images samples -- --ignored`
    /// with example.avif and example.heic (libheif's examples) in `dir`.
    #[test]
    #[ignore]
    fn samples() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        for (name, format) in [("example.avif", "AVIF"), ("example.heic", "HEIC")] {
            let (img, f) = open(&dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(f, format);
            assert!(img.width() > 100 && img.height() > 100);
        }
    }
}
