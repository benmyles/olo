//! Video decoded on Intel's and AMD's GPUs through VA-API: AV1 and VP9,
//! what Fedora's own drivers decode (iHD and Mesa leave out the
//! patented codecs, so H.264 and HEVC aren't there; NVIDIA's GPU has
//! NVDEC for those, `olo_video::nvdec`, and the processor has them all).
//!
//! VA-API decodes a picture from what the stream's headers say, which
//! it leaves to us: cros-codecs' parsers (pure Rust, nothing linked)
//! read the OBUs and frames, and each picture's parameters are filled
//! here as FFmpeg's and ChromeOS's VA-API decoders fill them. Each
//! decoded picture is copied from the GPU into planes, luma and chroma
//! interleaved (NV12, or P010 for 10-bit video), which the window draws
//! as they are (`olo_yuv::YuvFrame`).
//!
//! A picture with film grain is decoded twice over by the driver: the
//! picture it keeps for reference, and the one shown (grain applied),
//! each a surface of its own.

use std::{
    cell::RefCell,
    ffi::{c_int, c_uint, c_void},
    path::PathBuf,
    ptr::null_mut,
    rc::Rc,
    sync::{Arc, OnceLock},
};

use cros_codecs::codec::{av1::parser as av1, vp9::parser as vp9};
use va_sys as va;

use cros_codecs::decoder::{BlockingMode, stateless::StatelessDecoder};

use crate::{Display, nvidia, render_nodes};

mod h264;
mod h265;
mod stateless;

/// A codec VA-API may decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Av1,
    Vp9,
    H264,
    Hevc,
}

impl Codec {
    /// The VA-API profiles of it that are decoded here (8- and 10-bit
    /// 4:2:0).
    fn profiles(self) -> &'static [va::VAProfile] {
        match self {
            Codec::Av1 => &[va::VAProfileAV1Profile0],
            Codec::Vp9 => &[va::VAProfileVP9Profile0, va::VAProfileVP9Profile2],
            Codec::H264 => &[va::VAProfileH264High, va::VAProfileH264Main, va::VAProfileH264ConstrainedBaseline],
            Codec::Hevc => &[va::VAProfileHEVCMain, va::VAProfileHEVCMain10],
        }
    }
}

/// A picture decoded, as the GPU made it.
pub struct Picture {
    /// The presentation time it was sent with.
    pub pts: i64,
    /// Its size (the luma plane's).
    pub width: u32,
    pub height: u32,
    /// 16-bit samples, each in the high bits (P010), else 8-bit (NV12).
    pub deep: bool,
    /// The luma plane, rows packed.
    pub y: Vec<u8>,
    /// Cb and Cr interleaved, half as wide and high (rounded up), rows
    /// packed.
    pub uv: Vec<u8>,
    /// Its matrix coefficients, as ITU-T H.273 numbers them (1 BT.709,
    /// 5 and 6 BT.601, 9 BT.2020; 2 unspecified).
    pub matrix: u8,
    /// Full range (0–255), else limited.
    pub full_range: bool,
    /// Its planes left where the GPU copied them (`Decoder::hold`), `y`
    /// and `uv` then empty.
    pub held: Option<Held>,
}

/// A picture left in the memory the GPU copied it into (`Decoder::hold`):
/// page-aligned, a whole number of pages, which a window's GPU can copy
/// from as it is (`olo_yuv::YuvMemory`), nothing copied by the processor.
/// Its luma at `y`, its chroma (Cb and Cr interleaved) at `uv`: each an
/// offset and a row pitch, in bytes. The memory is the decoder's again
/// when this is dropped (freed, if the decoder has gone).
pub struct Held {
    host: Arc<Host>,
    pub y: (usize, usize),
    pub uv: (usize, usize),
}

impl Held {
    /// Where its memory starts.
    pub fn ptr(&self) -> *const u8 {
        self.host.ptr
    }

    /// How long its memory is (a whole number of pages).
    pub fn len(&self) -> usize {
        self.host.layout.size()
    }

    /// Never empty (a picture has pages).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Its memory.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: ours, allocated with this size, unchanged while held
        // (only a picture nothing holds is written).
        unsafe { std::slice::from_raw_parts(self.host.ptr, self.host.layout.size()) }
    }
}

/// A GPU's driver, what it decodes.
struct Probed {
    node: PathBuf,
    driver: String,
    profiles: Vec<va::VAProfile>,
}

/// What each GPU (not NVIDIA's) decodes, looked at once.
fn probed() -> &'static [Probed] {
    static PROBED: OnceLock<Vec<Probed>> = OnceLock::new();
    PROBED.get_or_init(|| {
        let all = [Codec::Av1, Codec::Vp9, Codec::H264, Codec::Hevc].iter().flat_map(|c| c.profiles().iter().copied()).collect::<Vec<_>>();
        render_nodes()
            .into_iter()
            .filter(|n| !nvidia(n))
            .filter_map(|node| {
                let d = Display::open(&node).ok()?;
                let profiles = all.iter().copied().filter(|&p| d.entrypoints(p).contains(&va::VAEntrypointVLD)).collect();
                Some(Probed { node, driver: d.vendor.clone(), profiles })
            })
            .collect()
    })
}

/// The GPU that decodes `codec` (its first profile, 8-bit), and its
/// driver's name, if one does.
pub fn decoder_for(codec: Codec) -> Option<(PathBuf, String)> {
    probed().iter().find(|p| p.profiles.contains(&codec.profiles()[0])).map(|p| (p.node.clone(), p.driver.clone()))
}

/// Whether a GPU here decodes `codec` through VA-API.
pub fn available(codec: Codec) -> bool {
    decoder_for(codec).is_some()
}

/// A reference slot's pictures: the one decoded (kept for reference)
/// and the one shown (film grain applied; else the same).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Slot {
    kept: usize,
    shown: usize,
}

/// What pictures are decoded into: a configuration, a context and its
/// surfaces, for one profile and size.
struct Session {
    profile: va::VAProfile,
    width: u32,
    height: u32,
    deep: bool,
    config: va::VAConfigID,
    context: va::VAContextID,
    surfaces: Vec<va::VASurfaceID>,
    /// What pictures are copied out through: linear surfaces in memory of
    /// ours (`Linear`), made as needed (one, unless pictures are held:
    /// one each, up to `HELD`); `None` till the first picture, empty
    /// where the driver can't (then surfaces are mapped, `read_mapped`).
    linear: Option<Vec<Linear>>,
}

/// The most pictures held at once (`Decoder::hold`; past it, copied out).
const HELD: usize = 16;

/// An AV1 picture under way: its header, slot and buffers so far.
struct Current {
    header: av1::FrameHeaderObu,
    slot: Slot,
    buffers: Vec<va::VABufferID>,
}

/// AV1 and VP9, whose references are slots the stream names.
struct Frames {
    codec: Codec,
    va: Va,
    /// The eight reference slots.
    refs: [Option<Slot>; 8],
    /// After a seek: nothing is decoded until a key frame.
    waiting: bool,
    av1: av1::Parser,
    sequence: Option<Rc<av1::SequenceHeaderObu>>,
    current: Option<Current>,
    vp9: vp9::Parser,
    segments: [vp9::Segmentation; vp9::MAX_SEGMENTS],
    /// The size each surface's picture was decoded at.
    sizes: Vec<(u32, u32)>,
}

/// A stream decoded on a GPU through VA-API.
pub struct Decoder {
    inner: Inner,
    driver: String,
}

enum Inner {
    Frames(Box<Frames>),
    H264(Box<stateless::Stream<StatelessDecoder<cros_codecs::decoder::stateless::h264::H264, stateless::Backend>>>),
    Hevc(Box<stateless::Stream<StatelessDecoder<cros_codecs::decoder::stateless::h265::H265, stateless::Backend>>>),
}

impl Decoder {
    /// A decoder for `codec` on the GPU that decodes it.
    pub fn new(codec: Codec) -> Result<Decoder, String> {
        let (node, _) = decoder_for(codec).ok_or_else(|| format!("no GPU here decodes {codec:?} through VA-API"))?;
        let d = Display::open(&node)?;
        let driver = d.vendor.clone();
        let stream = |d: Display| {
            let va = Rc::new(RefCell::new(Va { d, session: None, hold: false }));
            let look: Rc<RefCell<stateless::Look>> = Rc::default();
            (stateless::Backend::new(va.clone(), look.clone()), va, look)
        };
        let inner = match codec {
            Codec::Av1 | Codec::Vp9 => Inner::Frames(Box::new(Frames::new(codec, d))),
            Codec::H264 => {
                let (backend, va, look) = stream(d);
                let dec = StatelessDecoder::new(backend, BlockingMode::Blocking).map_err(|e| e.to_string())?;
                Inner::H264(Box::new(stateless::Stream { dec, va, look }))
            }
            Codec::Hevc => {
                let (backend, va, look) = stream(d);
                let dec = StatelessDecoder::new(backend, BlockingMode::Blocking).map_err(|e| e.to_string())?;
                Inner::Hevc(Box::new(stateless::Stream { dec, va, look }))
            }
        };
        Ok(Decoder { inner, driver })
    }

    /// What decodes, as its driver names itself.
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// Pictures left in the memory the GPU copies them into (`Held`), for
    /// a window's GPU to copy from as they are, rather than copied out
    /// into planes by the processor.
    pub fn hold(&mut self, on: bool) {
        match &mut self.inner {
            Inner::Frames(f) => f.va.hold = on,
            Inner::H264(s) => s.va.borrow_mut().hold = on,
            Inner::Hevc(s) => s.va.borrow_mut().hold = on,
        }
    }

    /// Decodes `data` (a sample: an AV1 temporal unit, a VP9 frame or
    /// superframe, H.264's or HEVC's NAL units in Annex B), shown at
    /// `pts`; the pictures it shows go in `out` (H.264's and HEVC's in
    /// display order, each with its own sample's time).
    pub fn decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        match &mut self.inner {
            Inner::Frames(f) => f.decode(data, pts, out),
            Inner::H264(s) => s.decode(data, pts, out),
            Inner::Hevc(s) => s.decode(data, pts, out),
        }
    }

    /// The stream ended: the pictures still held, shown.
    pub fn flush(&mut self, out: &mut Vec<Picture>) -> Result<(), String> {
        match &mut self.inner {
            Inner::Frames(_) => Ok(()),
            Inner::H264(s) => s.flush(out),
            Inner::Hevc(s) => s.flush(out),
        }
    }

    /// Forgets the pictures it holds (a seek): it starts again at the
    /// next key frame.
    pub fn reset(&mut self) {
        match &mut self.inner {
            Inner::Frames(f) => f.reset(),
            Inner::H264(s) => s.reset(),
            Inner::Hevc(s) => s.reset(),
        }
    }
}

impl Frames {
    fn new(codec: Codec, d: Display) -> Frames {
        Frames {
            codec,
            va: Va { d, session: None, hold: false },
            refs: [None; 8],
            waiting: false,
            av1: av1::Parser::default(),
            sequence: None,
            current: None,
            vp9: vp9::Parser::default(),
            segments: Default::default(),
            sizes: Vec::new(),
        }
    }

    fn decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        match self.codec {
            Codec::Av1 => self.av1_decode(data, pts, out),
            Codec::Vp9 => self.vp9_decode(data, pts, out),
            Codec::H264 | Codec::Hevc => Err("not a codec of frames".into()),
        }
    }

    fn reset(&mut self) {
        if let Some(c) = self.current.take() {
            self.va.drop_buffers(&c.buffers);
        }
        self.refs = [None; 8];
        self.waiting = true;
    }

    // ---- AV1 ----

    fn av1_decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        let mut rest = data;
        while !rest.is_empty() {
            let obu = match self.av1.read_obu(rest)? {
                av1::ObuAction::Process(obu) => obu,
                av1::ObuAction::Drop(n) => {
                    rest = rest.get(n as usize..).unwrap_or_default();
                    continue;
                }
            };
            let used = obu.bytes_used;
            match self.av1.parse_obu(obu)? {
                av1::ParsedObu::SequenceHeader(seq) => {
                    if self.sequence.as_deref() != Some(&*seq) {
                        if let Some(c) = self.current.take() {
                            self.va.drop_buffers(&c.buffers);
                        }
                        self.refs = [None; 8];
                        self.sequence = Some(seq);
                    }
                }
                av1::ParsedObu::FrameHeader(header) => {
                    self.av1_finish(pts, out)?;
                    self.av1_begin(header)?;
                }
                av1::ParsedObu::TileGroup(tiles) => self.av1_tiles(&tiles)?,
                av1::ParsedObu::Frame(frame) => {
                    self.av1_finish(pts, out)?;
                    self.av1_begin(frame.header)?;
                    self.av1_tiles(&frame.tile_group)?;
                    self.av1_finish(pts, out)?;
                }
                av1::ParsedObu::TileList => return Err("AV1's large-scale tiles aren't decoded".into()),
                _ => {}
            }
            rest = rest.get(used..).unwrap_or_default();
        }
        self.av1_finish(pts, out)
    }

    /// A frame's header: a picture begins (or an existing one shows).
    fn av1_begin(&mut self, header: av1::FrameHeaderObu) -> Result<(), String> {
        if self.waiting {
            if header.frame_type != av1::FrameType::KeyFrame || header.show_existing_frame {
                // (Still its references' updates, which the parser keeps.)
                self.av1.ref_frame_update(&header)?;
                return Ok(());
            }
            self.waiting = false;
        }
        if header.show_existing_frame {
            let slot = self.refs[header.frame_to_show_map_idx as usize].ok_or("AV1: a frame shown that isn't there")?;
            self.current = Some(Current { header, slot, buffers: Vec::new() });
            return Ok(());
        }
        let seq = self.sequence.clone().ok_or("AV1: a frame before its sequence header")?;
        let deep = match seq.bit_depth {
            av1::BitDepth::Depth8 => false,
            av1::BitDepth::Depth10 => true,
            av1::BitDepth::Depth12 => return Err("AV1: 12-bit video isn't decoded on the GPU".into()),
        };
        if seq.seq_profile != av1::Profile::Profile0 || seq.color_config.mono_chrome {
            return Err("AV1: only 4:2:0 video (profile 0) is decoded on the GPU".into());
        }
        let grain = seq.film_grain_params_present;
        let (w, h) = (seq.max_frame_width_minus_1 as u32 + 1, seq.max_frame_height_minus_1 as u32 + 1);
        // Each reference's two pictures, the new one's two, and one over.
        self.session(va::VAProfileAV1Profile0, w, h, deep, if grain { 19 } else { 10 })?;
        let kept = self.free(&[])?;
        let applies = grain && header.film_grain_params.apply_grain;
        let shown = if applies { self.free(&[kept])? } else { kept };
        let s = self.va.session.as_ref().unwrap();
        let surface = |i: usize| s.surfaces[i];
        let refs = self.refs.map(|r| r.map_or(va::VA_INVALID_SURFACE, |r| surface(r.kept)));
        let display = if applies { surface(shown) } else { va::VA_INVALID_SURFACE };
        let mut p = av1_picture(&header, &seq, surface(kept), display, refs)?;
        let buffer = self.va.buffer(va::VAPictureParameterBufferType, &mut p, 1)?;
        self.current = Some(Current { header, slot: Slot { kept, shown }, buffers: vec![buffer] });
        Ok(())
    }

    /// A tile group of the picture under way.
    fn av1_tiles(&mut self, group: &av1::TileGroupObu) -> Result<(), String> {
        let Some(current) = &self.current else {
            // (A picture skipped until a key frame.)
            return Ok(());
        };
        if current.header.show_existing_frame {
            return Err("AV1: tiles for a frame that shows an existing one".into());
        }
        let mut params: Vec<va::VASliceParameterBufferAV1> = group
            .tiles
            .iter()
            .map(|t| va::VASliceParameterBufferAV1 {
                slice_data_size: t.tile_size,
                slice_data_offset: t.tile_offset,
                slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
                tile_row: t.tile_row as u16,
                tile_column: t.tile_col as u16,
                tg_start: group.tg_start as u16,
                tg_end: group.tg_end as u16,
                ..Default::default()
            })
            .collect();
        let n = params.len();
        if n == 0 {
            return Ok(());
        }
        let p = self.va.buffer_n(va::VASliceParameterBufferType, params.as_mut_ptr(), n)?;
        let data = group.obu.as_ref();
        let d = match self.va.buffer_bytes(va::VASliceDataBufferType, data) {
            Ok(d) => d,
            Err(e) => {
                self.va.drop_buffers(&[p]);
                return Err(e);
            }
        };
        let current = self.current.as_mut().unwrap();
        current.buffers.extend([p, d]);
        Ok(())
    }

    /// The picture under way decoded, the references updated, and the
    /// picture out if it shows.
    fn av1_finish(&mut self, pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        let Some(current) = self.current.take() else { return Ok(()) };
        let header = &current.header;
        if !header.show_existing_frame {
            let surface = self.va.session.as_ref().unwrap().surfaces[current.slot.kept];
            let done = self.va.run(surface, &current.buffers);
            self.va.drop_buffers(&current.buffers);
            done?;
        }
        // An existing key frame shown refreshes the references too.
        if !header.show_existing_frame || header.frame_type == av1::FrameType::KeyFrame {
            for (i, r) in self.refs.iter_mut().enumerate() {
                if header.refresh_frame_flags & (1 << i) != 0 {
                    *r = Some(current.slot);
                }
            }
        }
        self.av1.ref_frame_update(header)?;
        if header.show_frame || header.show_existing_frame {
            let seq = self.sequence.clone().ok_or("AV1: no sequence header")?;
            let (w, h) = (header.upscaled_width, header.frame_height);
            let cc = &seq.color_config;
            let picture = self.va.read(current.slot.shown, w, h, pts, cc.matrix_coefficients as u8, cc.color_range)?;
            out.push(picture);
        }
        Ok(())
    }

    // ---- VP9 ----

    fn vp9_decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        let frames = self.vp9.parse_chunk(data)?;
        for frame in frames {
            let h = &frame.header;
            let key = h.frame_type == vp9::FrameType::KeyFrame;
            if self.waiting {
                if !key {
                    continue;
                }
                self.waiting = false;
            }
            if h.show_existing_frame {
                let slot = self.refs[h.frame_to_show_map_idx as usize].ok_or("VP9: a frame shown that isn't there")?;
                let (w, height) = self.size_of(slot);
                out.push(self.va.read(slot.shown, w, height, pts, vp9_matrix(h.color_space), h.color_range == vp9::ColorRange::FullSwing)?);
                continue;
            }
            let (profile, deep) = match (h.profile, h.bit_depth) {
                (vp9::Profile::Profile0, _) => (va::VAProfileVP9Profile0, false),
                (vp9::Profile::Profile2, vp9::BitDepth::Depth10) => (va::VAProfileVP9Profile2, true),
                _ => return Err(format!("VP9: {:?} at {:?} isn't decoded on the GPU", h.profile, h.bit_depth)),
            };
            // A key frame may change the size; another frame decodes
            // into the surfaces there are (as big, or bigger).
            let fits = self.va.session.as_ref().is_some_and(|s| s.profile == profile && s.width >= h.width && s.height >= h.height);
            if key || !fits {
                self.session(profile, h.width, h.height, deep, 10)?;
            }
            vp9::Segmentation::update_segmentation(&mut self.segments, h);
            let kept = self.free(&[])?;
            let surfaces = &self.va.session.as_ref().unwrap().surfaces;
            let refs = self.refs.map(|r| r.map_or(va::VA_INVALID_SURFACE, |r| surfaces[r.kept]));
            let surface = surfaces[kept];
            let bytes = frame.as_ref();
            let mut p = vp9_picture(h, refs);
            let mut s = vp9_slice(&self.segments, bytes.len());
            let mut buffers = Vec::with_capacity(3);
            let made = (|| -> Result<(), String> {
                buffers.push(self.va.buffer(va::VAPictureParameterBufferType, &mut p, 1)?);
                buffers.push(self.va.buffer(va::VASliceParameterBufferType, &mut s, 1)?);
                buffers.push(self.va.buffer_bytes(va::VASliceDataBufferType, bytes)?);
                Ok(())
            })();
            let done = made.and_then(|_| self.va.run(surface, &buffers));
            self.va.drop_buffers(&buffers);
            done?;
            let slot = Slot { kept, shown: kept };
            for (i, r) in self.refs.iter_mut().enumerate() {
                if h.refresh_frame_flags & (1 << i) != 0 {
                    *r = Some(slot);
                }
            }
            self.sizes[kept] = (h.width, h.height);
            if h.show_frame {
                out.push(self.va.read(kept, h.width, h.height, pts, vp9_matrix(h.color_space), h.color_range == vp9::ColorRange::FullSwing)?);
            }
        }
        Ok(())
    }

    // ---- The GPU's side ----

    /// The session for `profile` at `width` × `height` (a new one: the
    /// references go with the old).
    fn session(&mut self, profile: va::VAProfile, width: u32, height: u32, deep: bool, count: usize) -> Result<(), String> {
        if self.va.session(profile, width, height, deep, count)? {
            self.refs = [None; 8];
            self.sizes = vec![(width, height); count];
        }
        Ok(())
    }

    /// A surface no reference holds, nor any of `but`.
    fn free(&self, but: &[usize]) -> Result<usize, String> {
        let n = self.va.session.as_ref().map_or(0, |s| s.surfaces.len());
        (0..n).find(|i| !but.contains(i) && !self.refs.iter().flatten().any(|r| r.kept == *i || r.shown == *i)).ok_or_else(|| "no surface free to decode into".into())
    }

    /// The size the picture in a slot was decoded at.
    fn size_of(&self, slot: Slot) -> (u32, u32) {
        self.sizes.get(slot.kept).copied().unwrap_or((0, 0))
    }
}

/// libva's display, and the session pictures are decoded in there.
struct Va {
    d: Display,
    session: Option<Session>,
    /// Pictures left where the GPU copied them (`Decoder::hold`).
    hold: bool,
}

impl Va {
    /// A session for `profile` at `width` × `height` with `count`
    /// surfaces: the one there is if it's the same, else a new one (true:
    /// pictures of the old are gone).
    fn session(&mut self, profile: va::VAProfile, width: u32, height: u32, deep: bool, count: usize) -> Result<bool, String> {
        if self.session.as_ref().is_some_and(|s| s.profile == profile && s.width == width && s.height == height && s.deep == deep && s.surfaces.len() >= count) {
            return Ok(false);
        }
        self.end_session();
        let api = &self.d.api;
        let dpy = self.d.dpy;
        let rt = if deep { va::VA_RT_FORMAT_YUV420_10 } else { va::VA_RT_FORMAT_YUV420 };
        let mut attr = va::VAConfigAttrib { type_: va::VAConfigAttribRTFormat, value: rt };
        let mut config = 0;
        // SAFETY: our display; one attribute.
        api.ok(unsafe { (api.create_config)(dpy, profile, va::VAEntrypointVLD, &mut attr, 1, &mut config) }, "vaCreateConfig")?;
        let mut surfaces = vec![va::VA_INVALID_SURFACE; count];
        // SAFETY: `surfaces` holds `count`; no attributes.
        let st = unsafe { (api.create_surfaces)(dpy, rt, width, height, surfaces.as_mut_ptr(), count as c_uint, null_mut(), 0) };
        if let Err(e) = api.ok(st, "vaCreateSurfaces") {
            // SAFETY: the config just made.
            unsafe { (api.destroy_config)(dpy, config) };
            return Err(e);
        }
        let mut context = 0;
        // SAFETY: the surfaces just made, as the context's targets.
        let st = unsafe { (api.create_context)(dpy, config, width as c_int, height as c_int, va::VA_PROGRESSIVE as c_int, surfaces.as_mut_ptr(), count as c_int, &mut context) };
        if let Err(e) = api.ok(st, "vaCreateContext") {
            // SAFETY: what was just made.
            unsafe {
                (api.destroy_surfaces)(dpy, surfaces.as_mut_ptr(), count as c_int);
                (api.destroy_config)(dpy, config);
            }
            return Err(e);
        }
        self.session = Some(Session { profile, width, height, deep, config, context, surfaces, linear: None });
        Ok(true)
    }

    fn end_session(&mut self) {
        let Some(mut s) = self.session.take() else { return };
        let api = &self.d.api;
        // SAFETY: what the session made, once.
        for l in s.linear.take().into_iter().flatten() {
            l.end(&self.d);
        }
        unsafe {
            (api.destroy_context)(self.d.dpy, s.context);
            (api.destroy_surfaces)(self.d.dpy, s.surfaces.as_mut_ptr(), s.surfaces.len() as c_int);
            (api.destroy_config)(self.d.dpy, s.config);
        }
    }

    /// A buffer of one `T` (or `n`, from `value`).
    fn buffer<T>(&self, kind: va::VABufferType, value: &mut T, n: usize) -> Result<va::VABufferID, String> {
        self.buffer_n(kind, value as *mut T, n)
    }

    fn buffer_n<T>(&self, kind: va::VABufferType, first: *mut T, n: usize) -> Result<va::VABufferID, String> {
        let s = self.session.as_ref().ok_or("no session")?;
        let mut id = va::VA_INVALID_ID;
        // SAFETY: `first` points at `n` values of `T`, copied by libva.
        let st = unsafe { (self.d.api.create_buffer)(self.d.dpy, s.context, kind, std::mem::size_of::<T>() as c_uint, n as c_uint, first as *mut c_void, &mut id) };
        self.d.api.ok(st, "vaCreateBuffer")?;
        Ok(id)
    }

    fn buffer_bytes(&self, kind: va::VABufferType, bytes: &[u8]) -> Result<va::VABufferID, String> {
        let s = self.session.as_ref().ok_or("no session")?;
        let mut id = va::VA_INVALID_ID;
        // SAFETY: `bytes` is read, copied by libva.
        let st = unsafe { (self.d.api.create_buffer)(self.d.dpy, s.context, kind, bytes.len() as c_uint, 1, bytes.as_ptr() as *mut c_void, &mut id) };
        self.d.api.ok(st, "vaCreateBuffer")?;
        Ok(id)
    }

    fn drop_buffers(&self, buffers: &[va::VABufferID]) {
        for &b in buffers {
            // SAFETY: a buffer of ours, once.
            unsafe { (self.d.api.destroy_buffer)(self.d.dpy, b) };
        }
    }

    /// Decodes into `surface` from `buffers`.
    fn run(&self, surface: va::VASurfaceID, buffers: &[va::VABufferID]) -> Result<(), String> {
        let s = self.session.as_ref().ok_or("no session")?;
        let api = &self.d.api;
        let mut list = buffers.to_vec();
        // SAFETY: our context, surface and buffers.
        unsafe {
            api.ok((api.begin_picture)(self.d.dpy, s.context, surface), "vaBeginPicture")?;
            let rendered = api.ok((api.render_picture)(self.d.dpy, s.context, list.as_mut_ptr(), list.len() as c_int), "vaRenderPicture");
            let ended = api.ok((api.end_picture)(self.d.dpy, s.context), "vaEndPicture");
            rendered.and(ended)
        }
    }

    /// Surface `i`'s picture, `width` × `height`, copied out: by the GPU
    /// into memory of ours (linear, cached), then into planes; else
    /// mapped (the driver untiling it as the processor reads, five
    /// times slower at 4K).
    fn read(&mut self, i: usize, width: u32, height: u32, pts: i64, matrix: u8, full_range: bool) -> Result<Picture, String> {
        let s = self.session.as_mut().ok_or("no session")?;
        let (w, h) = (width.min(s.width), height.min(s.height));
        let surface = s.surfaces[i];
        let deep = s.deep;
        if s.linear.is_none() {
            let made = Linear::new(&self.d, s.width, s.height, deep);
            if let Err(e) = &made {
                tracing::info!("VA-API: no copy into memory of ours ({e}); pictures are mapped");
            }
            s.linear = Some(made.into_iter().collect());
        }
        let pool = s.linear.as_mut().unwrap();
        if pool.is_empty() {
            let (y, uv) = read_mapped(&self.d, surface, w, h, deep)?;
            return Ok(Picture { pts, width: w, height: h, deep, y, uv, matrix, full_range, held: None });
        }
        // One nothing holds (a picture shown and let go), else another made.
        let free = match pool.iter().position(Linear::free) {
            Some(i) => Some(i),
            None if self.hold && pool.len() < HELD => Linear::new(&self.d, s.width, s.height, deep).ok().map(|l| {
                pool.push(l);
                pool.len() - 1
            }),
            None => None,
        };
        let Some(i) = free else {
            let (y, uv) = read_mapped(&self.d, surface, w, h, deep)?;
            return Ok(Picture { pts, width: w, height: h, deep, y, uv, matrix, full_range, held: None });
        };
        let l = &pool[i];
        l.copy_in(&self.d, surface)?;
        if self.hold {
            let held = Held { host: l.host.clone(), y: (0, l.pitch), uv: (l.uv_at, l.pitch) };
            return Ok(Picture { pts, width: w, height: h, deep, y: Vec::new(), uv: Vec::new(), matrix, full_range, held: Some(held) });
        }
        let (y, uv) = l.copy_out(w, h);
        Ok(Picture { pts, width: w, height: h, deep, y, uv, matrix, full_range, held: None })
    }
}

impl Drop for Va {
    fn drop(&mut self) {
        self.end_session();
    }
}

/// Planes of `w` × `h` (rows packed): luma, then Cb and Cr interleaved.
fn planes(w: u32, h: u32, deep: bool) -> (Vec<u8>, Vec<u8>, usize, usize) {
    let bytes = if deep { 2 } else { 1 };
    let row_y = (w * bytes) as usize;
    let row_uv = (w.div_ceil(2) * 2 * bytes) as usize;
    (vec![0u8; row_y * h as usize], vec![0u8; row_uv * h.div_ceil(2) as usize], row_y, row_uv)
}

/// `surface`'s picture, mapped and copied out.
fn read_mapped(d: &Display, surface: va::VASurfaceID, w: u32, h: u32, deep: bool) -> Result<(Vec<u8>, Vec<u8>), String> {
    let api = &d.api;
    // SAFETY: our surface.
    api.ok(unsafe { (api.sync_surface)(d.dpy, surface) }, "vaSyncSurface")?;
    let mut image = va::VAImage::default();
    // SAFETY: our surface, its image ours until destroyed below.
    api.ok(unsafe { (api.derive_image)(d.dpy, surface, &mut image) }, "vaDeriveImage")?;
    let mut mapped: *mut c_void = null_mut();
    // SAFETY: the image's buffer, mapped until unmapped below.
    if let Err(e) = api.ok(unsafe { (api.map_buffer)(d.dpy, image.buf, &mut mapped) }, "vaMapBuffer") {
        // SAFETY: the image made above.
        unsafe { (api.destroy_image)(d.dpy, image.image_id) };
        return Err(e);
    }
    let (mut y, mut uv, row_y, row_uv) = planes(w, h, deep);
    // SAFETY: rows within the mapped image, as its pitches and offsets
    // say; then the image goes.
    unsafe {
        let base = mapped as *const u8;
        copy_rows(base.add(image.offsets[0] as usize), image.pitches[0] as usize, &mut y, row_y);
        copy_rows(base.add(image.offsets[1] as usize), image.pitches[1] as usize, &mut uv, row_uv);
        (api.unmap_buffer)(d.dpy, image.buf);
        (api.destroy_image)(d.dpy, image.image_id);
    }
    Ok((y, uv))
}

/// Page-aligned memory of ours, which a surface uses (`USER_PTR`).
struct Host {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

// SAFETY: plain memory, written only while nothing else holds it.
unsafe impl Send for Host {}
unsafe impl Sync for Host {}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: allocated with this layout, once.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

/// A linear surface in our own memory, which the driver copies a
/// decoded picture into (`vaCopy`): the GPU untiles it, and the
/// processor reads it from cached memory.
struct Linear {
    surface: va::VASurfaceID,
    pitch: usize,
    uv_at: usize,
    deep: bool,
    /// Its memory: held by a picture too while one is (`Held`).
    host: Arc<Host>,
}

impl Linear {
    fn new(d: &Display, width: u32, height: u32, deep: bool) -> Result<Linear, String> {
        let api = &d.api;
        if api.copy.is_none() {
            return Err("this libva has no vaCopy".into());
        }
        let bytes = if deep { 2 } else { 1 };
        let pitch = (width as usize * bytes).next_multiple_of(128);
        let uv_at = pitch * height.next_multiple_of(2) as usize;
        let size = (uv_at + pitch * height.div_ceil(2) as usize).next_multiple_of(4096);
        let layout = std::alloc::Layout::from_size_align(size, 4096).map_err(|e| e.to_string())?;
        // SAFETY: a non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("out of memory".into());
        }
        let host = Arc::new(Host { ptr, layout });
        let (fourcc, rt) = if deep { (va::VA_FOURCC_P010, va::VA_RT_FORMAT_YUV420_10) } else { (va::VA_FOURCC_NV12, va::VA_RT_FORMAT_YUV420) };
        let mut buffers = [host.ptr as usize];
        let mut ext = va::VASurfaceAttribExternalBuffers {
            pixel_format: fourcc,
            width,
            height,
            data_size: size as u32,
            num_planes: 2,
            pitches: [pitch as u32, pitch as u32, 0, 0],
            offsets: [0, uv_at as u32, 0, 0],
            buffers: buffers.as_mut_ptr(),
            num_buffers: 1,
            flags: 0,
            private_data: null_mut(),
        };
        let int = |i: u32| va::VAGenericValue { type_: va::VAGenericValueTypeInteger, value: va::_VAGenericValue__bindgen_ty_1 { i: i as i32 } };
        let mut attrs = [
            va::VASurfaceAttrib { type_: va::VASurfaceAttribPixelFormat, flags: va::VA_SURFACE_ATTRIB_SETTABLE, value: int(fourcc) },
            va::VASurfaceAttrib { type_: va::VASurfaceAttribMemoryType, flags: va::VA_SURFACE_ATTRIB_SETTABLE, value: int(va::VA_SURFACE_ATTRIB_MEM_TYPE_USER_PTR) },
            va::VASurfaceAttrib {
                type_: va::VASurfaceAttribExternalBufferDescriptor,
                flags: va::VA_SURFACE_ATTRIB_SETTABLE,
                value: va::VAGenericValue { type_: va::VAGenericValueTypePointer, value: va::_VAGenericValue__bindgen_ty_1 { p: (&mut ext as *mut va::VASurfaceAttribExternalBuffers).cast() } },
            },
        ];
        let mut surface = va::VA_INVALID_SURFACE;
        // SAFETY: the descriptor and its buffer list live through the
        // call; the memory outlives the surface (`end`).
        api.ok(unsafe { (api.create_surfaces)(d.dpy, rt, width, height, &mut surface, 1, attrs.as_mut_ptr(), attrs.len() as c_uint) }, "vaCreateSurfaces (user memory)")?;
        Ok(Linear { surface, pitch, uv_at, deep, host })
    }

    /// Nothing but its surface holds its memory: it can take a picture.
    fn free(&self) -> bool {
        Arc::strong_count(&self.host) == 1
    }

    /// `from`'s picture copied by the GPU into our memory, waited for.
    fn copy_in(&self, d: &Display, from: va::VASurfaceID) -> Result<(), String> {
        let api = &d.api;
        let copy = api.copy.ok_or("no vaCopy")?;
        let object = |surface: va::VASurfaceID| va::VACopyObject { obj_type: va::VACopyObjectSurface, object: va::_VACopyObject__bindgen_ty_1 { surface_id: surface }, ..Default::default() };
        let (mut to, mut from) = (object(self.surface), object(from));
        // Waiting for it, by the copy engine as fast as it goes.
        let option = va::VACopyOption { value: va::VA_EXEC_SYNC | (va::VA_EXEC_MODE_DEFAULT << 2) };
        // SAFETY: our surfaces, as big as each other.
        api.ok(unsafe { copy(d.dpy, &mut to, &mut from, option) }, "vaCopy")
    }

    /// The picture in our memory, `w` × `h`, as planes (rows packed).
    fn copy_out(&self, w: u32, h: u32) -> (Vec<u8>, Vec<u8>) {
        let (mut y, mut uv, row_y, row_uv) = planes(w, h, self.deep);
        // SAFETY: rows within our memory, as laid out in `new`; the GPU
        // is done with it (synced).
        unsafe {
            copy_rows(self.host.ptr, self.pitch, &mut y, row_y);
            copy_rows(self.host.ptr.add(self.uv_at), self.pitch, &mut uv, row_uv);
        }
        (y, uv)
    }

    /// What it made goes (its memory after the surface).
    fn end(self, d: &Display) {
        let mut surface = self.surface;
        // SAFETY: ours, once.
        unsafe { (d.api.destroy_surfaces)(d.dpy, &mut surface, 1) };
        drop(self.host);
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        if let Some(c) = self.current.take() {
            self.va.drop_buffers(&c.buffers);
        }
    }
}

/// Rows of `to.len() / row` from `from` (`pitch` apart) into `to`.
///
/// # Safety
/// `from` must hold that many rows of `pitch` bytes (the last at least
/// `row`).
unsafe fn copy_rows(from: *const u8, pitch: usize, to: &mut [u8], row: usize) {
    for (k, line) in to.chunks_exact_mut(row).enumerate() {
        // SAFETY: the caller's word.
        unsafe { std::ptr::copy_nonoverlapping(from.add(k * pitch), line.as_mut_ptr(), row) };
    }
}

/// VP9's colour space as H.273's matrix.
fn vp9_matrix(c: vp9::ColorSpace) -> u8 {
    match c {
        vp9::ColorSpace::Bt601 | vp9::ColorSpace::Smpte170 => 6,
        vp9::ColorSpace::Smpte240 => 7,
        vp9::ColorSpace::Bt2020 => 9,
        vp9::ColorSpace::Bt709 => 1,
        _ => 2,
    }
}

/// Bits `v` at `at`, `n` wide (a bitfield of a VA-API word).
fn bits(v: u32, at: u32, n: u32) -> u32 {
    (v & ((1 << n) - 1)) << at
}

fn vp9_picture(h: &vp9::Header, refs: [va::VASurfaceID; 8]) -> va::VADecPictureParameterBufferVP9 {
    let (last, golden, alt) = (h.ref_frame_idx[0] as u32, h.ref_frame_idx[1] as u32, h.ref_frame_idx[2] as u32);
    let sign = |i: usize| h.ref_frame_sign_bias[i] as u32;
    let fields = bits(h.subsampling_x as u32, 0, 1)
        | bits(h.subsampling_y as u32, 1, 1)
        | bits(h.frame_type as u32, 2, 1)
        | bits(h.show_frame as u32, 3, 1)
        | bits(h.error_resilient_mode as u32, 4, 1)
        | bits(h.intra_only as u32, 5, 1)
        | bits(h.allow_high_precision_mv as u32, 6, 1)
        | bits(h.interpolation_filter as u32, 7, 3)
        | bits(h.frame_parallel_decoding_mode as u32, 10, 1)
        | bits(h.reset_frame_context as u32, 11, 2)
        | bits(h.refresh_frame_context as u32, 13, 1)
        | bits(h.frame_context_idx as u32, 14, 2)
        | bits(h.seg.enabled as u32, 16, 1)
        | bits(h.seg.temporal_update as u32, 17, 1)
        | bits(h.seg.update_map as u32, 18, 1)
        | bits(last, 19, 3)
        | bits(sign(1), 22, 1)
        | bits(golden, 23, 3)
        | bits(sign(2), 26, 1)
        | bits(alt, 27, 3)
        | bits(sign(3), 30, 1)
        | bits(h.lossless as u32, 31, 1);
    let mut p = va::VADecPictureParameterBufferVP9 {
        frame_width: h.width as u16,
        frame_height: h.height as u16,
        reference_frames: refs,
        filter_level: h.lf.level,
        sharpness_level: h.lf.sharpness,
        log2_tile_rows: h.tile_rows_log2,
        log2_tile_columns: h.tile_cols_log2,
        frame_header_length_in_bytes: h.uncompressed_header_size_in_bytes as u8,
        first_partition_size: h.header_size_in_bytes,
        mb_segment_tree_probs: h.seg.tree_probs,
        segment_pred_probs: if h.seg.temporal_update { h.seg.pred_probs } else { [0xff; 3] },
        profile: h.profile as u8,
        bit_depth: h.bit_depth as u8,
        ..Default::default()
    };
    p.pic_fields.value = fields;
    p
}

fn vp9_slice(segments: &[vp9::Segmentation; vp9::MAX_SEGMENTS], size: usize) -> va::VASliceParameterBufferVP9 {
    let mut s = va::VASliceParameterBufferVP9 { slice_data_size: size as u32, slice_data_offset: 0, slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL, ..Default::default() };
    for (to, from) in s.seg_param.iter_mut().zip(segments) {
        to.segment_flags.value = (bits(from.reference_frame_enabled as u32, 0, 1) | bits(from.reference_frame as u32, 1, 2) | bits(from.reference_skip_enabled as u32, 3, 1)) as u16;
        to.filter_level = from.lvl_lookup;
        to.luma_ac_quant_scale = from.luma_ac_quant_scale;
        to.luma_dc_quant_scale = from.luma_dc_quant_scale;
        to.chroma_ac_quant_scale = from.chroma_ac_quant_scale;
        to.chroma_dc_quant_scale = from.chroma_dc_quant_scale;
    }
    s
}

fn av1_picture(
    h: &av1::FrameHeaderObu,
    seq: &av1::SequenceHeaderObu,
    current: va::VASurfaceID,
    display: va::VASurfaceID,
    refs: [va::VASurfaceID; 8],
) -> Result<va::VADecPictureParameterBufferAV1, String> {
    let cc = &seq.color_config;
    let b = |v: bool| v as u32;
    let mut p = va::VADecPictureParameterBufferAV1 {
        profile: seq.seq_profile as u8,
        order_hint_bits_minus_1: seq.order_hint_bits_minus_1 as u8,
        bit_depth_idx: match seq.bit_depth {
            av1::BitDepth::Depth8 => 0,
            av1::BitDepth::Depth10 => 1,
            av1::BitDepth::Depth12 => 2,
        },
        matrix_coefficients: cc.matrix_coefficients as u8,
        current_frame: current,
        current_display_picture: display,
        frame_width_minus1: (h.upscaled_width - 1) as u16,
        frame_height_minus1: (h.frame_height - 1) as u16,
        ref_frame_map: refs,
        primary_ref_frame: h.primary_ref_frame as u8,
        order_hint: h.order_hint as u8,
        tile_cols: h.tile_info.tile_cols as u8,
        tile_rows: h.tile_info.tile_rows as u8,
        context_update_tile_id: h.tile_info.context_update_tile_id as u16,
        superres_scale_denominator: h.superres_denom as u8,
        interp_filter: h.interpolation_filter as u8,
        ..Default::default()
    };
    p.seq_info_fields.value = bits(b(seq.still_picture), 0, 1)
        | bits(b(seq.use_128x128_superblock), 1, 1)
        | bits(b(seq.enable_filter_intra), 2, 1)
        | bits(b(seq.enable_intra_edge_filter), 3, 1)
        | bits(b(seq.enable_interintra_compound), 4, 1)
        | bits(b(seq.enable_masked_compound), 5, 1)
        | bits(b(seq.enable_dual_filter), 6, 1)
        | bits(b(seq.enable_order_hint), 7, 1)
        | bits(b(seq.enable_jnt_comp), 8, 1)
        | bits(b(seq.enable_cdef), 9, 1)
        | bits(b(cc.mono_chrome), 10, 1)
        | bits(b(cc.color_range), 11, 1)
        | bits(b(cc.subsampling_x), 12, 1)
        | bits(b(cc.subsampling_y), 13, 1)
        | bits(cc.chroma_sample_position as u32, 14, 1)
        | bits(b(seq.film_grain_params_present), 15, 1);
    for (i, r) in p.ref_frame_idx.iter_mut().enumerate() {
        *r = h.ref_frame_idx[i];
    }

    // Segmentation.
    let seg = &h.segmentation_params;
    p.seg_info.segment_info_fields.value =
        bits(b(seg.segmentation_enabled), 0, 1) | bits(b(seg.segmentation_update_map), 1, 1) | bits(b(seg.segmentation_temporal_update), 2, 1) | bits(b(seg.segmentation_update_data), 3, 1);
    for i in 0..av1::MAX_SEGMENTS {
        let mut mask = 0u8;
        for j in 0..av1::SEG_LVL_MAX {
            if seg.feature_enabled[i][j] {
                mask |= 1 << j;
            }
            p.seg_info.feature_data[i][j] = seg.feature_data[i][j];
        }
        p.seg_info.feature_mask[i] = mask;
    }

    // Film grain (applied by the driver into the display picture).
    let fg = &h.film_grain_params;
    let g = &mut p.film_grain_info;
    g.film_grain_info_fields.value = bits(b(fg.apply_grain), 0, 1)
        | bits(b(fg.chroma_scaling_from_luma), 1, 1)
        | bits(fg.grain_scaling_minus_8 as u32, 2, 2)
        | bits(fg.ar_coeff_lag, 4, 2)
        | bits(fg.ar_coeff_shift_minus_6 as u32, 6, 2)
        | bits(fg.grain_scale_shift as u32, 8, 2)
        | bits(b(fg.overlap_flag), 10, 1)
        | bits(b(fg.clip_to_restricted_range), 11, 1);
    g.grain_seed = fg.grain_seed;
    g.num_y_points = fg.num_y_points;
    g.point_y_value.copy_from_slice(&fg.point_y_value[..14]);
    g.point_y_scaling.copy_from_slice(&fg.point_y_scaling[..14]);
    g.num_cb_points = fg.num_cb_points;
    g.point_cb_value.copy_from_slice(&fg.point_cb_value[..10]);
    g.point_cb_scaling.copy_from_slice(&fg.point_cb_scaling[..10]);
    g.num_cr_points = fg.num_cr_points;
    g.point_cr_value.copy_from_slice(&fg.point_cr_value[..10]);
    g.point_cr_scaling.copy_from_slice(&fg.point_cr_scaling[..10]);
    for (to, from) in g.ar_coeffs_y.iter_mut().zip(fg.ar_coeffs_y_plus_128.iter()) {
        *to = (*from as i16 - 128) as i8;
    }
    for (to, from) in g.ar_coeffs_cb.iter_mut().zip(fg.ar_coeffs_cb_plus_128.iter()) {
        *to = (*from as i16 - 128) as i8;
    }
    for (to, from) in g.ar_coeffs_cr.iter_mut().zip(fg.ar_coeffs_cr_plus_128.iter()) {
        *to = (*from as i16 - 128) as i8;
    }
    g.cb_mult = fg.cb_mult;
    g.cb_luma_mult = fg.cb_luma_mult;
    g.cb_offset = fg.cb_offset;
    g.cr_mult = fg.cr_mult;
    g.cr_luma_mult = fg.cr_luma_mult;
    g.cr_offset = fg.cr_offset;

    // Tiles.
    let t = &h.tile_info;
    for (to, from) in p.width_in_sbs_minus_1.iter_mut().zip(t.width_in_sbs_minus_1.iter()) {
        *to = *from as u16;
    }
    for (to, from) in p.height_in_sbs_minus_1.iter_mut().zip(t.height_in_sbs_minus_1.iter()) {
        *to = *from as u16;
    }

    p.pic_info_fields.value = bits(h.frame_type as u32, 0, 2)
        | bits(b(h.show_frame), 2, 1)
        | bits(b(h.showable_frame), 3, 1)
        | bits(b(h.error_resilient_mode), 4, 1)
        | bits(b(h.disable_cdf_update), 5, 1)
        | bits(h.allow_screen_content_tools, 6, 1)
        | bits(h.force_integer_mv, 7, 1)
        | bits(b(h.allow_intrabc), 8, 1)
        | bits(b(h.use_superres), 9, 1)
        | bits(b(h.allow_high_precision_mv), 10, 1)
        | bits(b(h.is_motion_mode_switchable), 11, 1)
        | bits(b(h.use_ref_frame_mvs), 12, 1)
        | bits(b(h.disable_frame_end_update_cdf), 13, 1)
        | bits(b(t.uniform_tile_spacing_flag), 14, 1)
        | bits(b(h.allow_warped_motion), 15, 1);

    // Loop filter.
    let lf = &h.loop_filter_params;
    p.filter_level = [lf.loop_filter_level[0], lf.loop_filter_level[1]];
    p.filter_level_u = lf.loop_filter_level[2];
    p.filter_level_v = lf.loop_filter_level[3];
    p.loop_filter_info_fields.value = (bits(lf.loop_filter_sharpness as u32, 0, 3) | bits(b(lf.loop_filter_delta_enabled), 3, 1) | bits(b(lf.loop_filter_delta_update), 4, 1)) as u8;
    p.ref_deltas = lf.loop_filter_ref_deltas;
    p.mode_deltas = lf.loop_filter_mode_deltas;

    // Quantization.
    let q = &h.quantization_params;
    p.base_qindex = q.base_q_idx as u8;
    p.y_dc_delta_q = q.delta_q_y_dc as i8;
    p.u_dc_delta_q = q.delta_q_u_dc as i8;
    p.u_ac_delta_q = q.delta_q_u_ac as i8;
    p.v_dc_delta_q = q.delta_q_v_dc as i8;
    p.v_ac_delta_q = q.delta_q_v_ac as i8;
    p.qmatrix_fields.value = (bits(b(q.using_qmatrix), 0, 1) | bits(q.qm_y, 1, 4) | bits(q.qm_u, 5, 4) | bits(q.qm_v, 9, 4)) as u16;
    p.mode_control_fields.value = bits(b(q.delta_q_present), 0, 1)
        | bits(q.delta_q_res, 1, 2)
        | bits(b(lf.delta_lf_present), 3, 1)
        | bits(lf.delta_lf_res as u32, 4, 2)
        | bits(lf.delta_lf_multi as u32, 6, 1)
        | bits(h.tx_mode as u32, 7, 2)
        | bits(b(h.reference_select), 9, 1)
        | bits(b(h.reduced_tx_set), 10, 1)
        | bits(b(h.skip_mode_present), 11, 1);

    // CDEF: each strength's primary and secondary (3 meaning 4).
    let cdef = &h.cdef_params;
    p.cdef_damping_minus_3 = (cdef.cdef_damping - 3) as u8;
    p.cdef_bits = cdef.cdef_bits as u8;
    for i in 0..(1usize << cdef.cdef_bits) {
        let sec = |s: u32| if s == 4 { 3 } else { s & 3 };
        p.cdef_y_strengths[i] = (((cdef.cdef_y_pri_strength[i] & 0xf) << 2) | sec(cdef.cdef_y_sec_strength[i])) as u8;
        p.cdef_uv_strengths[i] = (((cdef.cdef_uv_pri_strength[i] & 0xf) << 2) | sec(cdef.cdef_uv_sec_strength[i])) as u8;
    }

    // Loop restoration.
    let lr = &h.loop_restoration_params;
    p.loop_restoration_fields.value = (bits(lr.frame_restoration_type[0] as u32, 0, 2)
        | bits(lr.frame_restoration_type[1] as u32, 2, 2)
        | bits(lr.frame_restoration_type[2] as u32, 4, 2)
        | bits(lr.lr_unit_shift as u32, 6, 2)
        | bits(lr.lr_uv_shift as u32, 8, 1)) as u16;

    // Global motion, for each reference after the intra frame.
    let gm = &h.global_motion_params;
    for (i, wm) in p.wm.iter_mut().enumerate() {
        let r = i + 1;
        wm.wmtype = gm.gm_type[r] as u32;
        for k in 0..6 {
            wm.wmmat[k] = gm.gm_params[r][k];
        }
        wm.invalid = u8::from(!gm.warp_valid[r]);
    }
    Ok(p)
}
