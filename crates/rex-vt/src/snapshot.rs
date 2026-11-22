//! Incremental `GHOSTSNP` binary snapshot decoder and encoder over `libghostty-vt`.

use std::{borrow::Cow, ffi::c_void, ptr, sync::Arc};

use crate::{
    error::{VtError, check},
    ffi,
    terminal::{GhosttyTerminal, ScreenKind, Shared},
};

/// Progress emitted by a single [`SnapshotDecoder::next`] step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotProgress {
    /// One scrollback history page was restored.
    Page { screen: ScreenKind, rows: usize, remaining: u32 },
    /// One Kitty graphics image or animation frame's pixels were restored.
    Image { screen: ScreenKind, image_id: u32, frame: u32, bytes: usize },
}

/// Maximum wire size of one snapshot, excluding trailing live VT bytes.
pub const MAX_SNAPSHOT_BYTES: usize = crate::limits::SNAPSHOT_BYTES;

/// Finds the FINISH offset as transport chunks arrive. Keep one per stream and
/// pass its accumulated, append-only buffer to `advance`. Each complete record
/// is visited once. Restoration still validates the full prefix independently.
#[derive(Default)]
pub struct SnapshotFramer {
    end: usize,
    page_budget: u64,
    records: u32,
    complete: bool,
}

impl SnapshotFramer {
    pub fn advance(&mut self, bytes: &[u8]) -> Result<Option<usize>, VtError> {
        use crate::limits;
        const ENVELOPE: &[u8] = b"GHOSTSNP\x01\x00";
        let prefix = ENVELOPE.len().min(bytes.len());
        if bytes[..prefix] != ENVELOPE[..prefix] {
            return Err(VtError::InvalidValue);
        }
        if bytes.len() < self.end {
            return Err(VtError::InvalidValue);
        }
        if self.complete {
            return Ok(Some(self.end));
        }
        self.end = self.end.max(prefix);
        while let Some(header) = bytes[self.end..].first_chunk::<10>() {
            let tag = u16::from_le_bytes([header[0], header[1]]);
            let len = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
            let start = self.end + 10;
            let end = start.checked_add(len).ok_or(VtError::LimitExceeded)?;
            if end > limits::SNAPSHOT_BYTES || self.records >= 65_536 {
                return Err(VtError::LimitExceeded);
            }
            if matches!(tag, 5 | 6) && len != 0 {
                return Err(VtError::InvalidValue);
            }
            let Some(payload) = bytes.get(start..end) else {
                break;
            };
            match tag {
                1 | 3 => {
                    let &[a, b, c, d] = payload.first_chunk::<4>().ok_or(VtError::InvalidValue)?;
                    let cols = u16::from_le_bytes([a, b]);
                    let rows = u16::from_le_bytes([c, d]);
                    if tag == 1 {
                        limits::grid(cols, rows)?;
                    } else {
                        // Page mappings bypass the native heap. Charge cells, table
                        // capacity hints and a normal native page (about 400 KiB) even for
                        // a mostly empty grid whose wire form is only a few bytes.
                        let h = payload.first_chunk::<20>().ok_or(VtError::InvalidValue)?;
                        let cells = u64::from(cols) * u64::from(rows);
                        let styles = u16::from_le_bytes([h[8], h[9]]);
                        let links = u16::from_le_bytes([h[10], h[11]]);
                        let graphemes = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
                        let strings = u32::from_le_bytes([h[16], h[17], h[18], h[19]]);
                        let charge = (cells * 16 + u64::from(styles) * 64 + u64::from(links) * 8 + u64::from(graphemes) * 2 + u64::from(strings) * 2).max(400 << 10);
                        if charge > limits::HEAP_BYTES as u64 - self.page_budget {
                            return Err(VtError::LimitExceeded);
                        }
                        self.page_budget += charge;
                    }
                }
                2 => {
                    // A screen declares history extents before sending those pages.
                    let h = payload.first_chunk::<12>().ok_or(VtError::InvalidValue)?;
                    let pages = u16::from_le_bytes([h[2], h[3]]);
                    let history_rows = u64::from_le_bytes(h[4..12].try_into().unwrap());
                    if pages > 4096 || history_rows > 16 * 1024 * 1024 {
                        return Err(VtError::LimitExceeded);
                    }
                }
                4 => {
                    let h = payload.first_chunk::<6>().ok_or(VtError::InvalidValue)?;
                    let pages = u32::from_le_bytes([h[2], h[3], h[4], h[5]]);
                    if pages > 4096 {
                        return Err(VtError::LimitExceeded);
                    }
                }
                6 => {
                    self.complete = true;
                    self.end = end;
                    return Ok(Some(end));
                }
                _ => {}
            }
            self.end = end;
            self.records += 1;
        }
        if bytes.len() >= limits::SNAPSHOT_BYTES {
            return Err(VtError::LimitExceeded);
        }
        Ok(None)
    }
}

/// Owned buffer for stepwise restoration. The decoder keeps a terminal owner
/// until FINISH or cancellation, and uses the same lock as readers and writers.
pub struct SnapshotDecoder(Decoder<'static>);
struct Decoder<'a> {
    raw: ffi::GhosttySnapshotDecoder,
    bytes: Cow<'a, [u8]>,
    shared: Arc<Shared>,
    terminal: Option<GhosttyTerminal>,
    failed: bool,
    finished: bool,
}

// SAFETY: the heap buffer is immutable and remains at the same address after
// a move. The native handle is used exclusively and terminal access serialized.
unsafe impl Send for Decoder<'_> {}

impl<'a> Decoder<'a> {
    fn new(bytes: Cow<'a, [u8]>, compress: bool, image_limit: Option<u64>) -> Result<Self, VtError> {
        crate::sys::init()?;
        let shared = Shared::new();
        let mut raw = ptr::null_mut();
        // SAFETY: bytes owns or borrows a stable slice until Drop; shared keeps the allocator live.
        check(unsafe { ffi::ghostty_snapshot_decoder_new_buf(shared.budget.raw(), &mut raw, bytes.as_ptr(), bytes.len()) })?;
        if raw.is_null() {
            return Err(VtError::InvalidValue);
        }
        let dec = Self { raw, bytes, shared, terminal: None, failed: false, finished: false };
        // SAFETY: snapshot.h specifies a bool and a uint64_t, both copied before decoding starts.
        unsafe {
            check(ffi::ghostty_snapshot_decoder_set(raw, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY, ptr::from_ref(&compress).cast()))?;
            if let Some(limit) = image_limit {
                if limit > crate::limits::IMAGE_BYTES as u64 {
                    return Err(VtError::LimitExceeded);
                }
                check(ffi::ghostty_snapshot_decoder_set(raw, ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT, ptr::from_ref(&limit).cast()))?;
            }
        }
        Ok(dec)
    }

    fn ready(&mut self) -> Result<GhosttyTerminal, VtError> {
        if self.failed || self.terminal.is_some() || self.finished {
            return Err(VtError::InvalidValue);
        }
        self.failed = true;
        let mut raw = ptr::null_mut();
        // SAFETY: the fresh decoder writes a caller-owned terminal on success.
        check(unsafe { ffi::ghostty_snapshot_decoder_ready(self.raw, &mut raw) })?;
        let term = GhosttyTerminal::owned(raw, self.shared.clone())?;
        self.terminal = Some(term.retain_for_decoder());
        // Remote snapshots carry the sender's scrollback policy, which can be unlimited.
        // Keep any smaller limit. Both getter and setter take size_t*.
        let mut limit = 0usize;
        // SAFETY: this is a fresh, unexposed terminal; the output type is size_t.
        let result = unsafe { ffi::ghostty_terminal_get(raw, ffi::GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES, ptr::from_mut(&mut limit).cast()) };
        if result != ffi::GHOSTTY_NO_VALUE {
            check(result)?;
        }
        if result == ffi::GHOSTTY_NO_VALUE || limit > crate::limits::HISTORY_BYTES {
            // SAFETY: the option copies the local byte limit immediately.
            check(unsafe { ffi::ghostty_terminal_set(raw, ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, ptr::from_ref(&crate::limits::HISTORY_BYTES).cast()) })?;
        }
        self.failed = false;
        Ok(term)
    }

    /// Decode the next incremental record (history page or Kitty image pixels).
    /// Returns `Ok(None)` when `FINISH` is reached.
    fn next(&mut self) -> Result<Option<SnapshotProgress>, VtError> {
        if self.finished {
            return Ok(None);
        }
        if self.failed || self.terminal.is_none() {
            return Err(VtError::InvalidValue);
        }
        self.failed = true;
        let res = {
            let _access = self.shared.change();
            // SAFETY: self retains the READY terminal; its lock covers the native mutation.
            unsafe { ffi::ghostty_snapshot_decoder_next(self.raw) }
        };
        if res == ffi::GHOSTTY_NO_VALUE {
            self.finished = true;
            self.failed = false;
            self.terminal.take();
            return Ok(None);
        }
        check(res)?;

        let mut kind: ffi::GhosttySnapshotDecoderProgressKind = ffi::GHOSTTY_SNAPSHOT_DECODER_PROGRESS_KIND_PAGE;
        // SAFETY: The previous step succeeded; the enum output matches this progress field in snapshot.h.
        check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_KIND, ptr::from_mut(&mut kind).cast::<c_void>()) })?;

        let mut raw_screen: ffi::GhosttyTerminalScreen = ffi::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        // SAFETY: The previous step succeeded; the enum output matches this progress field in snapshot.h.
        check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_SCREEN, ptr::from_mut(&mut raw_screen).cast::<c_void>()) })?;
        let screen = ScreenKind::from_ffi(raw_screen);

        if kind == ffi::GHOSTTY_SNAPSHOT_DECODER_PROGRESS_KIND_IMAGE {
            let mut image_id = 0u32;
            let mut frame = 0u32;
            let mut bytes = 0usize;
            // SAFETY: IMAGE progress provides a uint32_t output.
            check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_ID, ptr::from_mut(&mut image_id).cast::<c_void>()) })?;
            // SAFETY: IMAGE progress provides a uint32_t output.
            check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_FRAME, ptr::from_mut(&mut frame).cast::<c_void>()) })?;
            // SAFETY: IMAGE_BYTES writes size_t to the local usize.
            check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_IMAGE_BYTES, ptr::from_mut(&mut bytes).cast::<c_void>()) })?;
            self.failed = false;
            Ok(Some(SnapshotProgress::Image { screen, image_id, frame, bytes }))
        } else if kind == ffi::GHOSTTY_SNAPSHOT_DECODER_PROGRESS_KIND_PAGE {
            let mut rows = 0usize;
            let mut remaining = 0u32;
            // SAFETY: ROWS writes size_t after a PAGE step.
            check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS, ptr::from_mut(&mut rows).cast::<c_void>()) })?;
            // SAFETY: REMAINING writes uint32_t after a PAGE step.
            check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_REMAINING, ptr::from_mut(&mut remaining).cast::<c_void>()) })?;
            self.failed = false;
            Ok(Some(SnapshotProgress::Page { screen, rows, remaining }))
        } else {
            Err(VtError::Unsupported)
        }
    }

    /// Number of bytes consumed from the snapshot buffer so far. After
    /// [`Self::next`] returns `Ok(None)` (`FINISH`), this is the exact offset
    /// of the first live VT byte trailing the snapshot.
    pub fn source_offset(&self) -> Result<usize, VtError> {
        if self.failed {
            return Err(VtError::InvalidValue);
        }
        let mut offset = 0usize;
        // SAFETY: The decoder is live and unpoisoned; snapshot.h specifies the local output's integer type.
        check(unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET, ptr::from_mut(&mut offset).cast::<c_void>()) })?;
        if offset > self.bytes.len() {
            return Err(VtError::InvalidValue);
        }
        Ok(offset)
    }

    /// Advisory total scrollback rows for the primary screen (valid after `ready`).
    pub fn history_rows_primary(&self) -> Option<u64> {
        if self.failed {
            return None;
        }
        let mut rows = 0u64;
        // SAFETY: The decoder is live and unpoisoned; snapshot.h specifies the local output's integer type.
        let res = unsafe { ffi::ghostty_snapshot_decoder_get(self.raw, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_HISTORY_ROWS_PRIMARY, ptr::from_mut(&mut rows).cast::<c_void>()) };
        (res == ffi::GHOSTTY_SUCCESS).then_some(rows)
    }
}

impl Drop for Decoder<'_> {
    fn drop(&mut self) {
        let _access = self.shared.lock();
        // SAFETY: decoder is freed exactly once, before its buffer, allocator and retained terminal.
        unsafe { ffi::ghostty_snapshot_decoder_free(self.raw) };
    }
}

impl SnapshotDecoder {
    pub fn new(mut bytes: Vec<u8>, compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<Self, VtError> {
        let mut framing = SnapshotFramer::default();
        framing.advance(&bytes)?;
        bytes.truncate(framing.end);
        // Do not retain capacity belonging to a large trailing live-data frame.
        Ok(Self(Decoder::new(Cow::Owned(bytes.into_boxed_slice().into_vec()), compress_history, kitty_storage_limit)?))
    }
    /// Restore through READY. The terminal can be used or dropped between steps.
    pub fn ready(&mut self) -> Result<GhosttyTerminal, VtError> {
        self.0.ready()
    }
    /// Restore one history page or image. FINISH is idempotent.
    pub fn next(&mut self) -> Result<Option<SnapshotProgress>, VtError> {
        self.0.next()
    }
    pub fn source_offset(&self) -> Result<usize, VtError> {
        self.0.source_offset()
    }
    pub fn history_rows_primary(&self) -> Option<u64> {
        self.0.history_rows_primary()
    }
    pub fn decode_all(bytes: Vec<u8>, compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<(GhosttyTerminal, usize), VtError> {
        let mut decoder = Self::new(bytes, compress_history, kitty_storage_limit)?;
        let terminal = decoder.ready()?;
        while decoder.next()?.is_some() {}
        Ok((terminal, decoder.source_offset()?))
    }
}

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
/// - `Ok(None)` if framing needs more bytes. Ghostty validates record contents
///   and checksums once FINISH is available,
/// - `Err(VtError)` if `buf` is corrupt or does not begin with `GHOSTSNP`.
pub fn try_decode_snapshot_prefix(buf: &[u8], compress_history: bool, kitty_storage_limit: Option<u64>) -> Result<Option<StreamingSnapshotResult>, VtError> {
    let Some(end) = SnapshotFramer::default().advance(buf)? else {
        return Ok(None);
    };
    let mut decoder = Decoder::new(Cow::Borrowed(&buf[..end]), compress_history, kitty_storage_limit)?;
    let terminal = decoder.ready()?;
    while decoder.next()?.is_some() {}
    let consumed = decoder.source_offset()?;
    if consumed != end {
        return Err(VtError::InvalidValue);
    }
    Ok(Some(StreamingSnapshotResult { terminal, consumed }))
}

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn page_reservations_and_announced_history_are_bounded_before_decode() {
        fn record(tag: u16, payload: &[u8]) -> Result<Option<usize>, VtError> {
            let mut bytes = b"GHOSTSNP\x01\0".to_vec();
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(payload);
            SnapshotFramer::default().advance(&bytes)
        }
        let mut page = [0; 20];
        page[..4].copy_from_slice(&[80, 0, 1, 0]);
        page[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(record(3, &page), Err(VtError::LimitExceeded));
        assert_eq!(record(2, &[255; 12]), Err(VtError::LimitExceeded));
        assert_eq!(record(4, &[255; 6]), Err(VtError::LimitExceeded));
    }
}
