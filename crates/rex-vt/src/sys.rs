//! Process-global Ghostty callbacks. Registration completes before any engine
//! object is made. The callbacks use only the arguments of the current call.

use std::{ffi::c_void, io::Cursor, panic::catch_unwind, ptr, slice, sync::OnceLock};

use image::{DynamicImage, ImageDecoder, Limits, codecs::png::PngDecoder};

use crate::{
    error::{VtError, check},
    ffi, limits,
};

static INIT: OnceLock<Result<(), VtError>> = OnceLock::new();

pub fn init() -> Result<(), VtError> {
    *INIT.get_or_init(|| {
        let png: ffi::GhosttySysDecodePngFn = Some(decode_png_cb);
        let log: ffi::GhosttySysLogFn = Some(log_cb);
        // SAFETY: sys.h passes callback addresses directly (as in the stderr
        // example). Both callbacks have the ABI checked above and static lifetime.
        unsafe {
            check(ffi::ghostty_sys_set(ffi::GHOSTTY_SYS_OPT_DECODE_PNG, png.map_or(ptr::null(), |f| f as *const c_void)))?;
            check(ffi::ghostty_sys_set(ffi::GHOSTTY_SYS_OPT_LOG, log.map_or(ptr::null(), |f| f as *const c_void)))
        }
    })
}

fn png(bytes: &[u8]) -> Option<image::RgbaImage> {
    let mut budget = Limits::default();
    budget.max_alloc = Some(limits::IMAGE_BYTES as u64);
    budget.max_image_width = Some(16_384);
    budget.max_image_height = Some(16_384);
    let decoder = PngDecoder::with_limits(Cursor::new(bytes), budget).ok()?;
    let (width, height) = decoder.dimensions();
    limits::rgba_len(width, height)?;
    if decoder.total_bytes() > limits::IMAGE_BYTES as u64 {
        return None;
    }
    Some(DynamicImage::from_decoder(decoder).ok()?.into_rgba8())
}

unsafe extern "C" fn decode_png_cb(_: *mut c_void, allocator: *const ffi::GhosttyAllocator, data: *const u8, len: usize, out: *mut ffi::GhosttySysImage) -> bool {
    if data.is_null() || out.is_null() || len == 0 || len > limits::IMAGE_BYTES {
        return false;
    }
    let Ok(Some(image)) = catch_unwind(|| {
        // SAFETY: sys.h lends len initialized bytes for this synchronous callback.
        png(unsafe { slice::from_raw_parts(data, len) })
    }) else {
        return false;
    };
    let (width, height) = image.dimensions();
    let bytes = image.as_raw();
    // SAFETY: the library supplies the allocator for this image, including its
    // own limit. On success it takes ownership of this exact allocation.
    unsafe {
        let dst = ffi::ghostty_alloc(allocator, bytes.len());
        if dst.is_null() {
            return false;
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        out.write(ffi::GhosttySysImage { width, height, data: dst, data_len: bytes.len() });
    }
    true
}

unsafe extern "C" fn log_cb(_: *mut c_void, level: ffi::GhosttySysLogLevel, scope: *const u8, scope_len: usize, message: *const u8, message_len: usize) {
    // A tracing subscriber is application code. Its panic must end here.
    let _ = catch_unwind(|| {
        let scope = if scope.is_null() || scope_len == 0 {
            "ghostty"
        } else {
            // SAFETY: sys.h lends scope_len bytes for the duration of the callback.
            std::str::from_utf8(unsafe { slice::from_raw_parts(scope, scope_len) }).unwrap_or("ghostty")
        };
        let message = if message.is_null() || message_len == 0 {
            ""
        } else {
            // SAFETY: the same callback contract supplies message_len message bytes.
            std::str::from_utf8(unsafe { slice::from_raw_parts(message, message_len) }).unwrap_or("")
        };
        match level {
            ffi::GHOSTTY_SYS_LOG_LEVEL_ERROR => tracing::warn!(target: "ghostty_vt", scope, "{message}"),
            ffi::GHOSTTY_SYS_LOG_LEVEL_WARNING => tracing::debug!(target: "ghostty_vt", scope, "{message}"),
            _ => tracing::trace!(target: "ghostty_vt", scope, "{message}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::{Event, Metadata, Subscriber, span};

    #[test]
    fn a_panicking_log_subscriber_cannot_unwind_through_ghostty() {
        struct Panics;
        impl Subscriber for Panics {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }
            fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
            fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
            fn enter(&self, _: &span::Id) {}
            fn exit(&self, _: &span::Id) {}
            fn event(&self, _: &Event<'_>) {
                panic!("subscriber failed");
            }
        }
        tracing::subscriber::with_default(Panics, || {
            // SAFETY: empty scope and one borrowed message byte follow the log callback ABI.
            unsafe { log_cb(ptr::null_mut(), ffi::GHOSTTY_SYS_LOG_LEVEL_ERROR, ptr::null(), 0, b"x".as_ptr(), 1) };
        });
    }
}
