//! Shared screen frame codecs with no GUI or Wayland dependencies.
//!
//! Used by both `olo-screen-pane` (local playback, scrubbing, export) and
//! `olo-rex-link` (static musl host helper serving recorded frames and live
//! tile streams over Rex).
//!
//! - [`olof`]: the on-disk `.olof` recording format (`OLOFRM1\n`), segment
//!   indexing, [`FrameIndex`], [`Writer`], and incremental [`Player`] /
//!   [`OwnedPlayer`].
//! - [`tiles`]: 64×64 RGBA tile delta encoding/decoding for live streams,
//!   supporting both per-tile and single-stream-per-frame zstd compression.

use std::io::{Read as _, Write as _};

pub mod olof;
pub mod tiles;

pub use olof::{Entry, Frame, FrameIndex, FrameMark, OwnedPlayer, Player, Segment, Writer};
pub use tiles::{EncodedDelta, TileEncoding, TileRect};

/// Maximum allowed local `.olof` frame width or height in pixels (`7680 × 3 = 23040`,
/// fitting in `u16` tile coordinates and preserving every screen size `screens::start`
/// allows).
pub(crate) const MAX_LOCAL_DIM: usize = 24_576;
/// Maximum allowed RGBA buffer size for a local `.olof` frame (1.25 GiB, covering
/// `7680 × 4320 @ 3×` = `23040 × 12960 × 4` ≈ 1.11 GiB).
pub(crate) const MAX_LOCAL_FRAME_BYTES: usize = 1_280 * 1024 * 1024;
/// Maximum allowed byte length of a single `.olof` record on disk.
pub(crate) const MAX_RECORD_BYTES: usize = MAX_LOCAL_FRAME_BYTES + 1024 * 1024;
/// Maximum allowed network `OLOT` frame width or height in pixels.
pub(crate) const MAX_NET_DIM: usize = 16_384;
/// Maximum allowed RGBA buffer size for a single network `OLOT` frame (256 MiB).
pub(crate) const MAX_NET_FRAME_BYTES: usize = 256 * 1024 * 1024;
/// Maximum allowed compressed payload bytes inside a single `OLOT` frame.
pub(crate) const MAX_OLOT_DATA_BYTES: usize = MAX_NET_FRAME_BYTES + 1024 * 1024;
/// Maximum zstd decoder sliding window (`1 << 25` = 32 MiB) so a crafted header
/// cannot make libzstd allocate an unbounded window before yielding bytes.
pub(crate) const MAX_WINDOW_LOG: u32 = 25;

fn checked_frame_bytes_with(width: usize, height: usize, max_dim: usize, max_bytes: usize) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err(format!("frame dimensions {width}x{height} must be non-zero"));
    }
    if width > max_dim || height > max_dim {
        return Err(format!("frame dimensions {width}x{height} exceed limit {max_dim}"));
    }
    let bytes = width.checked_mul(height).and_then(|p| p.checked_mul(4)).ok_or_else(|| "frame dimensions overflow".to_owned())?;
    if bytes > max_bytes {
        return Err(format!("frame size {bytes} bytes exceeds limit {max_bytes}"));
    }
    Ok(bytes)
}

/// Validate local recording/playback frame dimensions and return `width * height * 4`.
pub fn checked_local_frame_bytes(width: usize, height: usize) -> Result<usize, String> {
    checked_frame_bytes_with(width, height, MAX_LOCAL_DIM, MAX_LOCAL_FRAME_BYTES)
}

/// Validate untrusted network `OLOT` frame dimensions and return `width * height * 4`.
pub fn checked_net_frame_bytes(width: usize, height: usize) -> Result<usize, String> {
    checked_frame_bytes_with(width, height, MAX_NET_DIM, MAX_NET_FRAME_BYTES)
}

/// Number of canonical `64 × 64` grid cells covering a `width × height` frame.
pub(crate) fn canonical_grid_count(width: usize, height: usize) -> usize {
    width.div_ceil(olof::TILE) * height.div_ceil(olof::TILE)
}

/// Validate that `(x, y, tw, th)` sits on the canonical `64 × 64` tile grid for a
/// `w × h` frame without duplicating an earlier tile in `seen`, returning its
/// exact RGBA byte length.
pub(crate) fn validate_canonical_tile(w: usize, h: usize, x: usize, y: usize, tw: usize, th: usize, seen: &mut [u64]) -> Result<usize, String> {
    let tile = olof::TILE;
    if tw == 0 || th == 0 {
        return Err(format!("empty tile ({x},{y} {tw}x{th})"));
    }
    if x >= w || y >= h || x.checked_add(tw).is_none_or(|e| e > w) || y.checked_add(th).is_none_or(|e| e > h) {
        return Err(format!("tile ({x},{y} {tw}x{th}) out of bounds for {w}x{h}"));
    }
    if !x.is_multiple_of(tile) || !y.is_multiple_of(tile) {
        return Err(format!("tile ({x},{y} {tw}x{th}) is not aligned to {tile}px grid"));
    }
    let expect_w = tile.min(w - x);
    let expect_h = tile.min(h - y);
    if tw != expect_w || th != expect_h {
        return Err(format!("tile ({x},{y} {tw}x{th}) size differs from canonical {expect_w}x{expect_h}"));
    }
    let cols = w.div_ceil(tile);
    let idx = (y / tile) * cols + (x / tile);
    let word = idx / 64;
    let bit = 1u64 << (idx % 64);
    if seen[word] & bit != 0 {
        return Err(format!("duplicate tile at ({x},{y})"));
    }
    seen[word] |= bit;
    tw.checked_mul(th).and_then(|p| p.checked_mul(4)).ok_or_else(|| "tile byte length overflow".to_owned())
}

/// Compress `raw` with zstd at `level` while capping the encoder's sliding
/// window at [`MAX_WINDOW_LOG`] so the decoder always accepts the output.
pub(crate) fn encode_zstd_bounded(raw: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    let mut enc = zstd::stream::write::Encoder::new(Vec::new(), level)?;
    enc.window_log(MAX_WINDOW_LOG)?;
    enc.write_all(raw)?;
    enc.finish()
}

/// Decompress a zstd block that must expand to exactly `expect` bytes,
/// refusing immediately if the header declares more or the stream expands past
/// `expect` bytes.
pub(crate) fn decode_zstd_exact(compressed: &[u8], expect: usize) -> Result<Vec<u8>, String> {
    if let Some(declared) = zstd::stream::decompressed_size(compressed)
        && declared > expect as u64
    {
        return Err(format!("zstd frame declares {declared} bytes, exceeds expected {expect}"));
    }
    let mut decoder = zstd::stream::read::Decoder::with_buffer(compressed).map_err(|e| format!("zstd decoder init: {e}"))?;
    let _ = decoder.window_log_max(MAX_WINDOW_LOG);
    let mut out = Vec::new();
    // Reserve only a bounded hint proportional to the input so a tiny corrupt
    // block cannot force a large upfront allocation before failing.
    let hint = compressed.len().saturating_mul(8).min(expect).min(4 * 1024 * 1024);
    let _ = out.try_reserve_exact(hint);
    let limit = (expect as u64).checked_add(1).ok_or_else(|| "expected length overflow".to_owned())?;
    decoder.take(limit).read_to_end(&mut out).map_err(|e| format!("zstd decode: {e}"))?;
    if out.len() > expect {
        return Err(format!("decompressed data exceeds expected {expect} bytes"));
    }
    if out.len() != expect {
        return Err(format!("decompressed pixel length {} != expected {expect}", out.len()));
    }
    Ok(out)
}
