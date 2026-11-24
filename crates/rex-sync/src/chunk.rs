//! Content-defined chunks (FastCDC's normalized gear hash): a cut where the
//! hash of the bytes just before says, so an insertion or edit moves the cuts
//! only near it and the chunks after it stay the same.
//!
//! Tuned finer than `olo-diskbackup`'s chunker (4 / 16 / 64 KiB instead of
//! 256 KiB / 1 MiB / 4 MiB) so an edit in the middle of a file or a small
//! change in a binary or SQLite database re-sends tens of KiB rather than MiB.

use std::io;

/// Chunk sizes: none smaller (but a file's last), about this, none larger.
pub const MIN: usize = 4 << 10;
pub const AVG: usize = 16 << 10;
pub const MAX: usize = 64 << 10;

/// A chunk's content identity: the first 16 bytes (128 bits) of its BLAKE3.
pub type ChunkId = [u8; 16];

/// One content-defined chunk of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkRef {
    pub len: u32,
    pub id: ChunkId,
}

/// Whole-file digest and chunk list produced in a single read pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    pub size: u64,
    pub hash: [u8; 32],
    pub chunks: Vec<ChunkRef>,
}

/// The gear: a random-looking u64 for each byte value, always the same
/// (splitmix64 from a fixed seed), so cuts are identical on every host.
const GEAR: [u64; 256] = {
    let mut g = [0u64; 256];
    let mut x: u64 = 0x6f6c_6f2d_7379_6e63; // "olo-sync"
    let mut i = 0;
    while i < 256 {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        g[i] = z ^ (z >> 31);
        i += 1;
    }
    g
};

/// Masks on the hash's top bits (where the most bytes count): harder to
/// match before the average size (two bits more), easier after (two fewer),
/// so chunk sizes cluster around `AVG`.
const BITS: u32 = AVG.trailing_zeros();
const MASK_HARD: u64 = !0u64 << (64 - (BITS + 2));
const MASK_EASY: u64 = !0u64 << (64 - (BITS - 2));

/// First 16 bytes of `data`'s BLAKE3.
#[inline]
pub fn chunk_id(data: &[u8]) -> ChunkId {
    let h = blake3::hash(data);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h.as_bytes()[..16]);
    id
}

/// How long `data`'s first chunk is (all of it if it's the end of the input
/// and no longer than `MAX`; `data` holds at least `MAX` bytes otherwise).
pub fn cut(data: &[u8]) -> usize {
    if data.len() <= MIN {
        return data.len();
    }
    let n = data.len().min(MAX);
    let normal = AVG.min(n);
    let mut h: u64 = 0;
    let mut i = MIN;
    while i < normal {
        h = (h << 1).wrapping_add(GEAR[data[i] as usize]);
        if h & MASK_HARD == 0 {
            return i + 1;
        }
        i += 1;
    }
    while i < n {
        h = (h << 1).wrapping_add(GEAR[data[i] as usize]);
        if h & MASK_EASY == 0 {
            return i + 1;
        }
        i += 1;
    }
    n
}

/// Read `r` a chunk at a time, handing each slice to `each`.
pub fn chunks(r: impl io::Read, each: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    chunks_in(&mut Vec::new(), r, each)
}

/// [`chunks`], reusing `buf` (grown once to `2 * MAX`, 128 KiB) across files.
pub fn chunks_in(buf: &mut Vec<u8>, mut r: impl io::Read, mut each: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    if buf.len() < 2 * MAX {
        buf.resize(2 * MAX, 0);
    }
    let buf = &mut buf[..2 * MAX];
    let (mut start, mut end) = (0usize, 0usize);
    let mut eof = false;
    loop {
        while !eof && end - start < MAX {
            if end == buf.len() {
                buf.copy_within(start..end, 0);
                end -= start;
                start = 0;
            }
            match r.read(&mut buf[end..]) {
                Ok(0) => eof = true,
                Ok(k) => end += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if start == end {
            return Ok(());
        }
        let k = cut(&buf[start..end]);
        each(&buf[start..start + k])?;
        start += k;
    }
}

/// Read `r` once, returning its byte length, whole-file BLAKE3 (32 bytes),
/// and content-defined chunks. Never reads the input twice.
pub fn hash_and_chunk(r: impl io::Read) -> io::Result<FileDigest> {
    hash_and_chunk_in(&mut Vec::new(), r)
}

/// [`hash_and_chunk`], reusing `buf` across files.
pub fn hash_and_chunk_in(buf: &mut Vec<u8>, mut r: impl io::Read) -> io::Result<FileDigest> {
    if buf.len() < 2 * MAX {
        buf.resize(2 * MAX, 0);
    }
    let buf = &mut buf[..2 * MAX];
    let (mut start, mut end) = (0usize, 0usize);
    let mut eof = false;
    let mut whole = blake3::Hasher::new();
    let mut size = 0u64;
    let mut chunks = Vec::new();

    loop {
        while !eof && end - start < MAX {
            if end == buf.len() {
                buf.copy_within(start..end, 0);
                end -= start;
                start = 0;
            }
            match r.read(&mut buf[end..]) {
                Ok(0) => eof = true,
                Ok(k) => end += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if start == end {
            break;
        }
        let k = cut(&buf[start..end]);
        let slice = &buf[start..start + k];
        // Fast path for a file that fits in a single chunk: hash once instead
        // of twice (common for small source/config files).
        if chunks.is_empty() && eof && start + k == end {
            let h = *blake3::hash(slice).as_bytes();
            let mut id = [0u8; 16];
            id.copy_from_slice(&h[..16]);
            chunks.push(ChunkRef { len: k as u32, id });
            return Ok(FileDigest { size: k as u64, hash: h, chunks });
        }
        whole.update(slice);
        size += k as u64;
        chunks.push(ChunkRef { len: k as u32, id: chunk_id(slice) });
        start += k;
    }

    Ok(FileDigest { size, hash: *whole.finalize().as_bytes(), chunks })
}

/// Incremental chunker + whole-file hasher for streams being written (used by
/// the receiver to compute both BLAKE3 and `Vec<ChunkRef>` while writing a
/// file, keeping its cache warm with zero extra disk reads).
pub struct StreamDigest {
    buf: Vec<u8>,
    start: usize,
    end: usize,
    whole: blake3::Hasher,
    size: u64,
    chunks: Vec<ChunkRef>,
}

impl Default for StreamDigest {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamDigest {
    pub fn new() -> Self {
        Self { buf: vec![0u8; 2 * MAX], start: 0, end: 0, whole: blake3::Hasher::new(), size: 0, chunks: Vec::new() }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.whole.update(data);
        self.size += data.len() as u64;
        while !data.is_empty() {
            if self.end == self.buf.len() {
                self.buf.copy_within(self.start..self.end, 0);
                self.end -= self.start;
                self.start = 0;
            }
            let take = (self.buf.len() - self.end).min(data.len());
            self.buf[self.end..self.end + take].copy_from_slice(&data[..take]);
            self.end += take;
            data = &data[take..];

            while self.end - self.start >= MAX {
                let k = cut(&self.buf[self.start..self.end]);
                let id = chunk_id(&self.buf[self.start..self.start + k]);
                self.chunks.push(ChunkRef { len: k as u32, id });
                self.start += k;
            }
        }
    }

    pub fn finalize(mut self) -> FileDigest {
        while self.start < self.end {
            let k = cut(&self.buf[self.start..self.end]);
            let id = chunk_id(&self.buf[self.start..self.start + k]);
            self.chunks.push(ChunkRef { len: k as u32, id });
            self.start += k;
        }
        FileDigest { size: self.size, hash: *self.whole.finalize().as_bytes(), chunks: self.chunks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    fn all_chunks(data: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        chunks(data, |c| {
            out.push(c.to_vec());
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn sizes_and_boundaries() {
        let data = noise(4 << 20, 1);
        let c = all_chunks(&data);
        assert_eq!(c.concat(), data);
        assert!(c[..c.len() - 1].iter().all(|chunk| (MIN..=MAX).contains(&chunk.len())));
        let mean = data.len() / c.len();
        assert!((AVG / 2..AVG * 2).contains(&mean), "mean chunk size {mean} outside [{}, {})", AVG / 2, AVG * 2);
        assert_eq!(all_chunks(b"tiny"), vec![b"tiny".to_vec()]);
        assert!(all_chunks(b"").is_empty());
    }

    #[test]
    fn cuts_stay_put_after_insertion_except_nearby() {
        let data = noise(2 << 20, 7);
        let mid = data.len() / 2;
        let mut edited = Vec::with_capacity(data.len() + 64);
        edited.extend_from_slice(&data[..mid]);
        edited.extend_from_slice(b"inserted bytes right in the middle of the stream");
        edited.extend_from_slice(&data[mid..]);

        let a = hash_and_chunk(&data[..]).unwrap();
        let b = hash_and_chunk(&edited[..]).unwrap();
        let before: HashSet<ChunkRef> = a.chunks.iter().copied().collect();
        let new_chunks = b.chunks.iter().filter(|c| !before.contains(c)).count();
        assert!(new_chunks <= 3, "{new_chunks} of {} chunks changed after mid-file insertion", b.chunks.len());
    }

    #[test]
    fn hash_and_chunk_matches_blake3_and_stream_digest() {
        for &len in &[0usize, 1, 100, MIN, AVG, MAX, MAX + 1, 300_000] {
            let data = noise(len, len as u64 + 42);
            let digest = hash_and_chunk(&data[..]).unwrap();
            assert_eq!(digest.size, len as u64);
            assert_eq!(digest.hash, *blake3::hash(&data).as_bytes());
            let total_chunk_len: usize = digest.chunks.iter().map(|c| c.len as usize).sum();
            assert_eq!(total_chunk_len, len);

            let mut sd = StreamDigest::new();
            for piece in data.chunks(1337) {
                sd.update(piece);
            }
            assert_eq!(sd.finalize(), digest);
        }
    }
}
