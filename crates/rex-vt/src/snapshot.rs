//! Incremental `GHOSTSNP` binary snapshot decoder and encoder over `libghostty-vt`.

use std::{ffi::c_void, pin::Pin, ptr};

use crate::{
    error::{VtError, check},
    ffi,
    terminal::{GhosttyTerminal, ScreenKind},
};

/// Progress emitted by a single [`SnapshotDecoder::next`] step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotProgress {
    /// One scrollback history page was restored.
    Page { screen: ScreenKind, rows: usize, remaining: u32 },
    /// One Kitty graphics image or animation frame's pixels were restored.
    Image { screen: ScreenKind, image_id: u32, frame: u32, bytes: usize },
}

/// Incremental `GHOSTSNP` snapshot decoder backed by a caller-owned byte buffer.
///
/// The buffer is pinned for the lifetime of the decoder because
/// `ghostty_snapshot_decoder_new_buf` borrows it directly without copying.
pub struct SnapshotDecoder {
    raw: ffi::GhosttySnapshotDecoder,
    // Kept alive & immovable while `raw` borrows its pointer.
    _buf: Pin<Box<[u8]>>,
    ready_called: bool,
}

// `GhosttySnapshotDecoder` is not concurrently shared; ownership can move across threads.
unsafe impl Send for SnapshotDecoder {}

impl SnapshotDecoder {
    /// Create a decoder over `bytes`.
    ///
    /// By default, `COMPRESS_HISTORY` is enabled so large scrollback snapshots
    /// never cause a memory spike during restoration.
    pub fn new(bytes: Vec<u8>, compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<Self, VtError> {
        crate::sys::init();
        let pinned = Pin::new(bytes.into_boxed_slice());
        let mut raw: ffi::GhosttySnapshotDecoder = ptr::null_mut();
        check(unsafe { ffi::ghostty_snapshot_decoder_new_buf(ptr::null(), &mut raw, pinned.as_ptr(), pinned.len()) })?;

        if compress_history {
            let v = true;
            let _ = unsafe { ffi::ghostty_snapshot_decoder_set(raw, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY, (&v as *const bool).cast::<c_void>()) };
        }
        if let Some(limit) = kitty_storage_limit {
            let _ = unsafe { ffi::ghostty_snapshot_decoder_set(raw, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT, (&limit as *const u64).cast::<c_void>()) };
        }

        Ok(Self { raw, _buf: pinned, ready_called: false })
    }

    /// Decode and validate the immediately renderable viewport prefix through
    /// `READY`, returning a live [`GhosttyTerminal`].
    ///
    /// Keep the returned terminal alive while calling [`Self::next`].
    pub fn ready(&mut self) -> Result<GhosttyTerminal, VtError> {
        if self.ready_called {
            return Err(VtError::InvalidValue);
        }
        let mut term: ffi::GhosttyTerminal = ptr::null_mut();
        check(unsafe { ffi::ghostty_snapshot_decoder_ready(self.raw, &mut term) })?;
        self.ready_called = true;
        Ok(unsafe { GhosttyTerminal::from_raw(term) })
    }

    /// Decode the next incremental record (history page or Kitty image pixels).
    /// Returns `Ok(None)` when `FINISH` is reached.
    ///
    /// SAFETY contract: the caller must still own/keep alive the
    /// [`GhosttyTerminal`] returned by [`Self::ready`] and serialize access
    /// with any terminal mutation.
    pub fn next(&mut self) -> Result<Option<SnapshotProgress>, VtError> {
        let res = unsafe { ffi::ghostty_snapshot_decoder_next(self.raw) };
        if res == ffi::GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check(res)?;

        let mut kind: ffi::GhosttySnapshotDecoderProgressKind = ffi::GHOSTTY_SNAPSHOT_DECODER_PROGRESS_KIND_PAGE;
        check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_KIND, ptr::from_mut(&mut kind).cast::<c_void>()) })?;

        let mut raw_screen: ffi::GhosttyTerminalScreen = ffi::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_SCREEN, ptr::from_mut(&mut raw_screen).cast::<c_void>()) };
        let screen = ScreenKind::from_ffi(raw_screen);

        if kind == ffi::GHOSTTY_SNAPSHOT_DECODER_PROGRESS_KIND_IMAGE {
            let mut image_id = 0u32;
            let mut frame = 0u32;
            let mut bytes = 0usize;
            let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_ID, ptr::from_mut(&mut image_id).cast::<c_void>()) };
            let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_FRAME, ptr::from_mut(&mut frame).cast::<c_void>()) };
            let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_BYTES, ptr::from_mut(&mut bytes).cast::<c_void>()) };
            Ok(Some(SnapshotProgress::Image { screen, image_id, frame, bytes }))
        } else {
            let mut rows = 0usize;
            let mut remaining = 0u32;
            let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS, ptr::from_mut(&mut rows).cast::<c_void>()) };
            let _ = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_REMAINING, ptr::from_mut(&mut remaining).cast::<c_void>()) };
            Ok(Some(SnapshotProgress::Page { screen, rows, remaining }))
        }
    }

    /// Number of bytes consumed from the snapshot buffer so far. After
    /// [`Self::next`] returns `Ok(None)` (`FINISH`), this is the exact offset
    /// of the first live VT byte trailing the snapshot.
    pub fn source_offset(&self) -> Result<usize, VtError> {
        let mut offset = 0usize;
        check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET, ptr::from_mut(&mut offset).cast::<c_void>()) })?;
        Ok(offset)
    }

    /// Advisory total scrollback rows for the primary screen (valid after `ready`).
    pub fn history_rows_primary(&self) -> Option<u64> {
        let mut rows = 0u64;
        let res = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_HISTORY_ROWS_PRIMARY, ptr::from_mut(&mut rows).cast::<c_void>()) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(rows)
    }

    /// Decode a complete snapshot in one shot, returning the restored terminal
    /// and the exact byte offset where trailing live VT bytes begin.
    pub fn decode_all(bytes: Vec<u8>, compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<(GhosttyTerminal, usize), VtError> {
        let mut dec = Self::new(bytes, compress_history, kitty_storage_limit)?;
        let term = dec.ready()?;
        while dec.next()?.is_some() {}
        let offset = dec.source_offset()?;
        Ok((term, offset))
    }
}

impl Drop for SnapshotDecoder {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { ffi::ghostty_snapshot_decoder_free(self.raw) };
        }
    }
}

/// Streaming snapshot reader that buffers incoming stream bytes until the
/// `GHOSTSNP` snapshot `READY` and `FINISH` markers are completely parsed.
///
/// Rex's `rex.data.terminal.v1` stream starts with one `GHOSTSNP` binary
/// snapshot followed immediately by live VT bytes (including inline
/// `ESC _ rex1;<nonce>;<json> ESC \` APCs). Using `ghostty_snapshot_decoder_new`
/// with a custom `GhosttyReader` lets us probe incrementally or decode as soon
/// as a complete snapshot arrives without guessing frame boundaries.
pub struct StreamingSnapshotResult {
    pub terminal: GhosttyTerminal,
    /// Exact byte offset inside the supplied buffer where post-snapshot live VT
    /// bytes begin.
    pub consumed: usize,
}

/// Attempt to decode a `GHOSTSNP` snapshot from the start of `buf`.
///
/// Returns:
/// - `Ok(Some(StreamingSnapshotResult))` if a full snapshot was decoded,
/// - `Ok(None)` if `buf` is a truncated prefix of a valid snapshot (more bytes needed),
/// - `Err(VtError)` if `buf` is corrupt or does not begin with `GHOSTSNP`.
pub fn try_decode_snapshot_prefix(buf: &[u8], compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<Option<StreamingSnapshotResult>, VtError> {
    if buf.len() < 8 {
        if b"GHOSTSNP".starts_with(buf) {
            return Ok(None);
        }
        return Err(VtError::InvalidValue);
    }
    if !buf.starts_with(b"GHOSTSNP") {
        return Err(VtError::InvalidValue);
    }

    crate::sys::init();

    // Custom reader that distinguishes EOF-before-FINISH (returns false / IO_ERROR
    // after marking `truncated = true`) from corrupt snapshot bytes.
    struct PrefixReader<'a> {
        data: &'a [u8],
        pos: usize,
        hit_eof: bool,
    }

    unsafe extern "C" fn read_cb(userdata: *mut c_void, buf: *mut u8, len: usize, out_read: *mut usize) -> bool {
        let state = unsafe { &mut *userdata.cast::<PrefixReader<'_>>() };
        let avail = state.data.len().saturating_sub(state.pos);
        if avail == 0 {
            state.hit_eof = true;
            unsafe { *out_read = 0 };
            return false;
        }
        let n = avail.min(len);
        unsafe {
            ptr::copy_nonoverlapping(state.data.as_ptr().add(state.pos), buf, n);
            *out_read = n;
        }
        state.pos += n;
        true
    }

    let mut state = PrefixReader { data: buf, pos: 0, hit_eof: false };
    let reader = ffi::GhosttyReader { userdata: (&mut state as *mut PrefixReader<'_>).cast::<c_void>(), read: Some(read_cb) };

    let mut raw: ffi::GhosttySnapshotDecoder = ptr::null_mut();
    check(unsafe { ffi::ghostty_snapshot_decoder_new(ptr::null(), &mut raw, reader) })?;

    struct DecoderGuard(ffi::GhosttySnapshotDecoder);
    impl Drop for DecoderGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ffi::ghostty_snapshot_decoder_free(self.0) };
            }
        }
    }
    let guard = DecoderGuard(raw);

    if compress_history {
        let v = true;
        let _ = unsafe { ffi::ghostty_snapshot_decoder_set(guard.0, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY, (&v as *const bool).cast::<c_void>()) };
    }
    if let Some(limit) = kitty_storage_limit {
        let _ = unsafe { ffi::ghostty_snapshot_decoder_set(guard.0, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT, (&limit as *const u64).cast::<c_void>()) };
    }

    let mut term_raw: ffi::GhosttyTerminal = ptr::null_mut();
    let res = unsafe { ffi::ghostty_snapshot_decoder_ready(guard.0, &mut term_raw) };
    if res != ffi::GHOSTTY_SUCCESS {
        if state.hit_eof {
            return Ok(None);
        }
        return Err(VtError::from_ffi(res));
    }

    let term = unsafe { GhosttyTerminal::from_raw(term_raw) };
    loop {
        let next_res = unsafe { ffi::ghostty_snapshot_decoder_next(guard.0) };
        if next_res == ffi::GHOSTTY_NO_VALUE {
            break;
        }
        if next_res != ffi::GHOSTTY_SUCCESS {
            if state.hit_eof {
                return Ok(None);
            }
            return Err(VtError::from_ffi(next_res));
        }
    }

    let mut offset = 0usize;
    check(unsafe { ffi::ghostty_snapshot_decoder_get(guard.0, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET, ptr::from_mut(&mut offset).cast::<c_void>()) })?;

    Ok(Some(StreamingSnapshotResult { terminal: term, consumed: offset }))
}
