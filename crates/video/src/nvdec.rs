//! Video decoded by NVIDIA's GPU (NVDEC, through `libnvcuvid`): H.264,
//! HEVC, VP9 and AV1, 8- and 10-bit 4:2:0, for the Viewer's player when
//! the GPU decodes (`olo_preview::video`'s `Device`). Nothing is linked:
//! `libcuda.so.1` and `libnvcuvid.so.1` are opened at run time, as NVENC
//! is (`nvenc.rs`), so Olo builds and starts without them.
//!
//! NVIDIA's own parser reads the stream (Annex B for H.264 and HEVC, OBUs
//! for AV1, frames for VP9) and calls back: once the stream's format is
//! known, a decoder is made for it; each picture is decoded on the GPU;
//! each to show (in display order, with the time it was sent with) is
//! mapped and copied into memory as NV12 (8-bit) or P016 (16-bit, a
//! 10-bit sample in the high bits), the planes a window's GPU draws from
//! (`gpui::YuvFrame`). The callbacks run inside `cuvidParseVideoData`, on
//! the caller's thread, with the CUDA context current.

use std::{
    ffi::{c_int, c_void},
    ptr::null_mut,
};

use nvdec_sys::{cuviddec as dec, nvcuvid as parse};

use crate::nvenc::Lib;

/// What NVDEC decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

impl Codec {
    fn cuda(self) -> dec::cudaVideoCodec {
        match self {
            Codec::H264 => dec::cudaVideoCodec_enum::cudaVideoCodec_H264,
            Codec::Hevc => dec::cudaVideoCodec_enum::cudaVideoCodec_HEVC,
            Codec::Vp9 => dec::cudaVideoCodec_enum::cudaVideoCodec_VP9,
            Codec::Av1 => dec::cudaVideoCodec_enum::cudaVideoCodec_AV1,
        }
    }
}

/// A picture decoded: its planes as a GPU draws them (luma rows packed;
/// Cb and Cr interleaved, half as wide and high), 8-bit or 16-bit (the
/// sample in the high bits), with the time it was sent with (µs) and its
/// colour (H.273's matrix, its range).
pub struct Picture {
    pub pts: i64,
    pub width: u32,
    pub height: u32,
    pub deep: bool,
    pub y: Vec<u8>,
    pub uv: Vec<u8>,
    /// H.273's MatrixCoefficients (1: BT.709, 5 and 6: BT.601, 9: BT.2020;
    /// 2: not said).
    pub matrix: u8,
    pub full_range: bool,
}

type CuInit = unsafe extern "C" fn(u32) -> i32;
type CuDeviceGet = unsafe extern "C" fn(*mut i32, i32) -> i32;
type CuCtxCreate = unsafe extern "C" fn(*mut *mut c_void, u32, i32) -> i32;
type CuCtxDestroy = unsafe extern "C" fn(*mut c_void) -> i32;
type CuCtxPush = unsafe extern "C" fn(*mut c_void) -> i32;
type CuCtxPop = unsafe extern "C" fn(*mut *mut c_void) -> i32;
type CuMemcpy2D = unsafe extern "C" fn(*const Memcpy2D) -> i32;
type CreateParser = unsafe extern "C" fn(*mut parse::CUvideoparser, *mut parse::CUVIDPARSERPARAMS) -> i32;
type ParseData = unsafe extern "C" fn(parse::CUvideoparser, *mut parse::CUVIDSOURCEDATAPACKET) -> i32;
type DestroyParser = unsafe extern "C" fn(parse::CUvideoparser) -> i32;
type CreateDecoder = unsafe extern "C" fn(*mut dec::CUvideodecoder, *mut dec::CUVIDDECODECREATEINFO) -> i32;
type DestroyDecoder = unsafe extern "C" fn(dec::CUvideodecoder) -> i32;
type DecodePicture = unsafe extern "C" fn(dec::CUvideodecoder, *mut dec::CUVIDPICPARAMS) -> i32;
type MapFrame = unsafe extern "C" fn(dec::CUvideodecoder, c_int, *mut u64, *mut u32, *mut dec::CUVIDPROCPARAMS) -> i32;
type UnmapFrame = unsafe extern "C" fn(dec::CUvideodecoder, u64) -> i32;
type GetCaps = unsafe extern "C" fn(*mut dec::CUVIDDECODECAPS) -> i32;
type LockCreate = unsafe extern "C" fn(*mut dec::CUvideoctxlock, *mut c_void) -> i32;
type LockDestroy = unsafe extern "C" fn(dec::CUvideoctxlock) -> i32;

/// CUDA's `CUDA_MEMCPY2D`.
#[repr(C)]
struct Memcpy2D {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: u32,
    src_host: *const c_void,
    src_device: u64,
    src_array: *mut c_void,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: u32,
    dst_host: *mut c_void,
    dst_device: u64,
    dst_array: *mut c_void,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

const MEMORY_HOST: u32 = 1;
const MEMORY_DEVICE: u32 = 2;

struct Fns {
    cu_destroy: CuCtxDestroy,
    cu_push: CuCtxPush,
    cu_pop: CuCtxPop,
    memcpy: CuMemcpy2D,
    parse: ParseData,
    destroy_parser: DestroyParser,
    create_decoder: CreateDecoder,
    destroy_decoder: DestroyDecoder,
    decode: DecodePicture,
    map: MapFrame,
    unmap: UnmapFrame,
    caps: GetCaps,
    lock_destroy: LockDestroy,
}

/// What the callbacks share (boxed: the parser holds its address).
struct State {
    fns: Fns,
    codec: Codec,
    lock: dec::CUvideoctxlock,
    decoder: dec::CUvideodecoder,
    /// The decoder's output: its size (the display area), whether 16-bit,
    /// and the stream's colour.
    width: u32,
    height: u32,
    deep: bool,
    matrix: u8,
    full_range: bool,
    out: Vec<Picture>,
    error: Option<String>,
    /// The decoder's decode surfaces, and how many the parser was told
    /// it has (0: a new parser, not told yet).
    surfaces: u32,
    told: u32,
}

pub struct Nvdec {
    state: Box<State>,
    ctx: *mut c_void,
    parser: parse::CUvideoparser,
    create_parser: CreateParser,
    _libs: (Lib, Lib),
}

// SAFETY: the decoder is used from one thread at a time (the player's
// worker); CUDA's context is pushed for each call.
unsafe impl Send for Nvdec {}

/// Whether NVIDIA's decoder is here at all (its libraries; the GPU itself
/// is asked when one is made).
pub fn available() -> bool {
    Lib::open("libnvcuvid.so.1").is_ok() && Lib::open("libcuda.so.1").is_ok()
}

impl Nvdec {
    /// A decoder for `codec` on the first NVIDIA GPU, or why not.
    pub fn new(codec: Codec) -> Result<Nvdec, String> {
        let cuda = Lib::open("libcuda.so.1")?;
        let cuvid = Lib::open("libnvcuvid.so.1")?;
        // SAFETY: the libraries' own functions, called as declared; the
        // state's address stays put (boxed) while the parser has it.
        unsafe {
            let cu_init: CuInit = cuda.get("cuInit")?;
            let cu_device: CuDeviceGet = cuda.get("cuDeviceGet")?;
            let cu_create: CuCtxCreate = cuda.get("cuCtxCreate_v2")?;
            let lock_create: LockCreate = cuvid.get("cuvidCtxLockCreate")?;
            let fns = Fns {
                cu_destroy: cuda.get("cuCtxDestroy_v2")?,
                cu_push: cuda.get("cuCtxPushCurrent_v2")?,
                cu_pop: cuda.get("cuCtxPopCurrent_v2")?,
                memcpy: cuda.get("cuMemcpy2D_v2")?,
                parse: cuvid.get("cuvidParseVideoData")?,
                destroy_parser: cuvid.get("cuvidDestroyVideoParser")?,
                create_decoder: cuvid.get("cuvidCreateDecoder")?,
                destroy_decoder: cuvid.get("cuvidDestroyDecoder")?,
                decode: cuvid.get("cuvidDecodePicture")?,
                map: cuvid.get("cuvidMapVideoFrame64")?,
                unmap: cuvid.get("cuvidUnmapVideoFrame64")?,
                caps: cuvid.get("cuvidGetDecoderCaps")?,
                lock_destroy: cuvid.get("cuvidCtxLockDestroy")?,
            };
            let create_parser: CreateParser = cuvid.get("cuvidCreateVideoParser")?;
            if cu_init(0) != 0 {
                return Err("CUDA couldn't start (no NVIDIA GPU awake?)".into());
            }
            let mut device = 0i32;
            if cu_device(&mut device, 0) != 0 {
                return Err("CUDA found no GPU".into());
            }
            let mut ctx = null_mut();
            if cu_create(&mut ctx, 0, device) != 0 {
                return Err("CUDA couldn't make a context".into());
            }
            let mut lock: dec::CUvideoctxlock = null_mut();
            if lock_create(&mut lock, ctx) != 0 {
                (fns.cu_destroy)(ctx);
                return Err("NVDEC couldn't lock its context".into());
            }
            // (cuCtxCreate made it current here: let it go till it's used.)
            let mut was = null_mut();
            (fns.cu_pop)(&mut was);
            let state = Box::new(State { fns, codec, lock, decoder: null_mut(), width: 0, height: 0, deep: false, matrix: 2, full_range: false, out: Vec::new(), error: None, surfaces: 0, told: 0 });
            let mut me = Nvdec { state, ctx, parser: null_mut(), create_parser, _libs: (cuda, cuvid) };
            me.make_parser()?;
            Ok(me)
        }
    }

    /// A parser (again after a seek: what it held is let go).
    fn make_parser(&mut self) -> Result<(), String> {
        // SAFETY: as in `new`; the parameters live through the call.
        unsafe {
            if !self.parser.is_null() {
                (self.state.fns.destroy_parser)(self.parser);
                self.parser = null_mut();
            }
            self.state.told = 0;
            let mut p: parse::CUVIDPARSERPARAMS = std::mem::zeroed();
            p.CodecType = self.state.codec.cuda();
            p.ulMaxNumDecodeSurfaces = 1;
            // (A picture or two ahead: the decoder kept busy.)
            p.ulMaxDisplayDelay = 2;
            p.ulClockRate = 1_000_000;
            p.pUserData = (&mut *self.state as *mut State).cast();
            p.pfnSequenceCallback = Some(on_sequence);
            p.pfnDecodePicture = Some(on_decode);
            p.pfnDisplayPicture = Some(on_display);
            if (self.create_parser)(&mut self.parser, &mut p) != 0 {
                return Err("NVDEC couldn't make its parser".into());
            }
        }
        Ok(())
    }

    /// The context current on this thread while `f` runs.
    fn with_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        // SAFETY: our context, pushed and popped around the call.
        unsafe { (self.state.fns.cu_push)(self.ctx) };
        let r = f(self);
        let mut was = null_mut();
        // SAFETY: as above.
        unsafe { (self.state.fns.cu_pop)(&mut was) };
        r
    }

    fn send(&mut self, data: &[u8], pts: i64, flags: u64) -> Result<(), String> {
        self.with_context(|me| {
            let mut packet = parse::CUVIDSOURCEDATAPACKET { flags: flags as _, payload_size: data.len() as _, payload: data.as_ptr(), timestamp: pts };
            // SAFETY: the parser and packet are ours; it calls back into
            // the state, which it was given.
            let r = unsafe { (me.state.fns.parse)(me.parser, &mut packet) };
            match me.state.error.take() {
                Some(e) => Err(e),
                None if r != 0 => Err(format!("NVDEC couldn't parse the stream ({r})")),
                None => Ok(()),
            }
        })
    }

    /// One packet (`pts` in µs): the pictures it let come out.
    pub fn decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Picture>) -> Result<(), String> {
        let r = self.send(data, pts, parse::CUvideopacketflags::CUVID_PKT_TIMESTAMP as u64);
        out.append(&mut self.state.out);
        r
    }

    /// The end of the stream: what the decoder still held.
    pub fn flush(&mut self, out: &mut Vec<Picture>) {
        let _ = self.send(&[], 0, parse::CUvideopacketflags::CUVID_PKT_ENDOFSTREAM as u64);
        out.append(&mut self.state.out);
    }

    /// After a seek: nothing held, a stream starting again (its decoder
    /// kept if the format is the same).
    pub fn reset(&mut self) {
        self.state.out.clear();
        let _ = self.with_context(|me| me.make_parser());
    }
}

impl Drop for Nvdec {
    fn drop(&mut self) {
        // SAFETY: ours, let go in order (the parser, the decoder in its
        // context, the lock, the context).
        unsafe {
            (self.state.fns.cu_push)(self.ctx);
            if !self.parser.is_null() {
                (self.state.fns.destroy_parser)(self.parser);
            }
            if !self.state.decoder.is_null() {
                (self.state.fns.destroy_decoder)(self.state.decoder);
            }
            let mut was = null_mut();
            (self.state.fns.cu_pop)(&mut was);
            (self.state.fns.lock_destroy)(self.state.lock);
            (self.state.fns.cu_destroy)(self.ctx);
        }
    }
}

/// The stream's format, known: a decoder made for it (or the one there
/// kept, if it's the same); how many pictures it decodes into.
unsafe extern "C" fn on_sequence(user: *mut c_void, format: *mut parse::CUVIDEOFORMAT) -> c_int {
    // SAFETY: the parser gives back the state it was given, and a format.
    let (s, f) = unsafe { (&mut *user.cast::<State>(), &*format) };
    let deep = f.bit_depth_luma_minus8 > 0;
    let area = &f.display_area;
    let (w, h) = ((area.right - area.left).max(2) as u32, (area.bottom - area.top).max(2) as u32);
    // The same sequence again (its parameter sets sent before each key
    // frame): nothing to do, and nothing the parser should change (1),
    // once it has been told its surfaces. Asking the GPU again each time
    // stalled each key frame's pictures.
    if !s.decoder.is_null() && (s.width, s.height, s.deep) == (w, h, deep) {
        if s.told != 0 {
            return 1;
        }
        s.told = s.surfaces;
        return s.told as c_int;
    }
    if f.chroma_format != dec::cudaVideoChromaFormat_enum::cudaVideoChromaFormat_420 {
        s.error = Some("NVDEC here decodes 4:2:0 only".into());
        return 0;
    }
    // SAFETY: zeroed plain data, filled by the call.
    let mut caps: dec::CUVIDDECODECAPS = unsafe { std::mem::zeroed() };
    caps.eCodecType = f.codec;
    caps.eChromaFormat = f.chroma_format;
    caps.nBitDepthMinus8 = u32::from(f.bit_depth_luma_minus8);
    // SAFETY: as declared.
    if unsafe { (s.fns.caps)(&mut caps) } != 0 || caps.bIsSupported == 0 || f.coded_width > caps.nMaxWidth || f.coded_height > caps.nMaxHeight {
        s.error = Some(format!("this GPU's NVDEC doesn't decode {:?} {}×{} at {} bits", s.codec, f.coded_width, f.coded_height, 8 + f.bit_depth_luma_minus8));
        return 0;
    }
    // (Nor pictures smaller than it takes: VP9's 128 × 128 here.)
    if f.coded_width < u32::from(caps.nMinWidth) || f.coded_height < u32::from(caps.nMinHeight) {
        s.error = Some(format!("{}×{} is smaller than this GPU's NVDEC decodes {:?} ({}×{} at least)", f.coded_width, f.coded_height, s.codec, caps.nMinWidth, caps.nMinHeight));
        return 0;
    }
    let surfaces = u32::from(f.min_num_decode_surfaces).max(1) + 4;
    let vsd = &f.video_signal_description;
    // (video_format: 3 bits, then video_full_range_flag.)
    let full_range = vsd.video_full_range_flag() != 0;
    let matrix = vsd.matrix_coefficients;
    if !s.decoder.is_null() {
        // SAFETY: ours, made for another format.
        unsafe { (s.fns.destroy_decoder)(s.decoder) };
        s.decoder = null_mut();
    }
    // SAFETY: zeroed plain data, filled in; the call makes the decoder.
    let mut info: dec::CUVIDDECODECREATEINFO = unsafe { std::mem::zeroed() };
    info.ulWidth = f.coded_width as _;
    info.ulHeight = f.coded_height as _;
    info.ulMaxWidth = f.coded_width as _;
    info.ulMaxHeight = f.coded_height as _;
    info.ulNumDecodeSurfaces = surfaces as _;
    info.CodecType = f.codec;
    info.ChromaFormat = f.chroma_format;
    info.ulCreationFlags = dec::cudaVideoCreateFlags_enum::cudaVideoCreate_PreferCUVID as _;
    info.bitDepthMinus8 = u64::from(f.bit_depth_luma_minus8) as _;
    info.OutputFormat = if deep { dec::cudaVideoSurfaceFormat_enum::cudaVideoSurfaceFormat_P016 } else { dec::cudaVideoSurfaceFormat_enum::cudaVideoSurfaceFormat_NV12 };
    info.DeinterlaceMode =
        if f.progressive_sequence != 0 { dec::cudaVideoDeinterlaceMode_enum::cudaVideoDeinterlaceMode_Weave } else { dec::cudaVideoDeinterlaceMode_enum::cudaVideoDeinterlaceMode_Adaptive };
    info.display_area.left = area.left as i16;
    info.display_area.top = area.top as i16;
    info.display_area.right = area.right as i16;
    info.display_area.bottom = area.bottom as i16;
    info.ulTargetWidth = w as _;
    info.ulTargetHeight = h as _;
    info.ulNumOutputSurfaces = 2;
    info.vidLock = s.lock;
    // SAFETY: as declared.
    if unsafe { (s.fns.create_decoder)(&mut s.decoder, &mut info) } != 0 {
        s.decoder = null_mut();
        s.error = Some("NVDEC couldn't make a decoder for this stream".into());
        return 0;
    }
    (s.width, s.height, s.deep, s.matrix, s.full_range) = (w, h, deep, matrix, full_range);
    (s.surfaces, s.told) = (surfaces, surfaces);
    surfaces as c_int
}

unsafe extern "C" fn on_decode(user: *mut c_void, pic: *mut dec::CUVIDPICPARAMS) -> c_int {
    // SAFETY: the state given, and the picture's parameters.
    let s = unsafe { &mut *user.cast::<State>() };
    if s.decoder.is_null() {
        return 0;
    }
    // SAFETY: as declared.
    if unsafe { (s.fns.decode)(s.decoder, pic) } != 0 {
        s.error = Some("NVDEC couldn't decode a picture".into());
        return 0;
    }
    1
}

/// A picture to show: mapped, its planes copied into memory.
unsafe extern "C" fn on_display(user: *mut c_void, info: *mut parse::CUVIDPARSERDISPINFO) -> c_int {
    // SAFETY: the state given; a null `info` is the end of the stream.
    let s = unsafe { &mut *user.cast::<State>() };
    let Some(info) = (unsafe { info.as_ref() }) else { return 1 };
    if s.decoder.is_null() {
        return 0;
    }
    // SAFETY: zeroed plain data, filled in.
    let mut proc_: dec::CUVIDPROCPARAMS = unsafe { std::mem::zeroed() };
    proc_.progressive_frame = info.progressive_frame;
    proc_.top_field_first = info.top_field_first;
    proc_.second_field = info.repeat_first_field + 1;
    let (mut ptr, mut pitch) = (0u64, 0u32);
    // SAFETY: as declared.
    if unsafe { (s.fns.map)(s.decoder, info.picture_index, &mut ptr, &mut pitch, &mut proc_) } != 0 {
        s.error = Some("NVDEC couldn't map a picture".into());
        return 0;
    }
    let bytes = if s.deep { 2 } else { 1 };
    let (w, h) = (s.width as usize, s.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y = vec![0u8; w * h * bytes];
    let mut uv = vec![0u8; cw * ch * 2 * bytes];
    let copy = |src: u64, dst: &mut [u8], row: usize, rows: usize| {
        let m = Memcpy2D {
            src_x_in_bytes: 0,
            src_y: 0,
            src_memory_type: MEMORY_DEVICE,
            src_host: std::ptr::null(),
            src_device: src,
            src_array: null_mut(),
            src_pitch: pitch as usize,
            dst_x_in_bytes: 0,
            dst_y: 0,
            dst_memory_type: MEMORY_HOST,
            dst_host: dst.as_mut_ptr().cast(),
            dst_device: 0,
            dst_array: null_mut(),
            dst_pitch: row,
            width_in_bytes: row,
            height: rows,
        };
        // SAFETY: the mapped surface's rows into ours, `rows` of `row`.
        unsafe { (s.fns.memcpy)(&m) }
    };
    // (The chroma plane after the luma's rows, rounded up to even.)
    let chroma_at = ptr + u64::from(pitch) * ((h as u64 + 1) & !1);
    let ok = copy(ptr, &mut y, w * bytes, h) == 0 && copy(chroma_at, &mut uv, cw * 2 * bytes, ch) == 0;
    // SAFETY: mapped above.
    unsafe { (s.fns.unmap)(s.decoder, ptr) };
    if !ok {
        s.error = Some("NVDEC couldn't copy a picture".into());
        return 0;
    }
    s.out.push(Picture { pts: info.timestamp, width: s.width, height: s.height, deep: s.deep, y, uv, matrix: s.matrix, full_range: s.full_range });
    1
}
