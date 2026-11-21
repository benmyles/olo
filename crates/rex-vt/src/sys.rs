//! Process-global `libghostty-vt` system callbacks (PNG decoding via the
//! `image` crate and structured logging via `tracing`).

use std::{ffi::c_void, slice, sync::Once};

use crate::ffi;

static INIT: Once = Once::new();

/// Initialize `libghostty-vt`'s process-global system callbacks (PNG decoder
/// for Kitty graphics and tracing logger). Safe to call repeatedly; runs once.
pub fn init() {
    INIT.call_once(|| unsafe {
        let decode_fn: ffi::GhosttySysDecodePngFn = Some(decode_png_cb);
        let _ = ffi::ghostty_sys_set(ffi::GHOSTTY_SYS_OPT_DECODE_PNG, (&decode_fn as *const ffi::GhosttySysDecodePngFn).cast::<c_void>());
        let log_fn: ffi::GhosttySysLogFn = Some(log_cb);
        let _ = ffi::ghostty_sys_set(ffi::GHOSTTY_SYS_OPT_LOG, (&log_fn as *const ffi::GhosttySysLogFn).cast::<c_void>());
    });
}

unsafe extern "C" fn decode_png_cb(_userdata: *mut c_void, allocator: *const ffi::GhosttyAllocator, data: *const u8, data_len: usize, out: *mut ffi::GhosttySysImage) -> bool {
    if data.is_null() || data_len == 0 || out.is_null() {
        return false;
    }
    let bytes = unsafe { slice::from_raw_parts(data, data_len) };
    let Ok(img) = image::load_from_memory_with_format(bytes, image::ImageFormat::Png) else {
        return false;
    };
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let raw = rgba.as_raw();
    let len = raw.len();
    if len == 0 {
        return false;
    }
    let dst = unsafe { ffi::ghostty_alloc(allocator, len) }.cast::<u8>();
    if dst.is_null() {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(raw.as_ptr(), dst, len);
        (*out).width = width;
        (*out).height = height;
        (*out).data = dst;
        (*out).data_len = len;
    }
    true
}

unsafe extern "C" fn log_cb(_userdata: *mut c_void, level: ffi::GhosttySysLogLevel, scope: *const u8, scope_len: usize, message: *const u8, message_len: usize) {
    let scope_str = if !scope.is_null() && scope_len > 0 { std::str::from_utf8(unsafe { slice::from_raw_parts(scope, scope_len) }).unwrap_or("ghostty") } else { "ghostty" };
    let msg_str = if !message.is_null() && message_len > 0 { std::str::from_utf8(unsafe { slice::from_raw_parts(message, message_len) }).unwrap_or("") } else { "" };
    match level {
        ffi::GHOSTTY_SYS_LOG_LEVEL_ERROR => tracing::warn!(target: "ghostty_vt", scope = scope_str, "{msg_str}"),
        ffi::GHOSTTY_SYS_LOG_LEVEL_WARNING => tracing::debug!(target: "ghostty_vt", scope = scope_str, "{msg_str}"),
        ffi::GHOSTTY_SYS_LOG_LEVEL_INFO => tracing::trace!(target: "ghostty_vt", scope = scope_str, "{msg_str}"),
        _ => tracing::trace!(target: "ghostty_vt", scope = scope_str, "{msg_str}"),
    }
}
