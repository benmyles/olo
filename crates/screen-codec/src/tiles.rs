//! 64×64 RGBA tile delta codec for live screen streaming.
//!
//! Supports two wire encodings so the host helper and stream source can choose
//! the better one (see `review1.md` point 9):
//! - [`TileEncoding::PerTile`]: each changed tile is zstd-compressed on its own.
//! - [`TileEncoding::PackedStream`]: all changed tiles' raw RGBA pixels are
//!   concatenated in `rects` order and compressed in a single zstd stream
//!   (fewer frame headers and shared compression context across adjacent tiles).

use serde::{Deserialize, Serialize};

use crate::{
    MAX_OLOT_DATA_BYTES, canonical_grid_count, checked_net_frame_bytes, decode_zstd_exact, encode_zstd_bounded,
    olof::{Frame, TILE},
    validate_canonical_tile,
};

pub const TILE_SIZE: usize = TILE;
pub const MAGIC: &[u8; 4] = b"OLOT";
pub const VERSION: u8 = 1;

/// One dirty 64×64 (or partial edge) rectangle in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl TileRect {
    pub fn byte_len(self) -> usize {
        usize::from(self.width) * usize::from(self.height) * 4
    }
}

/// How tile pixel data inside [`EncodedDelta::data`] is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TileEncoding {
    /// Each tile in `rects` is encoded as `[u32 compressed_len][zstd bytes]`.
    PerTile = 0,
    /// All tiles in `rects` concatenated in order, then compressed as one zstd stream.
    PackedStream = 1,
}

impl TileEncoding {
    pub fn from_u8(v: u8) -> Option<TileEncoding> {
        match v {
            0 => Some(TileEncoding::PerTile),
            1 => Some(TileEncoding::PackedStream),
            _ => None,
        }
    }
}

/// An encoded keyframe or dirty-tile delta ready for wire framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedDelta {
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub encoding: TileEncoding,
    pub rects: Vec<TileRect>,
    pub data: Vec<u8>,
}

/// Encode `frame` against `before` (`None` or mismatched size produces a
/// keyframe) using `encoding` at zstd compression `level`.
pub fn encode_delta(frame: &Frame, before: Option<&Frame>, encoding: TileEncoding, level: i32) -> std::io::Result<EncodedDelta> {
    let expect = checked_net_frame_bytes(frame.width, frame.height).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if frame.rgba.len() != expect {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("frame pixel length {} != expected {expect}", frame.rgba.len())));
    }
    if let Some(b) = before
        && b.width == frame.width
        && b.height == frame.height
        && b.rgba.len() != expect
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("reference frame pixel length {} != expected {expect}", b.rgba.len())));
    }
    let keyframe = before.is_none_or(|b| b.width != frame.width || b.height != frame.height);
    let changed = frame.changed(if keyframe { None } else { before });
    let rects: Vec<TileRect> = changed.iter().map(|&(x, y, w, h)| TileRect { x: x as u16, y: y as u16, width: w as u16, height: h as u16 }).collect();
    let data = if changed.is_empty() {
        Vec::new()
    } else {
        match encoding {
            TileEncoding::PerTile => {
                let mut out = Vec::with_capacity(changed.len() * 512);
                for &r in &changed {
                    let packed = encode_zstd_bounded(&frame.tile(r)[..], level)?;
                    out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
                    out.extend_from_slice(&packed);
                }
                out
            }
            TileEncoding::PackedStream => {
                let raw_len: usize = changed.iter().map(|&(_, _, w, h)| w * h * 4).sum();
                let mut raw = Vec::with_capacity(raw_len);
                for &r in &changed {
                    raw.extend_from_slice(&frame.tile(r));
                }
                encode_zstd_bounded(&raw[..], level)?
            }
        }
    };
    Ok(EncodedDelta { width: frame.width as u32, height: frame.height as u32, keyframe, encoding, rects, data })
}

/// Apply `delta` onto `dst`, resizing `dst` only on a complete keyframe and
/// requiring a matching base frame for non-keyframe deltas.
pub fn apply_delta(delta: &EncodedDelta, dst: &mut Frame) -> Result<(), String> {
    let (w, h) = (delta.width as usize, delta.height as usize);
    let total_bytes = checked_net_frame_bytes(w, h)?;
    let grid_count = canonical_grid_count(w, h);
    if delta.rects.len() > grid_count {
        return Err(format!("tile count {} exceeds grid cell count {grid_count} for {w}x{h}", delta.rects.len()));
    }
    if delta.data.len() > MAX_OLOT_DATA_BYTES {
        return Err(format!("OLOT payload {} bytes exceeds limit {MAX_OLOT_DATA_BYTES}", delta.data.len()));
    }
    if delta.keyframe {
        if delta.rects.len() != grid_count {
            return Err(format!("keyframe has {} tiles, expected full {grid_count} grid tiles for {w}x{h}", delta.rects.len()));
        }
    } else if dst.width != w || dst.height != h || dst.rgba.len() != total_bytes {
        return Err(format!("delta for {w}x{h} requires matching base frame (have {}x{}, {} bytes)", dst.width, dst.height, dst.rgba.len()));
    }
    let mut seen = vec![0u64; grid_count.div_ceil(64)];
    let mut expect_total = 0usize;
    for r in &delta.rects {
        let rx = usize::from(r.x);
        let ry = usize::from(r.y);
        let rw = usize::from(r.width);
        let rh = usize::from(r.height);
        let tile_bytes = validate_canonical_tile(w, h, rx, ry, rw, rh, &mut seen)?;
        expect_total = expect_total.checked_add(tile_bytes).ok_or_else(|| "total tile bytes overflow".to_owned())?;
        if expect_total > total_bytes {
            return Err(format!("tiles total {expect_total} bytes exceeds frame size {total_bytes}"));
        }
    }
    if delta.rects.is_empty() {
        if !delta.data.is_empty() {
            return Err("unexpected tile data with zero rects".to_owned());
        }
        return Ok(());
    }
    match delta.encoding {
        TileEncoding::PerTile => {
            // Validate every inner length against `delta.data` before resizing `dst` or decompressing.
            let mut p = 0usize;
            for _ in &delta.rects {
                let p4 = p.checked_add(4).ok_or_else(|| "per-tile header overflow".to_owned())?;
                let clen_bytes = delta.data.get(p..p4).ok_or_else(|| "truncated per-tile header".to_owned())?;
                let clen = u32::from_le_bytes(clen_bytes.try_into().unwrap()) as usize;
                let end = p4.checked_add(clen).ok_or_else(|| "per-tile payload length overflow".to_owned())?;
                if end > delta.data.len() {
                    return Err("truncated per-tile payload".to_owned());
                }
                p = end;
            }
            if p != delta.data.len() {
                return Err("trailing bytes in per-tile payload".to_owned());
            }
            // Decompress all tiles up front into a bounded staging buffer so a failing tile
            // never leaves `dst` partially overwritten or resized on error.
            let mut decoded = Vec::with_capacity(delta.rects.len());
            let mut p = 0usize;
            for r in &delta.rects {
                let clen = u32::from_le_bytes(delta.data[p..p + 4].try_into().unwrap()) as usize;
                let compressed = &delta.data[p + 4..p + 4 + clen];
                p += 4 + clen;
                let pixels = decode_zstd_exact(compressed, r.byte_len()).map_err(|e| format!("zstd decode tile: {e}"))?;
                decoded.push((r, pixels));
            }
            if delta.keyframe && (dst.width != w || dst.height != h || dst.rgba.len() != total_bytes) {
                *dst = Frame { width: w, height: h, rgba: vec![0; total_bytes] };
            }
            for (r, pixels) in decoded {
                dst.put_tile((usize::from(r.x), usize::from(r.y), usize::from(r.width), usize::from(r.height)), &pixels);
            }
            Ok(())
        }
        TileEncoding::PackedStream => {
            let pixels = decode_zstd_exact(&delta.data[..], expect_total).map_err(|e| format!("zstd decode packed frame: {e}"))?;
            if delta.keyframe && (dst.width != w || dst.height != h || dst.rgba.len() != total_bytes) {
                *dst = Frame { width: w, height: h, rgba: vec![0; total_bytes] };
            }
            let mut p = 0usize;
            for r in &delta.rects {
                let len = r.byte_len();
                dst.put_tile((usize::from(r.x), usize::from(r.y), usize::from(r.width), usize::from(r.height)), &pixels[p..p + len]);
                p += len;
            }
            Ok(())
        }
    }
}

impl EncodedDelta {
    /// Serialize into compact binary wire framing (`OLOT`).
    pub fn encode_bytes(&self) -> Vec<u8> {
        let cap = 24 + self.rects.len() * 8 + self.data.len();
        let mut out = Vec::with_capacity(cap);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(self.encoding as u8);
        out.push(u8::from(self.keyframe));
        out.push(0);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&(self.rects.len() as u32).to_le_bytes());
        for r in &self.rects {
            out.extend_from_slice(&r.x.to_le_bytes());
            out.extend_from_slice(&r.y.to_le_bytes());
            out.extend_from_slice(&r.width.to_le_bytes());
            out.extend_from_slice(&r.height.to_le_bytes());
        }
        out.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Deserialize from compact binary wire framing (`OLOT`), validating
    /// dimensions, canonical tile grid geometry, and payload bounds before
    /// copying compressed tile bytes.
    pub fn decode_bytes(bytes: &[u8]) -> Result<EncodedDelta, String> {
        if bytes.len() < 24 || &bytes[0..4] != MAGIC {
            return Err("not an OLOT tile frame".into());
        }
        if bytes[4] != VERSION {
            return Err(format!("unsupported OLOT version {}", bytes[4]));
        }
        let encoding = TileEncoding::from_u8(bytes[5]).ok_or_else(|| format!("unknown tile encoding {}", bytes[5]))?;
        let keyframe = bytes[6] != 0;
        let width = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let (w, h) = (width as usize, height as usize);
        let total_bytes = checked_net_frame_bytes(w, h)?;
        let grid_count = canonical_grid_count(w, h);
        let rect_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        if rect_count > grid_count {
            return Err(format!("OLOT rect count {rect_count} exceeds grid cell count {grid_count} for {w}x{h}"));
        }
        if keyframe && rect_count != grid_count {
            return Err(format!("OLOT keyframe has {rect_count} tiles, expected full {grid_count} grid tiles"));
        }
        let rects_end = 20usize.checked_add(rect_count.checked_mul(8).ok_or("rect count overflow")?).ok_or("rect header overflow")?;
        let data_len_end = rects_end.checked_add(4).ok_or("data length overflow")?;
        if bytes.len() < data_len_end {
            return Err("truncated OLOT rect list".into());
        }
        let data_len = u32::from_le_bytes(bytes[rects_end..data_len_end].try_into().unwrap()) as usize;
        if data_len > MAX_OLOT_DATA_BYTES {
            return Err(format!("OLOT payload {data_len} bytes exceeds limit {MAX_OLOT_DATA_BYTES}"));
        }
        if rect_count == 0 && data_len != 0 {
            return Err("unexpected OLOT data with zero rects".into());
        }
        let data_end = data_len_end.checked_add(data_len).ok_or("data length overflow")?;
        let data_slice = bytes.get(data_len_end..data_end).ok_or("truncated OLOT data")?;
        if data_end != bytes.len() {
            return Err("trailing bytes in OLOT frame".into());
        }
        let mut seen = vec![0u64; grid_count.div_ceil(64)];
        let mut expect_total = 0usize;
        let mut rects = Vec::with_capacity(rect_count);
        for chunk in bytes[20..rects_end].as_chunks::<8>().0 {
            let r = TileRect {
                x: u16::from_le_bytes([chunk[0], chunk[1]]),
                y: u16::from_le_bytes([chunk[2], chunk[3]]),
                width: u16::from_le_bytes([chunk[4], chunk[5]]),
                height: u16::from_le_bytes([chunk[6], chunk[7]]),
            };
            let tile_bytes = validate_canonical_tile(w, h, usize::from(r.x), usize::from(r.y), usize::from(r.width), usize::from(r.height), &mut seen)?;
            expect_total = expect_total.checked_add(tile_bytes).ok_or_else(|| "total tile bytes overflow".to_owned())?;
            if expect_total > total_bytes {
                return Err(format!("tiles total {expect_total} bytes exceeds frame size {total_bytes}"));
            }
            rects.push(r);
        }
        Ok(EncodedDelta { width, height, keyframe, encoding, rects, data: data_slice.to_vec() })
    }

    /// Stream-decode an `OLOT` payload of declared wire length `exact_len`
    /// directly from `r`, validating the 20-byte prefix, frame dimensions,
    /// canonical tile grid, and payload size *before* allocating `data`, and
    /// growing `data` incrementally in bounded chunks so a lying or truncated
    /// frame length never pre-allocates up to 256 MiB.
    pub fn decode_from_reader<R: std::io::Read>(r: &mut R, exact_len: usize) -> Result<EncodedDelta, String> {
        if exact_len < 24 {
            return Err("not an OLOT tile frame".into());
        }
        let mut head = [0u8; 20];
        r.read_exact(&mut head).map_err(|e| format!("truncated OLOT header: {e}"))?;
        if &head[0..4] != MAGIC {
            return Err("not an OLOT tile frame".into());
        }
        if head[4] != VERSION {
            return Err(format!("unsupported OLOT version {}", head[4]));
        }
        let encoding = TileEncoding::from_u8(head[5]).ok_or_else(|| format!("unknown tile encoding {}", head[5]))?;
        let keyframe = head[6] != 0;
        let width = u32::from_le_bytes(head[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(head[12..16].try_into().unwrap());
        let (w, h) = (width as usize, height as usize);
        let total_bytes = checked_net_frame_bytes(w, h)?;
        let grid_count = canonical_grid_count(w, h);
        let rect_count = u32::from_le_bytes(head[16..20].try_into().unwrap()) as usize;
        if rect_count > grid_count {
            return Err(format!("OLOT rect count {rect_count} exceeds grid cell count {grid_count} for {w}x{h}"));
        }
        if keyframe && rect_count != grid_count {
            return Err(format!("OLOT keyframe has {rect_count} tiles, expected full {grid_count} grid tiles"));
        }
        let rects_bytes = rect_count.checked_mul(8).ok_or("rect count overflow")?;
        let data_len_end = 20usize.checked_add(rects_bytes).and_then(|v| v.checked_add(4)).ok_or("rect header overflow")?;
        if exact_len < data_len_end {
            return Err("truncated OLOT rect list".into());
        }
        let mut rect_buf = vec![0u8; rects_bytes + 4];
        r.read_exact(&mut rect_buf).map_err(|e| format!("truncated OLOT rect list: {e}"))?;
        let data_len = u32::from_le_bytes(rect_buf[rects_bytes..rects_bytes + 4].try_into().unwrap()) as usize;
        if data_len > MAX_OLOT_DATA_BYTES {
            return Err(format!("OLOT payload {data_len} bytes exceeds limit {MAX_OLOT_DATA_BYTES}"));
        }
        if rect_count == 0 && data_len != 0 {
            return Err("unexpected OLOT data with zero rects".into());
        }
        let data_end = data_len_end.checked_add(data_len).ok_or("data length overflow")?;
        if data_end != exact_len {
            return Err(if data_end > exact_len { "truncated OLOT data".into() } else { "trailing bytes in OLOT frame".into() });
        }
        let mut seen = vec![0u64; grid_count.div_ceil(64)];
        let mut expect_total = 0usize;
        let mut rects = Vec::with_capacity(rect_count);
        for chunk in rect_buf[..rects_bytes].as_chunks::<8>().0 {
            let r = TileRect {
                x: u16::from_le_bytes([chunk[0], chunk[1]]),
                y: u16::from_le_bytes([chunk[2], chunk[3]]),
                width: u16::from_le_bytes([chunk[4], chunk[5]]),
                height: u16::from_le_bytes([chunk[6], chunk[7]]),
            };
            let tile_bytes = validate_canonical_tile(w, h, usize::from(r.x), usize::from(r.y), usize::from(r.width), usize::from(r.height), &mut seen)?;
            expect_total = expect_total.checked_add(tile_bytes).ok_or_else(|| "total tile bytes overflow".to_owned())?;
            if expect_total > total_bytes {
                return Err(format!("tiles total {expect_total} bytes exceeds frame size {total_bytes}"));
            }
            rects.push(r);
        }
        // Read compressed tile bytes incrementally in bounded chunks so a peer
        // that drops mid-frame after advertising a huge length never triggers a
        // giant upfront allocation.
        let mut data = Vec::with_capacity(data_len.min(64 << 10));
        let mut rem = data_len;
        let mut chunk = [0u8; 64 << 10];
        while rem > 0 {
            let want = rem.min(chunk.len());
            let n = r.read(&mut chunk[..want]).map_err(|e| format!("truncated OLOT data: {e}"))?;
            if n == 0 {
                return Err("truncated OLOT data".into());
            }
            data.extend_from_slice(&chunk[..n]);
            rem -= n;
        }
        Ok(EncodedDelta { width, height, keyframe, encoding, rects, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_frame(w: usize, h: usize, scroll_y: usize, typed_col: Option<usize>) -> Frame {
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            let line = (y + scroll_y) / 18;
            for x in 0..w {
                let i = (y * w + x) * 4;
                // Light panel background with dark text-like stripes every 18 px.
                let is_text = ((y + scroll_y) % 18) >= 5 && ((y + scroll_y) % 18) <= 13 && x >= 32 && x < w.saturating_sub(32) && ((x + line * 17) % 7 != 0) && ((x / 40 + line) % 5 != 0);
                let v = if is_text { 32u8 } else { 244u8 };
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = v;
                rgba[i + 3] = 255;
            }
        }
        if let Some(col) = typed_col {
            let y0 = 200.min(h / 2);
            for y in y0..(y0 + 16).min(h) {
                for x in col..(col + 10).min(w) {
                    let i = (y * w + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&[30, 80, 200, 255]);
                }
            }
        }
        Frame { width: w, height: h, rgba }
    }

    #[test]
    fn roundtrip_both_encodings_and_resize() {
        for encoding in [TileEncoding::PerTile, TileEncoding::PackedStream] {
            let a = desktop_frame(200, 140, 0, None);
            let b = desktop_frame(200, 140, 0, Some(60));
            let c = desktop_frame(150, 90, 12, None);

            let mut dst = Frame { width: 0, height: 0, rgba: Vec::new() };
            let d1 = EncodedDelta::decode_bytes(&encode_delta(&a, None, encoding, 3).unwrap().encode_bytes()).unwrap();
            assert!(d1.keyframe);
            apply_delta(&d1, &mut dst).unwrap();
            assert_eq!(dst, a);

            let d2 = EncodedDelta::decode_bytes(&encode_delta(&b, Some(&a), encoding, 3).unwrap().encode_bytes()).unwrap();
            assert!(!d2.keyframe);
            assert!(d2.rects.len() < d1.rects.len());
            apply_delta(&d2, &mut dst).unwrap();
            assert_eq!(dst, b);

            // Unchanged frame produces zero rects and leaves dst untouched.
            let d_same = EncodedDelta::decode_bytes(&encode_delta(&b, Some(&b), encoding, 3).unwrap().encode_bytes()).unwrap();
            assert!(d_same.rects.is_empty());
            assert!(d_same.data.is_empty());
            apply_delta(&d_same, &mut dst).unwrap();
            assert_eq!(dst, b);

            // Resize forces keyframe.
            let d3 = EncodedDelta::decode_bytes(&encode_delta(&c, Some(&b), encoding, 3).unwrap().encode_bytes()).unwrap();
            assert!(d3.keyframe);
            apply_delta(&d3, &mut dst).unwrap();
            assert_eq!(dst, c);
        }
    }

    #[test]
    fn packed_stream_vs_per_tile_on_typing_and_scroll() {
        // Synthetic 1440x900 desktop frames for typing (1 tile) and scroll (most tiles).
        let base = desktop_frame(1440, 900, 0, None);
        let typed = desktop_frame(1440, 900, 0, Some(100));
        let scrolled = desktop_frame(1440, 900, 36, None);

        let t0 = std::time::Instant::now();
        let t_per = encode_delta(&typed, Some(&base), TileEncoding::PerTile, 3).unwrap();
        let t_per_us = t0.elapsed().as_micros();
        let t0 = std::time::Instant::now();
        let t_pack = encode_delta(&typed, Some(&base), TileEncoding::PackedStream, 3).unwrap();
        let t_pack_us = t0.elapsed().as_micros();

        let t0 = std::time::Instant::now();
        let s_per = encode_delta(&scrolled, Some(&base), TileEncoding::PerTile, 3).unwrap();
        let s_per_us = t0.elapsed().as_micros();
        let t0 = std::time::Instant::now();
        let s_pack = encode_delta(&scrolled, Some(&base), TileEncoding::PackedStream, 3).unwrap();
        let s_pack_us = t0.elapsed().as_micros();

        let mut dst = base.clone();
        let t0 = std::time::Instant::now();
        apply_delta(&s_per, &mut dst).unwrap();
        let s_per_dec_us = t0.elapsed().as_micros();
        assert_eq!(dst, scrolled);

        let mut dst = base.clone();
        let t0 = std::time::Instant::now();
        apply_delta(&s_pack, &mut dst).unwrap();
        let s_pack_dec_us = t0.elapsed().as_micros();
        assert_eq!(dst, scrolled);

        eprintln!("typing ({} tiles): PerTile {} B ({t_per_us} us) vs PackedStream {} B ({t_pack_us} us)", t_pack.rects.len(), t_per.encode_bytes().len(), t_pack.encode_bytes().len());
        eprintln!(
            "scroll ({} tiles): PerTile {} B (enc {s_per_us} us, dec {s_per_dec_us} us) vs PackedStream {} B (enc {s_pack_us} us, dec {s_pack_dec_us} us)",
            s_pack.rects.len(),
            s_per.encode_bytes().len(),
            s_pack.encode_bytes().len()
        );
        assert!(s_pack.data.len() < s_per.data.len(), "expected PackedStream ({}) < PerTile ({}) on scroll", s_pack.data.len(), s_per.data.len());
        assert!(t_pack.data.len() <= t_per.data.len(), "expected PackedStream ({}) <= PerTile ({}) on single-tile typing", t_pack.data.len(), t_per.data.len());
    }

    #[test]
    fn rejects_out_of_bounds_and_corrupt_wire() {
        let bad = EncodedDelta { width: 64, height: 64, keyframe: false, encoding: TileEncoding::PackedStream, rects: vec![TileRect { x: 60, y: 0, width: 10, height: 10 }], data: vec![] };
        let mut dst = Frame { width: 64, height: 64, rgba: vec![0; 64 * 64 * 4] };
        assert!(apply_delta(&bad, &mut dst).is_err());
        assert!(EncodedDelta::decode_bytes(b"NOPE").is_err());
    }

    #[test]
    fn rejects_zstd_bomb_expanding_past_tile_cap() {
        // Small compressed blocks (~1 KiB) that expand to 8 MiB, far past one 64×64 tile (16 KiB).
        let huge = vec![0u8; 8 * 1024 * 1024];
        let stream_bomb = zstd::encode_all(&huge[..], 3).unwrap();
        let sized_bomb = zstd::bulk::compress(&huge, 3).unwrap();
        assert!(stream_bomb.len() < 4096 && sized_bomb.len() < 4096);

        let rect = TileRect { x: 0, y: 0, width: 64, height: 64 };
        let original = Frame { width: 64, height: 64, rgba: vec![42; 64 * 64 * 4] };

        for bomb in [&stream_bomb, &sized_bomb] {
            // PackedStream bomb
            let mut dst = original.clone();
            let packed = EncodedDelta { width: 64, height: 64, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![rect], data: bomb.clone() };
            let err = apply_delta(&packed, &mut dst).unwrap_err();
            assert!(err.contains("exceeds expected"), "unexpected error: {err}");
            assert_eq!(dst, original);

            // PerTile bomb
            let mut dst = original.clone();
            let mut data = Vec::with_capacity(4 + bomb.len());
            data.extend_from_slice(&(bomb.len() as u32).to_le_bytes());
            data.extend_from_slice(bomb);
            let per_tile = EncodedDelta { width: 64, height: 64, keyframe: true, encoding: TileEncoding::PerTile, rects: vec![rect], data };
            let err = apply_delta(&per_tile, &mut dst).unwrap_err();
            assert!(err.contains("exceeds expected"), "unexpected error: {err}");
            assert_eq!(dst, original);
        }
    }

    #[test]
    fn rejects_crafted_headers_exceeding_data_or_frame_limits() {
        // Header naming huge rect_count on a tiny buffer: refused before allocating.
        let mut wire = Vec::new();
        wire.extend_from_slice(MAGIC);
        wire.extend_from_slice(&[VERSION, TileEncoding::PackedStream as u8, 1, 0]);
        wire.extend_from_slice(&64u32.to_le_bytes());
        wire.extend_from_slice(&64u32.to_le_bytes());
        wire.extend_from_slice(&u32::MAX.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        assert!(EncodedDelta::decode_bytes(&wire).is_err());

        // Header naming huge data_len on a tiny buffer: refused before allocating.
        let mut wire = Vec::new();
        wire.extend_from_slice(MAGIC);
        wire.extend_from_slice(&[VERSION, TileEncoding::PackedStream as u8, 1, 0]);
        wire.extend_from_slice(&64u32.to_le_bytes());
        wire.extend_from_slice(&64u32.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(EncodedDelta::decode_bytes(&wire).is_err());

        // Header naming absurd frame dimensions: refused before allocating.
        let mut wire = Vec::new();
        wire.extend_from_slice(MAGIC);
        wire.extend_from_slice(&[VERSION, TileEncoding::PackedStream as u8, 1, 0]);
        wire.extend_from_slice(&100_000u32.to_le_bytes());
        wire.extend_from_slice(&100_000u32.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        assert!(EncodedDelta::decode_bytes(&wire).is_err());

        let mut dst = Frame { width: 64, height: 64, rgba: vec![7; 64 * 64 * 4] };
        let keep = dst.clone();

        // Direct apply_delta with absurd frame dimensions.
        let huge_dim = EncodedDelta { width: 100_000, height: 100_000, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![], data: vec![] };
        assert!(apply_delta(&huge_dim, &mut dst).is_err());
        assert_eq!(dst, keep);

        // PerTile inner length naming more bytes than delta.data has.
        let bad_clen = EncodedDelta {
            width: 64,
            height: 64,
            keyframe: false,
            encoding: TileEncoding::PerTile,
            rects: vec![TileRect { x: 0, y: 0, width: 64, height: 64 }],
            data: u32::MAX.to_le_bytes().to_vec(),
        };
        assert!(apply_delta(&bad_clen, &mut dst).is_err());
        assert_eq!(dst, keep);

        // Duplicate overlapping rects whose combined bytes exceed the frame size.
        let dup_rects = EncodedDelta {
            width: 64,
            height: 64,
            keyframe: false,
            encoding: TileEncoding::PackedStream,
            rects: vec![TileRect { x: 0, y: 0, width: 64, height: 64 }; 2],
            data: zstd::encode_all(&vec![0u8; 2 * 64 * 64 * 4][..], 3).unwrap(),
        };
        assert!(apply_delta(&dup_rects, &mut dst).is_err());
        assert_eq!(dst, keep);

        // Zero-sized rect refused.
        let zero_rect = EncodedDelta { width: 64, height: 64, keyframe: false, encoding: TileEncoding::PackedStream, rects: vec![TileRect { x: 0, y: 0, width: 0, height: 64 }], data: vec![] };
        assert!(apply_delta(&zero_rect, &mut dst).is_err());
        assert_eq!(dst, keep);
    }

    #[test]
    fn astra_f8_rejects_excess_small_tiles_and_noncanonical_grid_in_olot() {
        let mut rects = Vec::new();
        for x in 0..64u16 {
            rects.push(TileRect { x, y: 0, width: 1, height: 1 });
        }
        let raw = vec![0u8; 64 * 4];
        let packed = zstd::encode_all(&raw[..], 1).unwrap();
        let bad = EncodedDelta { width: 64, height: 64, keyframe: false, encoding: TileEncoding::PackedStream, rects, data: packed };
        let wire = bad.encode_bytes();
        assert!(EncodedDelta::decode_bytes(&wire).is_err(), "decode_bytes must reject non-canonical/excess tile count");
        let mut dst = Frame { width: 64, height: 64, rgba: vec![1; 64 * 64 * 4] };
        assert!(apply_delta(&bad, &mut dst).is_err(), "apply_delta must reject non-canonical/excess tile count");
    }

    #[test]
    fn astra_f9_rejects_delta_without_base_incomplete_keyframe_and_zero_dims() {
        let full_a = desktop_frame(128, 64, 0, None);
        let full_b = desktop_frame(128, 64, 0, Some(10));
        let delta_only = encode_delta(&full_b, Some(&full_a), TileEncoding::PackedStream, 1).unwrap();
        assert!(!delta_only.keyframe);

        // 1. Delta against empty canvas must fail and leave dst untouched.
        let mut empty = Frame { width: 0, height: 0, rgba: Vec::new() };
        assert!(apply_delta(&delta_only, &mut empty).is_err(), "delta without base must fail");
        assert_eq!((empty.width, empty.height, empty.rgba.len()), (0, 0, 0));

        // 2. Delta against wrong-sized canvas must fail and leave dst untouched.
        let mut wrong = Frame { width: 64, height: 64, rgba: vec![5; 64 * 64 * 4] };
        let wrong_keep = wrong.clone();
        assert!(apply_delta(&delta_only, &mut wrong).is_err(), "delta on wrong-size base must fail");
        assert_eq!(wrong, wrong_keep);

        // 3. Empty non-keyframe delta must not resize dst.
        let empty_delta = EncodedDelta { width: 128, height: 64, keyframe: false, encoding: TileEncoding::PackedStream, rects: vec![], data: vec![] };
        assert!(apply_delta(&empty_delta, &mut wrong).is_err(), "empty delta must not resize mismatched canvas");
        assert_eq!(wrong, wrong_keep);

        // 4. Incomplete keyframe (only 1 of 2 tiles) against prefilled canvas must fail.
        let one_tile = zstd::encode_all(&vec![9u8; 64 * 64 * 4][..], 1).unwrap();
        let holey_key = EncodedDelta { width: 128, height: 64, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![TileRect { x: 0, y: 0, width: 64, height: 64 }], data: one_tile };
        let mut prefilled = Frame { width: 128, height: 64, rgba: vec![77; 128 * 64 * 4] };
        let prefilled_keep = prefilled.clone();
        assert!(apply_delta(&holey_key, &mut prefilled).is_err(), "incomplete keyframe with holes must fail");
        assert_eq!(prefilled, prefilled_keep);

        // 5. Zero dimensions refused in both decode_bytes and apply_delta.
        let zero_frame = EncodedDelta { width: 0, height: 64, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![], data: vec![] };
        assert!(EncodedDelta::decode_bytes(&zero_frame.encode_bytes()).is_err(), "zero dimensions must fail decode_bytes");
        assert!(apply_delta(&zero_frame, &mut prefilled).is_err(), "zero dimensions must fail apply_delta");
    }

    #[test]
    fn astra_f25_encoder_validates_inputs_and_bounds_zstd_window() {
        // 1. Short pixel buffer must return Err, not panic.
        let short = Frame { width: 64, height: 64, rgba: vec![0; 10] };
        assert!(encode_delta(&short, None, TileEncoding::PackedStream, 1).is_err());

        // 2. Wide frame past MAX_NET_DIM (20_000 x 64) must fail encode_delta instead of emitting undecodable OLOT.
        let wide = Frame { width: 20_000, height: 64, rgba: vec![0; 20_000 * 64 * 4] };
        assert!(encode_delta(&wide, None, TileEncoding::PackedStream, 1).is_err());

        // 3. High zstd compression level (20, which defaults to >32 MiB window on large frames)
        //    must produce output our decoder (window_log_max = 25) accepts!
        //    Use a 4096x2160 (~35.4 MiB) frame so libzstd at level 20 would exceed windowLog=25 if uncapped.
        let big = Frame { width: 4096, height: 2160, rgba: vec![42; 4096 * 2160 * 4] };
        let encoded = encode_delta(&big, None, TileEncoding::PackedStream, 20).expect("level 20 encode");
        let wire = encoded.encode_bytes();
        let decoded = EncodedDelta::decode_bytes(&wire).expect("decode level 20");
        let mut dst = Frame { width: 0, height: 0, rgba: Vec::new() };
        apply_delta(&decoded, &mut dst).expect("apply level 20 must succeed within 32 MiB window");
        assert_eq!((dst.width, dst.height), (4096, 2160));
    }

    #[test]
    fn review2_f21_decode_from_reader_validates_prefix_before_allocating_and_roundtrips() {
        let a = desktop_frame(192, 128, 0, None);
        let encoded = encode_delta(&a, None, TileEncoding::PackedStream, 1).unwrap();
        let wire = encoded.encode_bytes();
        let mut cur = std::io::Cursor::new(&wire[..]);
        let decoded = EncodedDelta::decode_from_reader(&mut cur, wire.len()).expect("roundtrip decode_from_reader");
        assert_eq!(decoded, encoded);

        // Crafted header claiming 32 MiB of data on a 28-byte truncated stream:
        // must fail cleanly on EOF without allocating 32 MiB upfront.
        let mut trunc = Vec::new();
        trunc.extend_from_slice(MAGIC);
        trunc.extend_from_slice(&[VERSION, TileEncoding::PackedStream as u8, 1, 0]);
        trunc.extend_from_slice(&64u32.to_le_bytes());
        trunc.extend_from_slice(&64u32.to_le_bytes());
        trunc.extend_from_slice(&1u32.to_le_bytes());
        for v in [0u16, 0, 64, 64] {
            trunc.extend_from_slice(&v.to_le_bytes());
        }
        let fake_data_len = 32u32 << 20;
        trunc.extend_from_slice(&fake_data_len.to_le_bytes());
        trunc.extend_from_slice(b"short");
        let declared_total = 20 + 8 + 4 + fake_data_len as usize;
        let mut cur = std::io::Cursor::new(&trunc[..]);
        let err = EncodedDelta::decode_from_reader(&mut cur, declared_total).unwrap_err();
        assert!(err.contains("truncated"), "expected truncation error, got: {err}");
    }
}
