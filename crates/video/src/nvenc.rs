//! NVIDIA's hardware encoder (NVENC), for when the computer has an NVIDIA
//! GPU: H.264 as [`crate::H264`] makes it, at a fraction of the CPU (4K
//! in a few milliseconds a frame rather than over a hundred).
//!
//! Nothing of NVIDIA's is linked: `libnvidia-encode.so.1` and
//! `libcuda.so.1` are opened at run time (`dlopen`), so Olo builds and
//! starts where there is no NVIDIA driver, and [`Nvenc::new`] says why
//! it can't be used there. Its declarations are vendor/nvenc-sys's.
//!
//! The encoder works on a CUDA context of its own, from pictures in
//! memory (BGRA rows copied into NVENC's input buffer), synchronously:
//! one picture in, its bitstream out, no B-frames (so an MP4 needs no
//! reordering), its parameter sets with every key frame.
//!
//! Its structures are filled from zeroed memory through raw pointers and
//! only read as Rust values once every field is a valid one (an enum
//! whose zero isn't one of its values is set first, or the driver fills
//! it).

use std::{
    ffi::{CStr, CString, c_void},
    mem::MaybeUninit,
    ptr::{addr_of_mut, null_mut},
};

use nvenc_sys as nv;

/// A library opened at run time, closed when dropped (NVIDIA's here,
/// libfdk-aac's in `aac`).
pub(crate) struct Lib(*mut c_void);

impl Lib {
    pub(crate) fn open(name: &str) -> Result<Lib, String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: dlopen with a NUL-ended name.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            // SAFETY: dlerror's text, while it's valid.
            let why = unsafe { libc::dlerror() };
            let why = if why.is_null() { "not found".into() } else { unsafe { CStr::from_ptr(why) }.to_string_lossy().into_owned() };
            return Err(format!("{name}: {why}"));
        }
        Ok(Lib(h))
    }

    /// The function `name`, as `T` (a function pointer type).
    ///
    /// # Safety
    /// `T` must be the function's real type.
    pub(crate) unsafe fn get<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: dlsym on our handle.
        let p = unsafe { libc::dlsym(self.0, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("{name} isn't there"));
        }
        // SAFETY: the caller's word that it is a `T`.
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

impl Drop for Lib {
    fn drop(&mut self) {
        // SAFETY: our handle, once.
        unsafe { libc::dlclose(self.0) };
    }
}

type CuInit = unsafe extern "C" fn(u32) -> i32;
type CuDeviceGet = unsafe extern "C" fn(*mut i32, i32) -> i32;
type CuCtxCreate = unsafe extern "C" fn(*mut *mut c_void, u32, i32) -> i32;
type CuCtxDestroy = unsafe extern "C" fn(*mut c_void) -> i32;
type CreateInstance = unsafe extern "C" fn(*mut nv::NV_ENCODE_API_FUNCTION_LIST) -> nv::NVENCSTATUS;

/// Whether NVIDIA's encoder can be opened here (its libraries are).
pub fn available() -> bool {
    Lib::open("libnvidia-encode.so.1").is_ok() && Lib::open("libcuda.so.1").is_ok()
}

/// How many bits pictures get.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rate {
    /// About this many bits a second (at most twice it).
    Bitrate(u32),
    /// As many as keep this quality (NVENC's constant-quality VBR: its
    /// `targetQuality`, lower better), at most `cap` bits a second.
    Quality(u8, u32),
}

/// An NVENC encoder (H.264, HEVC or AV1) for pictures of one size.
pub struct Nvenc {
    codec: crate::Codec,
    api: Box<nv::NV_ENCODE_API_FUNCTION_LIST>,
    encoder: *mut c_void,
    cuda: *mut c_void,
    cu_destroy: CuCtxDestroy,
    input: nv::NV_ENC_INPUT_PTR,
    output: nv::NV_ENC_OUTPUT_PTR,
    width: u32,
    height: u32,
    frame: u64,
    // (Kept open while the encoder is: last, so dropped after it.)
    _libs: (Lib, Lib),
}

// SAFETY: the session and its buffers are used from one thread at a time
// (the recorder's encoder), as NVENC allows.
unsafe impl Send for Nvenc {}

fn ok(status: nv::NVENCSTATUS, what: &str) -> Result<(), String> {
    if status == nv::NVENCSTATUS::NV_ENC_SUCCESS { Ok(()) } else { Err(format!("NVENC: {what}: {status:?}")) }
}

/// A function of the table, or why not.
fn call<T>(f: Option<T>, name: &str) -> Result<T, String> {
    f.ok_or_else(|| format!("NVENC has no {name}"))
}

impl Nvenc {
    /// An encoder for `width` × `height` (even) pictures at up to `fps` a
    /// second, at about `bps` bits a second (screen content: a little
    /// of it changes, and stays sharp).
    pub fn new(width: u32, height: u32, fps: u32, bps: u32, codec: crate::Codec) -> Result<Nvenc, String> {
        Nvenc::with_rate(width, height, fps, Rate::Bitrate(bps), codec)
    }

    /// [`Nvenc::new`] at a rate of `rate`'s kind.
    pub fn with_rate(width: u32, height: u32, fps: u32, rate: Rate, codec: crate::Codec) -> Result<Nvenc, String> {
        let nvenc = Lib::open("libnvidia-encode.so.1")?;
        let cuda = Lib::open("libcuda.so.1")?;
        // SAFETY: the functions' types are CUDA's driver API and NVENC's.
        unsafe {
            let cu_init: CuInit = cuda.get("cuInit")?;
            let cu_device: CuDeviceGet = cuda.get("cuDeviceGet")?;
            let cu_create: CuCtxCreate = cuda.get("cuCtxCreate_v2")?;
            let cu_destroy: CuCtxDestroy = cuda.get("cuCtxDestroy_v2")?;
            let create: CreateInstance = nvenc.get("NvEncodeAPICreateInstance")?;
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
            // From here on a failure cleans up through Drop.
            let mut api: Box<nv::NV_ENCODE_API_FUNCTION_LIST> = Box::new(std::mem::zeroed());
            api.version = nv::NV_ENCODE_API_FUNCTION_LIST_VER;
            let mut me = Nvenc { codec, api, encoder: null_mut(), cuda: ctx, cu_destroy, input: null_mut(), output: null_mut(), width, height, frame: 0, _libs: (nvenc, cuda) };
            ok(create(&mut *me.api), "NvEncodeAPICreateInstance")?;
            me.open(fps, rate)?;
            Ok(me)
        }
    }

    unsafe fn open(&mut self, fps: u32, rate: Rate) -> Result<(), String> {
        // HEVC on the slower P6 up to 2560 × 1600: 30% smaller for the
        // same picture than P4 (screen content), which it stays on above
        // (P6 took 29 ms a 4K frame, P4 14). H.264 and AV1 gained nothing
        // from slower presets. (Measured, `codec_sizes`, 2026-11-05.)
        let preset_guid = if self.codec == crate::Codec::Hevc && u64::from(self.width) * u64::from(self.height) <= 2560 * 1600 { nv::NV_ENC_PRESET_P6_GUID } else { nv::NV_ENC_PRESET_P4_GUID };
        let guid = match self.codec {
            crate::Codec::H264 => nv::NV_ENC_CODEC_H264_GUID,
            crate::Codec::Hevc => nv::NV_ENC_CODEC_HEVC_GUID,
            crate::Codec::Av1 => nv::NV_ENC_CODEC_AV1_GUID,
        };
        // SAFETY: each structure zeroed (valid where zero is), versioned,
        // and handed to the driver with our session.
        unsafe {
            let mut session: nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
            session.version = nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
            session.deviceType = nv::NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_CUDA;
            session.device = self.cuda;
            session.apiVersion = nv::NVENCAPI_VERSION;
            ok(call(self.api.nvEncOpenEncodeSessionEx, "OpenEncodeSessionEx")?(&mut session, &mut self.encoder), "opening a session")?;
            // The preset's settings, filled by the driver (every field then
            // a valid one), then made ours.
            let mut preset = Box::new(MaybeUninit::<nv::NV_ENC_PRESET_CONFIG>::zeroed());
            let p = preset.as_mut_ptr();
            addr_of_mut!((*p).version).write(nv::NV_ENC_PRESET_CONFIG_VER);
            addr_of_mut!((*p).presetCfg.version).write(nv::NV_ENC_CONFIG_VER);
            ok(
                call(self.api.nvEncGetEncodePresetConfigEx, "GetEncodePresetConfigEx")?(self.encoder, guid, preset_guid, nv::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_HIGH_QUALITY, p),
                match self.codec {
                    crate::Codec::H264 => "the preset's settings",
                    crate::Codec::Hevc => "the preset's settings (HEVC: can this GPU encode it?)",
                    crate::Codec::Av1 => "the preset's settings (AV1: can this GPU encode it? NVIDIA's from Ada on)",
                },
            )?;
            let preset = preset.assume_init_mut();
            let cfg = &mut preset.presetCfg;
            // A key frame every two seconds, no B-frames, the parameter
            // sets (AV1: the sequence header) with each key frame.
            cfg.gopLength = fps * 2;
            cfg.frameIntervalP = 1;
            match self.codec {
                crate::Codec::H264 => {
                    cfg.encodeCodecConfig.h264Config.idrPeriod = fps * 2;
                    cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1);
                }
                crate::Codec::Hevc => {
                    cfg.encodeCodecConfig.hevcConfig.idrPeriod = fps * 2;
                    cfg.encodeCodecConfig.hevcConfig.set_repeatSPSPPS(1);
                }
                crate::Codec::Av1 => {
                    let av1 = &mut cfg.encodeCodecConfig.av1Config;
                    av1.idrPeriod = fps * 2;
                    av1.set_repeatSeqHdr(1);
                    // OBUs with their sizes (MP4's form), 8-bit 4:2:0.
                    av1.set_outputAnnexBFormat(0);
                    av1.set_chromaFormatIDC(1);
                }
            }
            // A picture out for each picture in: no lookahead holding
            // pictures back (the slowest presets turn it on).
            cfg.rcParams.set_enableLookahead(0);
            cfg.rcParams.lookaheadDepth = 0;
            cfg.rcParams.rateControlMode = nv::NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
            match rate {
                Rate::Bitrate(bps) => {
                    cfg.rcParams.averageBitRate = bps;
                    cfg.rcParams.maxBitRate = bps.saturating_mul(2);
                }
                Rate::Quality(q, cap) => {
                    cfg.rcParams.averageBitRate = 0;
                    cfg.rcParams.maxBitRate = cap;
                    cfg.rcParams.targetQuality = q;
                }
            }
            let mut init: nv::NV_ENC_INITIALIZE_PARAMS = std::mem::zeroed();
            init.version = nv::NV_ENC_INITIALIZE_PARAMS_VER;
            init.encodeGUID = guid;
            init.presetGUID = preset_guid;
            init.tuningInfo = nv::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_HIGH_QUALITY;
            init.encodeWidth = self.width;
            init.encodeHeight = self.height;
            init.darWidth = self.width;
            init.darHeight = self.height;
            init.maxEncodeWidth = self.width;
            init.maxEncodeHeight = self.height;
            init.frameRateNum = fps;
            init.frameRateDen = 1;
            init.enablePTD = 1;
            init.encodeConfig = cfg;
            ok(call(self.api.nvEncInitializeEncoder, "InitializeEncoder")?(self.encoder, &mut init), "starting the encoder")?;
            let mut input: nv::NV_ENC_CREATE_INPUT_BUFFER = std::mem::zeroed();
            input.version = nv::NV_ENC_CREATE_INPUT_BUFFER_VER;
            input.width = self.width;
            input.height = self.height;
            input.bufferFmt = nv::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB;
            ok(call(self.api.nvEncCreateInputBuffer, "CreateInputBuffer")?(self.encoder, &mut input), "its input buffer")?;
            self.input = input.inputBuffer;
            let mut output: nv::NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
            output.version = nv::NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
            ok(call(self.api.nvEncCreateBitstreamBuffer, "CreateBitstreamBuffer")?(self.encoder, &mut output), "its output buffer")?;
            self.output = output.bitstreamBuffer;
        }
        Ok(())
    }

    /// One picture (BGRA, `stride` bytes a row) encoded: its bytes (H.264
    /// and HEVC in Annex B, AV1 as OBUs), and whether it's a key frame.
    pub fn encode(&mut self, data: &[u8], stride: usize) -> Result<(Vec<u8>, bool), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let row = w * 4;
        if stride < row || data.len() < stride * (h - 1) + row {
            return Err("the picture is smaller than the video".into());
        }
        // SAFETY: NVENC's buffers, locked while written and read.
        unsafe {
            let mut lock: nv::NV_ENC_LOCK_INPUT_BUFFER = std::mem::zeroed();
            lock.version = nv::NV_ENC_LOCK_INPUT_BUFFER_VER;
            lock.inputBuffer = self.input;
            ok(call(self.api.nvEncLockInputBuffer, "LockInputBuffer")?(self.encoder, &mut lock), "locking the input")?;
            let pitch = lock.pitch as usize;
            let to = lock.bufferDataPtr.cast::<u8>();
            for y in 0..h {
                std::ptr::copy_nonoverlapping(data.as_ptr().add(y * stride), to.add(y * pitch), row);
            }
            ok(call(self.api.nvEncUnlockInputBuffer, "UnlockInputBuffer")?(self.encoder, self.input), "unlocking the input")?;
            let mut pic = MaybeUninit::<nv::NV_ENC_PIC_PARAMS>::zeroed();
            let p = pic.as_mut_ptr();
            addr_of_mut!((*p).version).write(nv::NV_ENC_PIC_PARAMS_VER);
            addr_of_mut!((*p).inputWidth).write(self.width);
            addr_of_mut!((*p).inputHeight).write(self.height);
            addr_of_mut!((*p).inputPitch).write(pitch as u32);
            addr_of_mut!((*p).inputBuffer).write(self.input);
            addr_of_mut!((*p).outputBitstream).write(self.output);
            addr_of_mut!((*p).bufferFmt).write(nv::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB);
            addr_of_mut!((*p).pictureStruct).write(nv::NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME);
            addr_of_mut!((*p).inputTimeStamp).write(self.frame);
            addr_of_mut!((*p).frameIdx).write(self.frame as u32);
            ok(call(self.api.nvEncEncodePicture, "EncodePicture")?(self.encoder, p), "encoding")?;
            self.frame += 1;
            let mut out = MaybeUninit::<nv::NV_ENC_LOCK_BITSTREAM>::zeroed();
            let o = out.as_mut_ptr();
            addr_of_mut!((*o).version).write(nv::NV_ENC_LOCK_BITSTREAM_VER);
            addr_of_mut!((*o).outputBitstream).write(self.output);
            addr_of_mut!((*o).pictureStruct).write(nv::NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME);
            ok(call(self.api.nvEncLockBitstream, "LockBitstream")?(self.encoder, o), "reading the bitstream")?;
            let size = addr_of_mut!((*o).bitstreamSizeInBytes).read() as usize;
            let bytes = addr_of_mut!((*o).bitstreamBufferPtr).read().cast::<u8>();
            let kind = addr_of_mut!((*o).pictureType).read();
            let annex_b = std::slice::from_raw_parts(bytes, size).to_vec();
            ok(call(self.api.nvEncUnlockBitstream, "UnlockBitstream")?(self.encoder, self.output), "unlocking the bitstream")?;
            let key = matches!(kind, nv::NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_IDR | nv::NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_I);
            Ok((annex_b, key))
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn codec(&self) -> crate::Codec {
        self.codec
    }
}

impl Drop for Nvenc {
    fn drop(&mut self) {
        // SAFETY: what we made, each once, the session last, then CUDA's.
        unsafe {
            if !self.encoder.is_null() {
                if !self.input.is_null()
                    && let Some(f) = self.api.nvEncDestroyInputBuffer
                {
                    f(self.encoder, self.input);
                }
                if !self.output.is_null()
                    && let Some(f) = self.api.nvEncDestroyBitstreamBuffer
                {
                    f(self.encoder, self.output);
                }
                if let Some(f) = self.api.nvEncDestroyEncoder {
                    f(self.encoder);
                }
            }
            if !self.cuda.is_null() {
                (self.cu_destroy)(self.cuda);
            }
        }
    }
}
