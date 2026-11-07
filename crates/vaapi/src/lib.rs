//! AV1 through VA-API, for Intel's and AMD's GPUs (decoding AV1 and VP9
//! is [`decode`]'s): what Fedora's own drivers encode (iHD and Mesa leave out the patented codecs, so no
//! H.264 or HEVC there, but AV1 profile 0 on Intel's Xe and Arc GPUs and
//! AMD's from RDNA3 on). The ThinkPad's Intel GPU records the screen
//! this way while the NVIDIA one sleeps.
//!
//! Nothing of libva's is linked: `libva.so.2` and `libva-drm.so.2` are
//! opened at run time, as NVENC's libraries are (`nvenc::Lib`); the
//! declarations are vendor/va-sys's.
//!
//! The encoder works as FFmpeg's `av1_vaapi` does, at a constant quality
//! (CQP): a key frame then P frames, each predicted from the one before
//! (no B frames, so an MP4 needs no reordering), on two reconstructed
//! surfaces taken in turn. VA-API leaves AV1's headers to us: the
//! sequence header (with every key frame) and each frame header are
//! written here (`Writer`) and handed over as packed headers, and the
//! driver writes the tile groups after them; with a constant quality the
//! driver patches nothing in them, so they say exactly what it was told
//! (no loop filter, CDEF or restoration; the transform size chosen per
//! block where it can). Pictures come as BGRA rows and go in as NV12
//! (BT.709, limited range, as the sequence header says), converted on a
//! few threads straight into the input surface's memory.

use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    ptr::null_mut,
};

use va_sys as va;

pub mod decode;

/// A library opened at run time, closed when dropped.
struct Lib(*mut c_void);

impl Lib {
    fn open(name: &str) -> Result<Lib, String> {
        let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
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
    unsafe fn get<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
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

/// AV1's quantizer for every frame (0–255, lower is finer): text on a
/// screen kept as the other codecs keep it (`codec_sizes`).
pub const QUALITY: u8 = 40;

type Dpy = va::VADisplay;
type St = va::VAStatus;

/// libva's functions, opened at run time.
struct Api {
    get_display_drm: unsafe extern "C" fn(c_int) -> Dpy,
    initialize: unsafe extern "C" fn(Dpy, *mut c_int, *mut c_int) -> St,
    terminate: unsafe extern "C" fn(Dpy) -> St,
    error_str: unsafe extern "C" fn(St) -> *const c_char,
    set_info_callback: unsafe extern "C" fn(Dpy, Option<unsafe extern "C" fn(*mut c_void, *const c_char)>, *mut c_void) -> *mut c_void,
    query_vendor_string: unsafe extern "C" fn(Dpy) -> *const c_char,
    max_num_entrypoints: unsafe extern "C" fn(Dpy) -> c_int,
    query_config_entrypoints: unsafe extern "C" fn(Dpy, va::VAProfile, *mut va::VAEntrypoint, *mut c_int) -> St,
    get_config_attributes: unsafe extern "C" fn(Dpy, va::VAProfile, va::VAEntrypoint, *mut va::VAConfigAttrib, c_int) -> St,
    create_config: unsafe extern "C" fn(Dpy, va::VAProfile, va::VAEntrypoint, *mut va::VAConfigAttrib, c_int, *mut va::VAConfigID) -> St,
    destroy_config: unsafe extern "C" fn(Dpy, va::VAConfigID) -> St,
    create_surfaces: unsafe extern "C" fn(Dpy, c_uint, c_uint, c_uint, *mut va::VASurfaceID, c_uint, *mut va::VASurfaceAttrib, c_uint) -> St,
    destroy_surfaces: unsafe extern "C" fn(Dpy, *mut va::VASurfaceID, c_int) -> St,
    create_context: unsafe extern "C" fn(Dpy, va::VAConfigID, c_int, c_int, c_int, *mut va::VASurfaceID, c_int, *mut va::VAContextID) -> St,
    destroy_context: unsafe extern "C" fn(Dpy, va::VAContextID) -> St,
    create_buffer: unsafe extern "C" fn(Dpy, va::VAContextID, va::VABufferType, c_uint, c_uint, *mut c_void, *mut va::VABufferID) -> St,
    destroy_buffer: unsafe extern "C" fn(Dpy, va::VABufferID) -> St,
    map_buffer: unsafe extern "C" fn(Dpy, va::VABufferID, *mut *mut c_void) -> St,
    unmap_buffer: unsafe extern "C" fn(Dpy, va::VABufferID) -> St,
    begin_picture: unsafe extern "C" fn(Dpy, va::VAContextID, va::VASurfaceID) -> St,
    render_picture: unsafe extern "C" fn(Dpy, va::VAContextID, *mut va::VABufferID, c_int) -> St,
    end_picture: unsafe extern "C" fn(Dpy, va::VAContextID) -> St,
    sync_surface: unsafe extern "C" fn(Dpy, va::VASurfaceID) -> St,
    derive_image: unsafe extern "C" fn(Dpy, va::VASurfaceID, *mut va::VAImage) -> St,
    create_image: unsafe extern "C" fn(Dpy, *mut va::VAImageFormat, c_int, c_int, *mut va::VAImage) -> St,
    put_image: unsafe extern "C" fn(Dpy, va::VASurfaceID, va::VAImageID, c_int, c_int, c_uint, c_uint, c_int, c_int, c_uint, c_uint) -> St,
    destroy_image: unsafe extern "C" fn(Dpy, va::VAImageID) -> St,
    /// Copies a surface as it is (libva 2.12 on; the driver's copy
    /// engine).
    copy: Option<unsafe extern "C" fn(Dpy, *mut va::VACopyObject, *mut va::VACopyObject, va::VACopyOption) -> St>,
    // (Kept open while the functions are used.)
    _va: Lib,
    _drm: Lib,
}

impl Api {
    fn open() -> Result<Api, String> {
        let lib = Lib::open("libva.so.2")?;
        let drm = Lib::open("libva-drm.so.2")?;
        // SAFETY: each named function's type as libva 2.24's headers
        // declare it.
        unsafe {
            Ok(Api {
                get_display_drm: drm.get("vaGetDisplayDRM")?,
                initialize: lib.get("vaInitialize")?,
                terminate: lib.get("vaTerminate")?,
                error_str: lib.get("vaErrorStr")?,
                set_info_callback: lib.get("vaSetInfoCallback")?,
                query_vendor_string: lib.get("vaQueryVendorString")?,
                max_num_entrypoints: lib.get("vaMaxNumEntrypoints")?,
                query_config_entrypoints: lib.get("vaQueryConfigEntrypoints")?,
                get_config_attributes: lib.get("vaGetConfigAttributes")?,
                create_config: lib.get("vaCreateConfig")?,
                destroy_config: lib.get("vaDestroyConfig")?,
                create_surfaces: lib.get("vaCreateSurfaces")?,
                destroy_surfaces: lib.get("vaDestroySurfaces")?,
                create_context: lib.get("vaCreateContext")?,
                destroy_context: lib.get("vaDestroyContext")?,
                create_buffer: lib.get("vaCreateBuffer")?,
                destroy_buffer: lib.get("vaDestroyBuffer")?,
                map_buffer: lib.get("vaMapBuffer")?,
                unmap_buffer: lib.get("vaUnmapBuffer")?,
                begin_picture: lib.get("vaBeginPicture")?,
                render_picture: lib.get("vaRenderPicture")?,
                end_picture: lib.get("vaEndPicture")?,
                sync_surface: lib.get("vaSyncSurface")?,
                derive_image: lib.get("vaDeriveImage")?,
                create_image: lib.get("vaCreateImage")?,
                put_image: lib.get("vaPutImage")?,
                destroy_image: lib.get("vaDestroyImage")?,
                copy: lib.get("vaCopy").ok(),
                _va: lib,
                _drm: drm,
            })
        }
    }

    /// `st` as a result, its error said with what was being done.
    fn ok(&self, st: St, doing: &str) -> Result<(), String> {
        if st == va::VA_STATUS_SUCCESS as St {
            return Ok(());
        }
        // SAFETY: libva's static string for the status.
        let why = unsafe { CStr::from_ptr((self.error_str)(st)) }.to_string_lossy().into_owned();
        Err(format!("{doing}: {why}"))
    }
}

/// libva's "info" lines (its version, the driver it opened) go nowhere.
unsafe extern "C" fn quiet(_: *mut c_void, _: *const c_char) {}

/// A GPU opened for VA-API.
struct Display {
    api: Api,
    dpy: Dpy,
    _node: std::fs::File,
    /// The driver, as it names itself ("Intel iHD driver for Intel(R)
    /// Gen Graphics - 26.2.4").
    vendor: String,
}

impl Display {
    fn open(node: &Path) -> Result<Display, String> {
        let api = Api::open()?;
        let file = std::fs::OpenOptions::new().read(true).write(true).open(node).map_err(|e| format!("{}: {e}", node.display()))?;
        // SAFETY: a DRM node's descriptor, open while the display is.
        let dpy = unsafe { (api.get_display_drm)(file.as_raw_fd()) };
        if dpy.is_null() {
            return Err(format!("{}: no VA-API display", node.display()));
        }
        // SAFETY: our display; the callback is a function that does nothing.
        unsafe { (api.set_info_callback)(dpy, Some(quiet), null_mut()) };
        let (mut major, mut minor) = (0, 0);
        // SAFETY: as above.
        let st = unsafe { (api.initialize)(dpy, &mut major, &mut minor) };
        if let Err(e) = api.ok(st, "vaInitialize") {
            // SAFETY: as above.
            unsafe { (api.terminate)(dpy) };
            return Err(e);
        }
        // SAFETY: libva's string for the display, copied.
        let vendor = unsafe {
            let s = (api.query_vendor_string)(dpy);
            if s.is_null() { String::new() } else { CStr::from_ptr(s).to_string_lossy().into_owned() }
        };
        Ok(Display { api, dpy, _node: file, vendor })
    }

    /// The entrypoint AV1 profile 0 is encoded through (the low-power
    /// one first: Intel's Xe has only that), if any.
    fn av1_entrypoint(&self) -> Option<va::VAEntrypoint> {
        let list = self.entrypoints(va::VAProfileAV1Profile0);
        [va::VAEntrypointEncSliceLP, va::VAEntrypointEncSlice].into_iter().find(|e| list.contains(e))
    }

    /// What the driver does with `profile` (decodes it, `VLD`; encodes
    /// it), none if it doesn't know it.
    fn entrypoints(&self, profile: va::VAProfile) -> Vec<va::VAEntrypoint> {
        // SAFETY: our display.
        let max = unsafe { (self.api.max_num_entrypoints)(self.dpy) }.max(0) as usize;
        let mut list = vec![0 as va::VAEntrypoint; max];
        let mut n = 0;
        // SAFETY: `list` holds the most there can be.
        let st = unsafe { (self.api.query_config_entrypoints)(self.dpy, profile, list.as_mut_ptr(), &mut n) };
        if st != va::VA_STATUS_SUCCESS as St {
            return Vec::new();
        }
        list.truncate(n.max(0) as usize);
        list
    }

    /// The values of `kinds` for AV1 encoding through `entry`.
    fn attributes(&self, entry: va::VAEntrypoint, kinds: &[va::VAConfigAttribType]) -> Result<Vec<u32>, String> {
        let mut attrs: Vec<va::VAConfigAttrib> = kinds.iter().map(|&k| va::VAConfigAttrib { type_: k, value: 0 }).collect();
        // SAFETY: `attrs` holds as many as said.
        let st = unsafe { (self.api.get_config_attributes)(self.dpy, va::VAProfileAV1Profile0, entry, attrs.as_mut_ptr(), attrs.len() as c_int) };
        self.api.ok(st, "vaGetConfigAttributes")?;
        Ok(attrs.into_iter().map(|a| a.value).collect())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: our display, once.
        unsafe { (self.api.terminate)(self.dpy) };
    }
}

/// What a GPU's VA-API driver can encode (AV1 only, for now).
#[derive(Debug, Clone)]
pub struct Abilities {
    /// The render node.
    pub node: PathBuf,
    /// The driver, as it names itself.
    pub driver: String,
    /// AV1 profile 0 (8 and 10 bits, 4:2:0), up to this size.
    pub av1: Option<(u32, u32)>,
}

/// The render nodes (`/dev/dri/renderD*`), in order.
fn render_nodes() -> Vec<PathBuf> {
    let mut nodes: Vec<PathBuf> =
        std::fs::read_dir("/dev/dri").map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))).collect()).unwrap_or_default();
    nodes.sort();
    nodes
}

/// Whether `node`'s GPU is NVIDIA's (its VA-API driver, nouveau's or
/// the NVIDIA one's shim, encodes nothing: NVENC is for it).
fn nvidia(node: &Path) -> bool {
    let Some(name) = node.file_name().and_then(|n| n.to_str()) else { return false };
    std::fs::read_to_string(format!("/sys/class/drm/{name}/device/vendor")).is_ok_and(|v| v.trim() == "0x10de")
}

/// What each GPU's VA-API driver can encode (not NVIDIA's).
pub fn abilities() -> Vec<Abilities> {
    render_nodes().into_iter().filter(|n| !nvidia(n)).filter_map(|n| probe(&n).ok()).collect()
}

/// What `node`'s driver can encode.
pub fn probe(node: &Path) -> Result<Abilities, String> {
    let d = Display::open(node)?;
    let av1 = d.av1_entrypoint().and_then(|e| {
        let v = d.attributes(e, &[va::VAConfigAttribMaxPictureWidth, va::VAConfigAttribMaxPictureHeight]).ok()?;
        Some((v[0], v[1]))
    });
    Ok(Abilities { node: node.to_owned(), driver: d.vendor.clone(), av1 })
}

/// The first GPU that encodes AV1 through VA-API.
pub fn av1_node() -> Option<PathBuf> {
    abilities().into_iter().find(|a| a.av1.is_some()).map(|a| a.node)
}

/// Bits written most significant first, as AV1's headers are.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    bits: usize,
}

impl Writer {
    /// `v`'s lowest `n` bits.
    fn put(&mut self, v: u64, n: u32) {
        for i in (0..n).rev() {
            if self.bits.is_multiple_of(8) {
                self.out.push(0);
            }
            if (v >> i) & 1 == 1 {
                let last = self.out.len() - 1;
                self.out[last] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    fn flag(&mut self, b: bool) {
        self.put(u64::from(b), 1);
    }

    /// trailing_bits(): a one, then zeros to the byte.
    fn trailing(mut self) -> Vec<u8> {
        self.put(1, 1);
        while !self.bits.is_multiple_of(8) {
            self.put(0, 1);
        }
        self.out
    }
}

/// An OBU of `kind` around `payload`, its size in `size_len` bytes (as
/// the driver writes its own: leb128, padded).
pub fn obu(kind: u8, payload: &[u8], size_len: usize) -> Vec<u8> {
    let mut o = vec![(kind << 3) | 0x02];
    let mut v = payload.len();
    for i in 0..size_len {
        let more = if i + 1 < size_len { 0x80 } else { 0 };
        o.push((v & 0x7f) as u8 | more);
        v >>= 7;
    }
    o.extend_from_slice(payload);
    o
}

pub const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;
const OBU_FRAME_HEADER: u8 = 3;
/// Superblocks of 64 × 64 (the sequence header says so).
const SB: u32 = 64;
const MAX_TILE_WIDTH: u32 = 4096;
const MAX_TILE_AREA: u32 = 4096 * 2304;
const MAX_TILE_COLS: u32 = 64;
const MAX_TILE_ROWS: u32 = 64;

/// The smallest `k` for which `block << k` reaches `target`.
fn tile_log2(block: u32, target: u32) -> u32 {
    let mut k = 0;
    while (block << k) < target {
        k += 1;
    }
    k
}

/// How a frame is cut into tiles: the fewest columns and rows AV1 allows
/// for its size, evenly where it can be (as FFmpeg's `av1_vaapi` does).
#[derive(Debug, Clone, PartialEq)]
struct Tiles {
    cols: u32,
    rows: u32,
    cols_log2: u32,
    rows_log2: u32,
    uniform: bool,
    min_log2_cols: u32,
    max_log2_cols: u32,
    min_log2_rows: u32,
    max_log2_rows: u32,
    widths_sb: Vec<u32>,
    heights_sb: Vec<u32>,
}

impl Tiles {
    fn for_size(width: u32, height: u32) -> Tiles {
        let mi_cols = 2 * width.div_ceil(8);
        let mi_rows = 2 * height.div_ceil(8);
        let sb_cols = mi_cols.div_ceil(16);
        let sb_rows = mi_rows.div_ceil(16);
        let sb_log2 = 6;
        let max_tile_width_sb = MAX_TILE_WIDTH >> sb_log2;
        let max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_log2);
        let min_log2_cols = tile_log2(max_tile_width_sb, sb_cols);
        let max_log2_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
        let max_log2_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS));
        let min_log2_tiles = min_log2_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));
        let cols = sb_cols.div_ceil(max_tile_width_sb).clamp(1, sb_cols);
        let cols_log2 = tile_log2(1, cols);
        let tile_width_sb = (sb_cols + (1 << cols_log2) - 1) >> cols_log2;
        let mut rows = 1;
        loop {
            let rows_log2 = tile_log2(1, rows);
            // Even tiles, as the uniform spacing makes them.
            if sb_cols.div_ceil(tile_width_sb) == cols {
                let tile_height_sb = (sb_rows + (1 << rows_log2) - 1) >> rows_log2;
                if sb_rows.div_ceil(tile_height_sb) == rows && tile_height_sb <= max_tile_area_sb / tile_width_sb {
                    let mut widths_sb = vec![tile_width_sb; cols as usize];
                    widths_sb[cols as usize - 1] = sb_cols - (cols - 1) * tile_width_sb;
                    let mut heights_sb = vec![tile_height_sb; rows as usize];
                    heights_sb[rows as usize - 1] = sb_rows - (rows - 1) * tile_height_sb;
                    let min_log2_rows = min_log2_tiles.saturating_sub(cols_log2);
                    return Tiles { cols, rows, cols_log2, rows_log2, uniform: true, min_log2_cols, max_log2_cols, min_log2_rows, max_log2_rows, widths_sb, heights_sb };
                }
            }
            // Else as even as they can be, each said.
            let widths_sb: Vec<u32> = (0..cols).map(|i| (i + 1) * sb_cols / cols - i * sb_cols / cols).collect();
            let widest = widths_sb.iter().copied().max().unwrap_or(1);
            let area = if min_log2_tiles > 0 { (sb_rows * sb_cols) >> (min_log2_tiles + 1) } else { sb_rows * sb_cols };
            let max_tile_height_sb = (area / widest).max(1);
            if rows == rows.clamp(sb_rows.div_ceil(max_tile_height_sb), sb_rows) || rows >= sb_rows.min(MAX_TILE_ROWS) {
                let heights_sb = (0..rows).map(|i| (i + 1) * sb_rows / rows - i * sb_rows / rows).collect();
                return Tiles { cols, rows, cols_log2, rows_log2, uniform: false, min_log2_cols, max_log2_cols, min_log2_rows: 0, max_log2_rows, widths_sb, heights_sb };
            }
            rows += 1;
        }
    }

    /// tile_info(), as the frame header has it.
    fn write(&self, w: &mut Writer, tile_size_bytes_minus_1: u32) {
        w.flag(self.uniform);
        if self.uniform {
            let mut log2 = self.min_log2_cols;
            while log2 < self.max_log2_cols {
                let more = log2 < self.cols_log2;
                w.flag(more);
                if !more {
                    break;
                }
                log2 += 1;
            }
            let mut log2 = self.min_log2_rows;
            while log2 < self.max_log2_rows {
                let more = log2 < self.rows_log2;
                w.flag(more);
                if !more {
                    break;
                }
                log2 += 1;
            }
        } else {
            // Each column's width, then each row's height, in superblocks
            // (ns(), the most a tile can take still to come).
            let sb_cols: u32 = self.widths_sb.iter().sum();
            let mut start = 0;
            for &width in &self.widths_sb {
                let max_width = (sb_cols - start).min(MAX_TILE_WIDTH / SB);
                ns(w, max_width, width - 1);
                start += width;
            }
            let sb_rows: u32 = self.heights_sb.iter().sum();
            let widest = self.widths_sb.iter().copied().max().unwrap_or(1);
            let max_tile_area_sb = if self.min_log2_tiles() > 0 { (sb_rows * sb_cols) >> (self.min_log2_tiles() + 1) } else { sb_rows * sb_cols };
            let max_height = (max_tile_area_sb / widest).max(1);
            let mut start = 0;
            for &height in &self.heights_sb {
                ns(w, (sb_rows - start).min(max_height), height - 1);
                start += height;
            }
        }
        if self.cols_log2 > 0 || self.rows_log2 > 0 {
            // context_update_tile_id: the first.
            w.put(0, self.cols_log2 + self.rows_log2);
            w.put(u64::from(tile_size_bytes_minus_1), 2);
        }
    }

    fn min_log2_tiles(&self) -> u32 {
        let sb_cols: u32 = self.widths_sb.iter().sum();
        let sb_rows: u32 = self.heights_sb.iter().sum();
        self.min_log2_cols.max(tile_log2(MAX_TILE_AREA >> 12, sb_rows * sb_cols))
    }
}

/// ns(n): `v` of `n` values, as AV1 codes it (the first `m` in one bit
/// fewer than the rest).
fn ns(w: &mut Writer, n: u32, v: u32) {
    if n <= 1 {
        return;
    }
    // FloorLog2(n) + 1.
    let bits = 32 - n.leading_zeros();
    let m = (1 << bits) - n;
    if v < m {
        w.put(u64::from(v), bits - 1);
    } else {
        let t = v + m;
        w.put(u64::from(t >> 1), bits - 1);
        w.put(u64::from(t & 1), 1);
    }
}

/// AV1's levels (Annex A): the index, and the most each takes (picture
/// size, width, height, pixels shown a second).
const LEVELS: [(u8, u64, u32, u32, u64); 14] = [
    (0, 147_456, 2048, 1152, 4_423_680),
    (1, 278_784, 2816, 1584, 8_363_520),
    (4, 665_856, 4352, 2448, 19_975_680),
    (5, 1_065_024, 5504, 3096, 31_950_720),
    (8, 2_359_296, 6144, 3456, 70_778_880),
    (9, 2_359_296, 6144, 3456, 141_557_760),
    (12, 8_912_896, 8192, 4352, 267_386_880),
    (13, 8_912_896, 8192, 4352, 534_773_760),
    (14, 8_912_896, 8192, 4352, 1_069_547_520),
    (15, 8_912_896, 8192, 4352, 1_069_547_520),
    (16, 35_651_584, 16384, 8704, 1_069_547_520),
    (17, 35_651_584, 16384, 8704, 2_139_095_040),
    (18, 35_651_584, 16384, 8704, 4_278_190_080),
    (19, 35_651_584, 16384, 8704, 4_278_190_080),
];

/// The lowest level for `width` × `height` at `fps` (31: none says).
fn level(width: u32, height: u32, fps: u32) -> u8 {
    let pixels = u64::from(width) * u64::from(height);
    LEVELS.iter().find(|&&(_, size, w, h, rate)| pixels <= size && width <= w && height <= h && pixels * u64::from(fps) <= rate).map_or(31, |l| l.0)
}

/// The sequence header OBU's payload: profile 0, 8 bits, 4:2:0, BT.709
/// in limited range, order hints of 8 bits, none of the tools the
/// driver isn't told of.
pub fn sequence_header(width: u32, height: u32, level: u8) -> Vec<u8> {
    let mut w = Writer::default();
    w.put(0, 3); // seq_profile
    w.flag(false); // still_picture
    w.flag(false); // reduced_still_picture_header
    w.flag(false); // timing_info_present_flag
    w.flag(false); // initial_display_delay_present_flag
    w.put(0, 5); // operating_points_cnt_minus_1
    w.put(0, 12); // operating_point_idc[0]
    w.put(u64::from(level), 5);
    if level > 7 {
        w.flag(false); // seq_tier: Main
    }
    let wbits = 32 - (width - 1).leading_zeros().min(31);
    let hbits = 32 - (height - 1).leading_zeros().min(31);
    let (wbits, hbits) = (wbits.max(1), hbits.max(1));
    w.put(u64::from(wbits - 1), 4);
    w.put(u64::from(hbits - 1), 4);
    w.put(u64::from(width - 1), wbits);
    w.put(u64::from(height - 1), hbits);
    w.flag(false); // frame_id_numbers_present_flag
    w.flag(false); // use_128x128_superblock
    w.flag(false); // enable_filter_intra
    w.flag(false); // enable_intra_edge_filter
    w.flag(false); // enable_interintra_compound
    w.flag(false); // enable_masked_compound
    w.flag(false); // enable_warped_motion
    w.flag(false); // enable_dual_filter
    w.flag(true); // enable_order_hint
    w.flag(false); // enable_jnt_comp
    w.flag(false); // enable_ref_frame_mvs
    w.flag(false); // seq_choose_screen_content_tools
    w.flag(false); // seq_force_screen_content_tools
    w.put(7, 3); // order_hint_bits_minus_1
    w.flag(false); // enable_superres
    w.flag(false); // enable_cdef
    w.flag(false); // enable_restoration
    // color_config()
    w.flag(false); // high_bitdepth
    w.flag(false); // mono_chrome
    w.flag(true); // color_description_present_flag
    w.put(1, 8); // color_primaries: BT.709
    w.put(1, 8); // transfer_characteristics: BT.709
    w.put(1, 8); // matrix_coefficients: BT.709
    w.flag(false); // color_range: limited
    w.put(0, 2); // chroma_sample_position: unknown
    w.flag(false); // separate_uv_delta_q
    w.flag(false); // film_grain_params_present
    w.trailing()
}

/// What a frame header says, beyond what's fixed.
struct Frame {
    key: bool,
    order_hint: u8,
    /// The slot it is kept in (P frames), and the one it's predicted from.
    refresh: u8,
    from: u8,
    qindex: u8,
    tx_select: bool,
    /// The loop filter's levels (luma across and down, then U and V).
    lf: [u8; 4],
}

/// A frame header OBU's payload (uncompressed_header()), as the driver
/// is told the frame is coded.
fn frame_header(f: &Frame, tiles: &Tiles, tile_size_bytes_minus_1: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.flag(false); // show_existing_frame
    w.put(if f.key { 0 } else { 1 }, 2); // frame_type: KEY_FRAME or INTER_FRAME
    w.flag(true); // show_frame
    if !f.key {
        w.flag(false); // error_resilient_mode (a shown key frame's is 1)
    }
    w.flag(false); // disable_cdf_update
    w.flag(false); // frame_size_override_flag
    w.put(u64::from(f.order_hint), 8);
    if !f.key {
        w.put(0, 3); // primary_ref_frame: LAST's
        w.put(u64::from(f.refresh), 8); // refresh_frame_flags
    }
    if f.key {
        // frame_size() (as the sequence's), render_size():
        w.flag(false); // render_and_frame_size_different
    } else {
        w.flag(false); // frame_refs_short_signaling
        for _ in 0..7 {
            w.put(u64::from(f.from), 3); // ref_frame_idx[i]
        }
        w.flag(false); // render_and_frame_size_different
        w.flag(false); // allow_high_precision_mv
        w.flag(false); // is_filter_switchable
        w.put(0, 2); // interpolation_filter: EIGHTTAP
        w.flag(false); // is_motion_mode_switchable
    }
    w.flag(false); // disable_frame_end_update_cdf
    tiles.write(&mut w, tile_size_bytes_minus_1);
    // quantization_params()
    w.put(u64::from(f.qindex), 8);
    w.flag(false); // DeltaQYDc
    w.flag(false); // DeltaQUDc
    w.flag(false); // DeltaQUAc
    w.flag(false); // using_qmatrix
    w.flag(false); // segmentation_enabled
    if f.qindex > 0 {
        w.flag(false); // delta_q_present
    }
    // loop_filter_params()
    w.put(u64::from(f.lf[0]), 6);
    w.put(u64::from(f.lf[1]), 6);
    if f.lf[0] != 0 || f.lf[1] != 0 {
        w.put(u64::from(f.lf[2]), 6);
        w.put(u64::from(f.lf[3]), 6);
    }
    w.put(0, 3); // loop_filter_sharpness
    w.flag(false); // loop_filter_delta_enabled
    // (cdef_params, lr_params: the sequence has neither.)
    w.flag(f.tx_select); // tx_mode_select
    if !f.key {
        w.flag(false); // reference_select
    }
    w.flag(false); // reduced_tx_set
    if !f.key {
        for _ in 0..7 {
            w.flag(false); // is_global
        }
    }
    w.trailing()
}

/// BGRA (or RGBA) rows to NV12 (BT.709, limited range) in `y` and `uv`
/// (`pitch_y`, `pitch_uv` bytes a row), on a few threads.
#[allow(clippy::too_many_arguments)]
fn to_nv12(data: &[u8], stride: usize, rgba: bool, width: usize, height: usize, y: &mut [u8], pitch_y: usize, uv: &mut [u8], pitch_uv: usize) {
    let (r, b) = if rgba { (0, 2) } else { (2, 0) };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    // Pairs of rows, shared out.
    let pairs = height / 2;
    let per = pairs.div_ceil(threads);
    let mut ys: Vec<&mut [u8]> = y.chunks_mut(pitch_y * 2 * per).collect();
    let mut uvs: Vec<&mut [u8]> = uv.chunks_mut(pitch_uv * per).collect();
    std::thread::scope(|s| {
        for (t, (yc, uvc)) in ys.iter_mut().zip(uvs.iter_mut()).enumerate() {
            let first = t * per;
            s.spawn(move || {
                for p in 0..per {
                    let pair = first + p;
                    if pair >= pairs {
                        break;
                    }
                    for row in 0..2 {
                        let src = &data[(pair * 2 + row) * stride..];
                        let dst = &mut yc[(p * 2 + row) * pitch_y..];
                        for x in 0..width {
                            let px = &src[x * 4..x * 4 + 3];
                            let (rr, gg, bb) = (i32::from(px[r]), i32::from(px[1]), i32::from(px[b]));
                            dst[x] = (((11966 * rr + 40254 * gg + 4064 * bb + 32768) >> 16) + 16) as u8;
                        }
                    }
                    let (a, c) = (&data[pair * 2 * stride..], &data[(pair * 2 + 1) * stride..]);
                    let dst = &mut uvc[p * pitch_uv..];
                    for x in 0..width / 2 {
                        let o = x * 8;
                        let sum = |i: usize| i32::from(a[o + i]) + i32::from(a[o + 4 + i]) + i32::from(c[o + i]) + i32::from(c[o + 4 + i]);
                        let (rr, gg, bb) = (sum(r), sum(1), sum(b));
                        // (Four pixels' sums: a quarter, with the scale.)
                        let u = (-6596 * rr - 22189 * gg + 28785 * bb + (1 << 17)) >> 18;
                        let v = (28785 * rr - 26145 * gg - 2640 * bb + (1 << 17)) >> 18;
                        dst[x * 2] = (u + 128).clamp(16, 240) as u8;
                        dst[x * 2 + 1] = (v + 128).clamp(16, 240) as u8;
                    }
                }
            });
        }
    });
}

/// An AV1 encoder on a GPU's VA-API driver.
pub struct Av1 {
    d: Display,
    config: va::VAConfigID,
    context: va::VAContextID,
    input: va::VASurfaceID,
    /// Two reconstructed pictures, taken in turn.
    recon: [va::VASurfaceID; 2],
    coded: va::VABufferID,
    /// The input surface's memory (derived), or an image of ours put in.
    image: va::VAImage,
    derived: bool,
    width: u32,
    height: u32,
    tiles: Tiles,
    level: u8,
    seq: Vec<u8>,
    obu_size_len: usize,
    tile_size_bytes_minus_1: u32,
    tx_select: bool,
    qindex: u8,
    /// Frames encoded, and since the last key frame.
    frame: u64,
    since_key: u64,
    /// A key frame every this many.
    gop: u64,
    /// The slot the last frame was kept in.
    slot: u8,
    /// The video processor making NV12 of pictures in GPU memory
    /// ([`Av1::encode_prime`]): its config and context, once asked for.
    vpp: Option<(va::VAConfigID, va::VAContextID)>,
    /// Those pictures' buffers as surfaces, by their keys (a capture's
    /// few, used in turn), the newest last.
    imported: Vec<(u64, va::VASurfaceID)>,
}

/// A picture in GPU memory for [`Av1::encode_prime`]: a dmabuf of one
/// plane of XRGB8888 (BGRx in memory, as app_server draws), laid out as
/// `modifier` says.
pub struct Prime<'a> {
    /// The same while the buffer is (its import is kept for it).
    pub key: u64,
    pub fd: std::os::fd::BorrowedFd<'a>,
    pub modifier: u64,
    pub offset: u32,
    pub pitch: u32,
    /// The buffer's size, in bytes.
    pub size: u32,
    pub width: u32,
    pub height: u32,
}

/// DRM's fourcc for XRGB8888 ("XR24").
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

/// Whether [`Av1::encode_prime`] reads pictures laid out as `modifier`
/// says: linear, and Intel's tiles without compression (X, Y, 4). (libva
/// can't be asked: its modifier list is for allocating. Xe2's 4 with
/// compression, 0x0100000000000010, imports without a word and reads as
/// noise, as AMD's with DCC might, untried.)
pub fn imports(modifier: u64) -> bool {
    matches!(modifier, 0 | 0x0100_0000_0000_0001 | 0x0100_0000_0000_0002 | 0x0100_0000_0000_0009)
}
/// Imports kept at most (a capture's buffers are two or three).
const IMPORTS_KEPT: usize = 4;

// (Its display and surfaces are used from one thread at a time, through
// `&mut`.)
unsafe impl Send for Av1 {}

impl Av1 {
    /// One for `width` × `height` (even) pictures at up to `fps` a second
    /// on `node`'s GPU (None: the first that encodes AV1), at quality
    /// `qindex` (1–255; [`QUALITY`]).
    pub fn new(node: Option<&Path>, width: u32, height: u32, fps: u32, qindex: u8) -> Result<Av1, String> {
        if width == 0 || height == 0 || width % 2 == 1 || height % 2 == 1 {
            return Err(format!("{width}×{height} isn't an even size"));
        }
        let node = match node {
            Some(n) => n.to_owned(),
            None => av1_node().ok_or("no GPU here encodes AV1 through VA-API")?,
        };
        let d = Display::open(&node)?;
        let entry = d.av1_entrypoint().ok_or_else(|| format!("{} ({}) doesn't encode AV1", node.display(), d.vendor))?;
        let v = d.attributes(
            entry,
            &[
                va::VAConfigAttribRTFormat,
                va::VAConfigAttribRateControl,
                va::VAConfigAttribEncPackedHeaders,
                va::VAConfigAttribEncAV1Ext2,
                va::VAConfigAttribMaxPictureWidth,
                va::VAConfigAttribMaxPictureHeight,
            ],
        )?;
        let (rt, rc, packed, ext2, max_w, max_h) = (v[0], v[1], v[2], v[3], v[4], v[5]);
        if rt & va::VA_RT_FORMAT_YUV420 == 0 {
            return Err("the driver doesn't take 4:2:0 pictures for AV1".into());
        }
        if rc & va::VA_RC_CQP == 0 {
            return Err("the driver has no constant quality for AV1".into());
        }
        let want = va::VA_ENC_PACKED_HEADER_SEQUENCE | va::VA_ENC_PACKED_HEADER_PICTURE;
        if packed == va::VA_ATTRIB_NOT_SUPPORTED || packed & want != want {
            return Err("the driver doesn't take AV1's headers written for it".into());
        }
        if ext2 == va::VA_ATTRIB_NOT_SUPPORTED {
            return Err("the driver doesn't say how it writes AV1".into());
        }
        if max_w != va::VA_ATTRIB_NOT_SUPPORTED && max_h != va::VA_ATTRIB_NOT_SUPPORTED && (width > max_w || height > max_h) {
            return Err(format!("{width}×{height} is larger than the GPU encodes ({max_w}×{max_h})"));
        }
        let ext2 = va::VAConfigAttribValEncAV1Ext2 { value: ext2 };
        // SAFETY: the driver's value, read as the union's bits.
        let (obu_size_len, tile_size_bytes_minus_1, tx_modes, max_tiles) =
            unsafe { (ext2.bits.obu_size_bytes_minus1() as usize + 1, ext2.bits.tile_size_bytes_minus1(), ext2.bits.tx_mode_support(), ext2.bits.max_tile_num_minus1() + 1) };
        let tx_select = tx_modes & 0x04 != 0;
        if !tx_select && tx_modes & 0x02 == 0 {
            return Err("the driver says no transform mode it can code".into());
        }
        let tiles = Tiles::for_size(width, height);
        if max_tiles > 1 && tiles.cols * tiles.rows > max_tiles {
            return Err(format!("{width}×{height} takes {} tiles, more than the GPU codes ({max_tiles})", tiles.cols * tiles.rows));
        }
        let api = &d.api;
        let mut attrs = [
            va::VAConfigAttrib { type_: va::VAConfigAttribRTFormat, value: va::VA_RT_FORMAT_YUV420 },
            va::VAConfigAttrib { type_: va::VAConfigAttribRateControl, value: va::VA_RC_CQP },
            va::VAConfigAttrib { type_: va::VAConfigAttribEncPackedHeaders, value: want },
        ];
        let mut config = 0;
        // SAFETY: our display; `attrs` as many as said.
        api.ok(unsafe { (api.create_config)(d.dpy, va::VAProfileAV1Profile0, entry, attrs.as_mut_ptr(), attrs.len() as c_int, &mut config) }, "vaCreateConfig")?;
        // Surfaces whole superblocks, as the driver codes them.
        let (sw, sh) = (width.div_ceil(SB) * SB, height.div_ceil(SB) * SB);
        let mut surfaces = [va::VA_INVALID_SURFACE; 3];
        // SAFETY: as above; room for three.
        let st = unsafe { (api.create_surfaces)(d.dpy, va::VA_RT_FORMAT_YUV420, sw, sh, surfaces.as_mut_ptr(), 3, null_mut(), 0) };
        if let Err(e) = api.ok(st, "vaCreateSurfaces") {
            // SAFETY: the config made above.
            unsafe { (api.destroy_config)(d.dpy, config) };
            return Err(e);
        }
        let mut context = 0;
        // SAFETY: as above.
        let st = unsafe { (api.create_context)(d.dpy, config, sw as c_int, sh as c_int, va::VA_PROGRESSIVE as c_int, surfaces.as_mut_ptr(), 3, &mut context) };
        let mut av1 = Av1 {
            config,
            context: va::VA_INVALID_ID,
            input: surfaces[0],
            recon: [surfaces[1], surfaces[2]],
            coded: va::VA_INVALID_ID,
            image: va::VAImage { image_id: va::VA_INVALID_ID, ..Default::default() },
            derived: false,
            width,
            height,
            level: level(width, height, fps.clamp(1, 240)),
            seq: Vec::new(),
            tiles,
            obu_size_len,
            tile_size_bytes_minus_1,
            tx_select,
            qindex: qindex.max(1),
            frame: 0,
            since_key: 0,
            gop: u64::from(fps.clamp(1, 240)) * 2,
            slot: 0,
            vpp: None,
            imported: Vec::new(),
            d,
        };
        let api = &av1.d.api;
        api.ok(st, "vaCreateContext")?;
        av1.context = context;
        // The coded picture's room: its NV12 size, at the most.
        let room = sw * sh * 3 / 2;
        let mut coded = 0;
        // SAFETY: as above.
        api.ok(unsafe { (api.create_buffer)(av1.d.dpy, context, va::VAEncCodedBufferType, room, 1, null_mut(), &mut coded) }, "vaCreateBuffer (coded)")?;
        av1.coded = coded;
        // Pictures go in through the input surface's own memory where the
        // driver lets it be mapped, else through an image of ours put in.
        let mut image = va::VAImage::default();
        // SAFETY: as above.
        let derived = unsafe { (api.derive_image)(av1.d.dpy, av1.input, &mut image) } == va::VA_STATUS_SUCCESS as St && image.format.fourcc == va::VA_FOURCC_NV12;
        if !derived {
            if image.image_id != va::VA_INVALID_ID && image.image_id != 0 {
                // SAFETY: the image derived, of the wrong kind.
                unsafe { (api.destroy_image)(av1.d.dpy, image.image_id) };
            }
            // (Byte order 1: VA_LSB_FIRST.)
            let mut format = va::VAImageFormat { fourcc: va::VA_FOURCC_NV12, byte_order: 1, bits_per_pixel: 12, ..Default::default() };
            image = va::VAImage::default();
            // SAFETY: as above.
            api.ok(unsafe { (api.create_image)(av1.d.dpy, &mut format, sw as c_int, sh as c_int, &mut image) }, "vaCreateImage")?;
        }
        av1.image = image;
        av1.derived = derived;
        av1.seq = obu(OBU_SEQUENCE_HEADER, &sequence_header(width, height, av1.level), obu_size_len);
        Ok(av1)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The driver, as it names itself.
    pub fn driver(&self) -> &str {
        &self.d.vendor
    }

    /// The GPU it encodes on, as its render node's device number (to
    /// tell whether a dmabuf from elsewhere is in its memory, for
    /// [`encode_prime`](Self::encode_prime)).
    pub fn device(&self) -> Option<u64> {
        use std::os::unix::fs::MetadataExt as _;
        self.d._node.metadata().ok().map(|m| m.rdev())
    }

    /// One picture (BGRA rows, or RGBA with `rgba`; `stride` bytes a
    /// row, at least this size) encoded: a temporal unit's OBUs (the
    /// sequence header with a key frame), and whether it's a key frame.
    pub fn encode(&mut self, data: &[u8], stride: usize, rgba: bool) -> Result<(Vec<u8>, bool), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if stride < w * 4 || data.len() < stride * (h - 1) + w * 4 {
            return Err("the picture is smaller than the video".into());
        }
        self.upload(data, stride, rgba)?;
        self.encode_input()
    }

    /// A picture in GPU memory (a capture's dmabuf) encoded, as
    /// [`encode`](Self::encode) does one in memory: made NV12 by the GPU's
    /// video processor straight into the input surface, never copied
    /// through the CPU. Its top left `width` × `height` are the video.
    pub fn encode_prime(&mut self, picture: &Prime) -> Result<(Vec<u8>, bool), String> {
        if picture.width < self.width || picture.height < self.height {
            return Err("the picture is smaller than the video".into());
        }
        self.convert(picture)?;
        self.encode_input()
    }

    /// The input surface, filled, encoded.
    fn encode_input(&mut self) -> Result<(Vec<u8>, bool), String> {
        let key = self.since_key == 0 || self.since_key >= self.gop;
        if key {
            self.since_key = 0;
        }
        let from = self.slot;
        let slot = if key { 0 } else { 1 - from };
        // The loop filter, as strong as the quantizer is coarse (measured:
        // a quarter of a dB better at 6 for 72, the files no larger).
        let lf = self.qindex / 12;
        let f = Frame { key, order_hint: (self.since_key % 256) as u8, refresh: if key { 0xff } else { 1 << slot }, from, qindex: self.qindex, tx_select: self.tx_select, lf: [lf, lf, lf, lf] };
        let header = obu(OBU_FRAME_HEADER, &frame_header(&f, &self.tiles, self.tile_size_bytes_minus_1), self.obu_size_len);
        let out = self.code(&f, slot, &header)?;
        self.slot = slot;
        self.frame += 1;
        self.since_key += 1;
        // A temporal delimiter first, as a stream of OBUs has.
        let mut unit = vec![OBU_TEMPORAL_DELIMITER << 3 | 0x02, 0];
        unit.extend_from_slice(&out);
        Ok((unit, key))
    }

    /// `picture` into the input surface as NV12, by the video processor
    /// (BT.709, full range in, limited out, as the sequence header says).
    fn convert(&mut self, picture: &Prime) -> Result<(), String> {
        let ctx = match self.vpp {
            Some((_, ctx)) => ctx,
            None => {
                let api = &self.d.api;
                let mut cfg = va::VA_INVALID_ID;
                // SAFETY: our display; no attributes.
                api.ok(unsafe { (api.create_config)(self.d.dpy, va::VAProfileNone, va::VAEntrypointVideoProc, null_mut(), 0, &mut cfg) }, "vaCreateConfig (video processing)")?;
                let mut ctx = va::VA_INVALID_ID;
                // SAFETY: the config just made, the input surface its target.
                let st = unsafe { (api.create_context)(self.d.dpy, cfg, self.width as c_int, self.height as c_int, va::VA_PROGRESSIVE as c_int, &mut self.input, 1, &mut ctx) };
                if let Err(e) = api.ok(st, "vaCreateContext (video processing)") {
                    // SAFETY: the config just made.
                    unsafe { (api.destroy_config)(self.d.dpy, cfg) };
                    return Err(e);
                }
                self.vpp = Some((cfg, ctx));
                ctx
            }
        };
        let surface = self.import(picture)?;
        let api = &self.d.api;
        let region = va::VARectangle { x: 0, y: 0, width: self.width as u16, height: self.height as u16 };
        let mut pp = va::VAProcPipelineParameterBuffer {
            surface,
            surface_region: &region,
            surface_color_standard: va::VAProcColorStandardBT709,
            output_region: &region,
            output_background_color: 0xff00_0000,
            output_color_standard: va::VAProcColorStandardBT709,
            ..Default::default()
        };
        pp.input_color_properties.color_range = va::VA_SOURCE_RANGE_FULL as u8;
        pp.output_color_properties.color_range = va::VA_SOURCE_RANGE_REDUCED as u8;
        let mut buf = va::VA_INVALID_ID;
        // SAFETY: libva copies the parameters (the regions read while
        // rendering, which is done before they go).
        let st = unsafe {
            (api.create_buffer)(
                self.d.dpy,
                ctx,
                va::VAProcPipelineParameterBufferType,
                std::mem::size_of::<va::VAProcPipelineParameterBuffer>() as c_uint,
                1,
                (&mut pp as *mut va::VAProcPipelineParameterBuffer).cast(),
                &mut buf,
            )
        };
        api.ok(st, "vaCreateBuffer (video processing)")?;
        // SAFETY: our context, the input surface its target, the buffer
        // just made.
        let done = unsafe {
            api.ok((api.begin_picture)(self.d.dpy, ctx, self.input), "vaBeginPicture (video processing)")
                .and_then(|_| api.ok((api.render_picture)(self.d.dpy, ctx, &mut buf, 1), "vaRenderPicture (video processing)"))
                .and_then(|_| api.ok((api.end_picture)(self.d.dpy, ctx), "vaEndPicture (video processing)"))
        };
        // SAFETY: the buffer made above.
        unsafe { (api.destroy_buffer)(self.d.dpy, buf) };
        done?;
        // SAFETY: our input surface.
        api.ok(unsafe { (api.sync_surface)(self.d.dpy, self.input) }, "vaSyncSurface (video processing)")
    }

    /// `picture`'s buffer as a surface (made once while kept).
    fn import(&mut self, picture: &Prime) -> Result<va::VASurfaceID, String> {
        use std::os::fd::AsRawFd as _;
        if let Some(i) = self.imported.iter().position(|(k, _)| *k == picture.key) {
            let kept = self.imported.remove(i);
            self.imported.push(kept);
            return Ok(kept.1);
        }
        let mut desc = va::VADRMPRIMESurfaceDescriptor { fourcc: va::VA_FOURCC_BGRX, width: picture.width, height: picture.height, num_objects: 1, num_layers: 1, ..Default::default() };
        desc.objects[0].fd = picture.fd.as_raw_fd();
        desc.objects[0].size = picture.size;
        desc.objects[0].drm_format_modifier = picture.modifier;
        desc.layers[0].drm_format = DRM_FORMAT_XRGB8888;
        desc.layers[0].num_planes = 1;
        desc.layers[0].object_index[0] = 0;
        desc.layers[0].offset[0] = picture.offset;
        desc.layers[0].pitch[0] = picture.pitch;
        let mut attrs = [
            va::VASurfaceAttrib {
                type_: va::VASurfaceAttribMemoryType,
                flags: va::VA_SURFACE_ATTRIB_SETTABLE,
                value: va::VAGenericValue { type_: va::VAGenericValueTypeInteger, value: va::_VAGenericValue__bindgen_ty_1 { i: va::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 as i32 } },
            },
            va::VASurfaceAttrib {
                type_: va::VASurfaceAttribExternalBufferDescriptor,
                flags: va::VA_SURFACE_ATTRIB_SETTABLE,
                value: va::VAGenericValue { type_: va::VAGenericValueTypePointer, value: va::_VAGenericValue__bindgen_ty_1 { p: (&mut desc as *mut va::VADRMPRIMESurfaceDescriptor).cast() } },
            },
        ];
        let api = &self.d.api;
        let mut surface = va::VA_INVALID_ID;
        // SAFETY: the descriptor lives through the call; the driver takes
        // the buffer in (its own handle: the fd is the caller's still).
        let st = unsafe { (api.create_surfaces)(self.d.dpy, va::VA_RT_FORMAT_RGB32, picture.width, picture.height, &mut surface, 1, attrs.as_mut_ptr(), attrs.len() as c_uint) };
        api.ok(st, "vaCreateSurfaces (a dmabuf)")?;
        if self.imported.len() >= IMPORTS_KEPT {
            let (_, mut old) = self.imported.remove(0);
            // SAFETY: a surface we imported, no longer kept.
            unsafe { (api.destroy_surfaces)(self.d.dpy, &mut old, 1) };
        }
        self.imported.push((picture.key, surface));
        Ok(surface)
    }

    /// The picture into the input surface, as NV12.
    fn upload(&mut self, data: &[u8], stride: usize, rgba: bool) -> Result<(), String> {
        let api = &self.d.api;
        let mut p: *mut c_void = null_mut();
        // SAFETY: our display and image's buffer.
        api.ok(unsafe { (api.map_buffer)(self.d.dpy, self.image.buf, &mut p) }, "vaMapBuffer (picture)")?;
        let img = &self.image;
        let (pitch_y, pitch_uv) = (img.pitches[0] as usize, img.pitches[1] as usize);
        let (w, h) = (self.width as usize, self.height as usize);
        // SAFETY: the mapped image, `data_size` bytes, its planes where it
        // says; written only between map and unmap.
        let all = unsafe { std::slice::from_raw_parts_mut(p.cast::<u8>(), img.data_size as usize) };
        let (y_off, uv_off) = (img.offsets[0] as usize, img.offsets[1] as usize);
        let (ys, uvs) = if y_off < uv_off {
            let (a, b) = all.split_at_mut(uv_off);
            (&mut a[y_off..], b)
        } else {
            let (a, b) = all.split_at_mut(y_off);
            (b, &mut a[uv_off..])
        };
        to_nv12(data, stride, rgba, w, h, ys, pitch_y, uvs, pitch_uv);
        // SAFETY: as above.
        api.ok(unsafe { (api.unmap_buffer)(self.d.dpy, self.image.buf) }, "vaUnmapBuffer (picture)")?;
        if !self.derived {
            // SAFETY: our surface and image, the picture's size.
            let st = unsafe { (api.put_image)(self.d.dpy, self.input, self.image.image_id, 0, 0, w as c_uint, h as c_uint, 0, 0, w as c_uint, h as c_uint) };
            api.ok(st, "vaPutImage")?;
        }
        Ok(())
    }

    /// A buffer of `kind` holding `value`, made for this picture.
    fn buffer<T>(&self, kind: va::VABufferType, value: &T, bufs: &mut Vec<va::VABufferID>) -> Result<(), String> {
        let api = &self.d.api;
        let mut id = 0;
        // SAFETY: libva copies `size_of::<T>()` bytes from `value`.
        let st = unsafe { (api.create_buffer)(self.d.dpy, self.context, kind, std::mem::size_of::<T>() as c_uint, 1, (value as *const T).cast_mut().cast(), &mut id) };
        api.ok(st, "vaCreateBuffer")?;
        bufs.push(id);
        Ok(())
    }

    /// A packed header (`bytes`, whole) of `kind`.
    fn packed(&self, kind: va::VAEncPackedHeaderType, bytes: &[u8], bufs: &mut Vec<va::VABufferID>) -> Result<(), String> {
        let param = va::VAEncPackedHeaderParameterBuffer { type_: kind, bit_length: (bytes.len() * 8) as u32, has_emulation_bytes: 0, ..Default::default() };
        self.buffer(va::VAEncPackedHeaderParameterBufferType, &param, bufs)?;
        let api = &self.d.api;
        let mut id = 0;
        // SAFETY: libva copies `bytes`.
        let st = unsafe { (api.create_buffer)(self.d.dpy, self.context, va::VAEncPackedHeaderDataBufferType, bytes.len() as c_uint, 1, bytes.as_ptr().cast_mut().cast(), &mut id) };
        api.ok(st, "vaCreateBuffer (header)")?;
        bufs.push(id);
        Ok(())
    }

    /// Encode the input surface as `f` into `slot`, its frame header
    /// `header`: the coded bytes.
    fn code(&mut self, f: &Frame, slot: u8, header: &[u8]) -> Result<Vec<u8>, String> {
        let mut bufs = Vec::new();
        let result = self.code_with(f, slot, header, &mut bufs);
        let api = &self.d.api;
        for b in bufs {
            // SAFETY: buffers made for this picture.
            unsafe { (api.destroy_buffer)(self.d.dpy, b) };
        }
        result
    }

    fn code_with(&mut self, f: &Frame, slot: u8, header: &[u8], bufs: &mut Vec<va::VABufferID>) -> Result<Vec<u8>, String> {
        if f.key {
            let mut seq = va::VAEncSequenceParameterBufferAV1 {
                seq_profile: 0,
                seq_level_idx: self.level,
                seq_tier: 0,
                intra_period: self.gop as u32,
                ip_period: 1,
                order_hint_bits_minus_1: 7,
                ..Default::default()
            };
            // SAFETY: setting a bit of the union's bitfield.
            unsafe { seq.seq_fields.bits.set_enable_order_hint(1) };
            self.buffer(va::VAEncSequenceParameterBufferType, &seq, bufs)?;
            self.packed(va::VAEncPackedHeaderSequence, &self.seq, bufs)?;
        }
        let mut pic = va::VAEncPictureParameterBufferAV1 {
            frame_width_minus_1: (self.width - 1) as u16,
            frame_height_minus_1: (self.height - 1) as u16,
            reconstructed_frame: self.recon[slot as usize],
            coded_buf: self.coded,
            reference_frames: [va::VA_INVALID_SURFACE; 8],
            primary_ref_frame: if f.key { 7 } else { 0 },
            order_hint: f.order_hint,
            refresh_frame_flags: f.refresh,
            base_qindex: f.qindex,
            tile_cols: self.tiles.cols as u8,
            tile_rows: self.tiles.rows as u8,
            filter_level: [f.lf[0], f.lf[1]],
            filter_level_u: f.lf[2],
            filter_level_v: f.lf[3],
            ..Default::default()
        };
        for (i, w) in self.tiles.widths_sb.iter().enumerate() {
            pic.width_in_sbs_minus_1[i] = (*w - 1) as u16;
        }
        for (i, h) in self.tiles.heights_sb.iter().enumerate() {
            pic.height_in_sbs_minus_1[i] = (*h - 1) as u16;
        }
        if !f.key {
            pic.ref_frame_idx = [f.from; 7];
            pic.reference_frames[f.from as usize] = self.recon[f.from as usize];
        }
        // SAFETY: setting bits of the unions' bitfields.
        unsafe {
            pic.picture_flags.bits.set_frame_type(if f.key { 0 } else { 1 });
            pic.picture_flags.bits.set_error_resilient_mode(u32::from(f.key));
            pic.picture_flags.bits.set_enable_frame_obu(0);
            pic.mode_control_flags.bits.set_reference_mode(0);
            pic.mode_control_flags.bits.set_tx_mode(if f.tx_select { 2 } else { 1 });
            pic.tile_group_obu_hdr_info.bits.set_obu_has_size_field(1);
            if !f.key {
                // LAST.
                pic.ref_frame_ctrl_l0.fields.set_search_idx0(1);
            }
        }
        self.buffer(va::VAEncPictureParameterBufferType, &pic, bufs)?;
        self.packed(va::VAEncPackedHeaderPicture, header, bufs)?;
        let tg = va::VAEncTileGroupBufferAV1 { tg_start: 0, tg_end: (self.tiles.cols * self.tiles.rows - 1) as u8, ..Default::default() };
        self.buffer(va::VAEncSliceParameterBufferType, &tg, bufs)?;
        let api = &self.d.api;
        let (dpy, ctx) = (self.d.dpy, self.context);
        // SAFETY: our display, context, surface and buffers.
        unsafe {
            api.ok((api.begin_picture)(dpy, ctx, self.input), "vaBeginPicture")?;
            api.ok((api.render_picture)(dpy, ctx, bufs.as_mut_ptr(), bufs.len() as c_int), "vaRenderPicture")?;
            api.ok((api.end_picture)(dpy, ctx), "vaEndPicture")?;
            api.ok((api.sync_surface)(dpy, self.input), "vaSyncSurface")?;
        }
        // What came out: the coded buffer's segments, in order.
        let mut out = Vec::new();
        let mut p: *mut c_void = null_mut();
        // SAFETY: as above.
        api.ok(unsafe { (api.map_buffer)(dpy, self.coded, &mut p) }, "vaMapBuffer (coded)")?;
        let mut seg = p.cast::<va::VACodedBufferSegment>();
        while !seg.is_null() {
            // SAFETY: the driver's list of segments, mapped.
            let s = unsafe { &*seg };
            if !s.buf.is_null() && s.size > 0 {
                // SAFETY: `size` bytes at `buf`.
                out.extend_from_slice(unsafe { std::slice::from_raw_parts(s.buf.cast::<u8>(), s.size as usize) });
            }
            seg = s.next.cast();
        }
        // SAFETY: as above.
        api.ok(unsafe { (api.unmap_buffer)(dpy, self.coded) }, "vaUnmapBuffer (coded)")?;
        Ok(out)
    }
}

impl Drop for Av1 {
    fn drop(&mut self) {
        let api = &self.d.api;
        let dpy = self.d.dpy;
        // SAFETY: what we made on our display, each once.
        unsafe {
            if self.image.image_id != va::VA_INVALID_ID {
                (api.destroy_image)(dpy, self.image.image_id);
            }
            if self.coded != va::VA_INVALID_ID {
                (api.destroy_buffer)(dpy, self.coded);
            }
            if self.context != va::VA_INVALID_ID {
                (api.destroy_context)(dpy, self.context);
            }
            if let Some((cfg, ctx)) = self.vpp {
                (api.destroy_context)(dpy, ctx);
                (api.destroy_config)(dpy, cfg);
            }
            for (_, mut s) in self.imported.drain(..) {
                (api.destroy_surfaces)(dpy, &mut s, 1);
            }
            let mut surfaces = [self.input, self.recon[0], self.recon[1]];
            (api.destroy_surfaces)(dpy, surfaces.as_mut_ptr(), 3);
            (api.destroy_config)(dpy, self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_go_in_most_significant_first() {
        let mut w = Writer::default();
        w.put(0b101, 3);
        w.put(0xff, 8);
        // (Then trailing_bits: a one, zeros to the byte.)
        assert_eq!(w.trailing(), vec![0b1011_1111, 0b1111_0000]);
    }

    #[test]
    fn an_obu_says_its_size_in_the_bytes_asked() {
        assert_eq!(obu(3, &[9, 9], 1), vec![0x1a, 2, 9, 9]);
        // Padded: 2 as four bytes of leb128.
        assert_eq!(obu(3, &[9, 9], 4), vec![0x1a, 0x82, 0x80, 0x80, 0x00, 9, 9]);
    }

    #[test]
    fn ns_codes_as_av1_reads() {
        // Decoded as the spec reads ns(n), every value comes back.
        for n in 2..70u32 {
            for v in 0..n {
                let mut w = Writer::default();
                ns(&mut w, n, v);
                let bits = w.bits;
                let out = w.out;
                let mut at = 0;
                let mut read = |k: u32| {
                    let mut x = 0u32;
                    for _ in 0..k {
                        x = (x << 1) | u32::from((out[at / 8] >> (7 - at % 8)) & 1);
                        at += 1;
                    }
                    x
                };
                let wbits = 32 - n.leading_zeros();
                let m = (1 << wbits) - n;
                let first = read(wbits - 1);
                let got = if first < m { first } else { (first << 1) - m + read(1) };
                assert_eq!(got, v, "ns({n}) of {v}");
                assert!(bits <= wbits as usize);
            }
        }
    }

    #[test]
    fn tiles_as_av1_allows() {
        // 4K fits one tile; 5K's 80 superblocks across take two columns.
        let t = Tiles::for_size(3840, 2160);
        assert_eq!((t.cols, t.rows, t.uniform), (1, 1, true));
        let t = Tiles::for_size(5120, 2880);
        assert_eq!((t.cols, t.cols_log2), (2, 1));
        assert_eq!(t.widths_sb.iter().sum::<u32>(), 80);
        assert!(t.widths_sb.iter().all(|&w| w <= 64));
    }

    #[test]
    fn levels_by_size_and_rate() {
        assert_eq!(level(1920, 1080, 30), 8);
        assert_eq!(level(1920, 1080, 60), 9);
        assert_eq!(level(3840, 2160, 60), 13);
        // (5K's 14.7 million pixels are past level 5's most.)
        assert_eq!(level(5120, 2880, 60), 16);
    }
}
