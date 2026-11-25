//! Content-defined chunks (FastCDC's normalized gear hash): a cut where the
//! hash of the bytes just before says, so an insertion or edit moves the cuts
//! only near it and the chunks after it stay the same.
//!
//! Tuned finer than `olo-diskbackup`'s chunker (4 / 16 / 64 KiB instead of
//! 256 KiB / 1 MiB / 4 MiB) so an edit in the middle of a file or a small
//! change in a binary or SQLite database re-sends tens of KiB rather than MiB.

use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::sync::OnceLock;

/// Chunk sizes: none smaller (but a file's last), about this, none larger.
pub const MIN: usize = 4 << 10;
pub const AVG: usize = 16 << 10;
pub const MAX: usize = 64 << 10;

/// Minimum file size (64 KiB) for which content-defined chunk lists are
/// computed and kept in an [`crate::Index`]. Smaller files are sent whole
/// (compressed) rather than negotiating chunk deltas, saving chunk-list
/// memory and cache space across hundreds of thousands of small files.
pub const CHUNK_MIN_SIZE: u64 = MAX as u64;

/// Minimum file size (1 MiB) for probing filesystem holes (`SEEK_DATA` /
/// `SEEK_HOLE`) so holes are fed as synthetic zeros without reading disk.
pub const SPARSE_HASH_MIN_SIZE: u64 = 1 << 20;

static ZEROS: [u8; MAX] = [0u8; MAX];

fn zero_chunk() -> ChunkRef {
    static ZERO_CHUNK: OnceLock<ChunkRef> = OnceLock::new();
    *ZERO_CHUNK.get_or_init(|| {
        let len = cut(&ZEROS);
        ChunkRef { len: len as u32, id: chunk_id(&ZEROS[..len]) }
    })
}

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

    chunks.shrink_to_fit();
    Ok(FileDigest { size, hash: *whole.finalize().as_bytes(), chunks })
}

/// Read `r` once, computing its byte length and whole-file BLAKE3 without
/// computing or allocating any content-defined chunks (used for files below
/// [`CHUNK_MIN_SIZE`]).
pub fn hash_only_in(buf: &mut Vec<u8>, mut r: impl io::Read) -> io::Result<FileDigest> {
    if buf.len() < MAX {
        buf.resize(MAX, 0);
    }
    let slice = &mut buf[..MAX];
    let mut whole = blake3::Hasher::new();
    let mut size = 0u64;
    loop {
        match r.read(slice) {
            Ok(0) => break,
            Ok(k) => {
                whole.update(&slice[..k]);
                size += k as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(FileDigest { size, hash: *whole.finalize().as_bytes(), chunks: Vec::new() })
}

/// Probe `[offset, offset + max_len)` of `file` via `SEEK_DATA`/`SEEK_HOLE`.
/// Returns `(true, hole_len)` if it begins with a hole, or `(false, data_len)`
/// for a data run. Note that `lseek` updates the file descriptor's offset.
pub(crate) fn probe_extent(file: &fs::File, offset: u64, max_len: u64) -> (bool, u64) {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let Ok(off) = libc::off_t::try_from(offset) else {
        return (false, max_len);
    };
    // SAFETY: `fd` is a valid open regular file descriptor owned by `file`.
    let data_pos = unsafe { libc::lseek(fd, off, libc::SEEK_DATA) };
    if data_pos < 0 {
        if io::Error::last_os_error().raw_os_error() == Some(libc::ENXIO) {
            return (true, max_len);
        }
        return (false, max_len);
    }
    let data_pos = data_pos as u64;
    if data_pos > offset {
        return (true, (data_pos - offset).min(max_len));
    }
    // SAFETY: `fd` is a valid open regular file descriptor owned by `file`.
    let hole_pos = unsafe { libc::lseek(fd, off, libc::SEEK_HOLE) };
    if hole_pos < 0 {
        return (false, max_len);
    }
    let hole_pos = hole_pos as u64;
    if hole_pos > offset { (false, (hole_pos - offset).min(max_len)) } else { (false, max_len) }
}

/// Hash (and, when `file_size >= CHUNK_MIN_SIZE`, chunk) an open regular file.
/// For files of at least [`SPARSE_HASH_MIN_SIZE`] (1 MiB), probes for holes
/// (`SEEK_DATA`/`SEEK_HOLE`) and feeds holes as synthetic zeros without reading
/// them from disk, producing the exact same digest as reading the whole file.
pub fn hash_and_chunk_file_in(buf: &mut Vec<u8>, file: &mut fs::File, file_size: u64) -> io::Result<FileDigest> {
    if file_size < CHUNK_MIN_SIZE {
        return hash_only_in(buf, file);
    }
    if file_size < SPARSE_HASH_MIN_SIZE {
        return hash_and_chunk_in(buf, file);
    }
    let (first_hole, first_len) = probe_extent(file, 0, file_size);
    if !first_hole && first_len >= file_size {
        file.seek(SeekFrom::Start(0))?;
        return hash_and_chunk_in(buf, file);
    }
    hash_and_chunk_sparse_with(buf, file_size, |off, max_len| probe_extent(file, off, max_len), |off, dst| file.read_at(dst, off))
}

/// Sparse-aware hash and chunk driven by an extent prober and positional reader
/// (separated so tests can count exact data bytes read from disk).
pub(crate) fn hash_and_chunk_sparse_with(
    buf: &mut Vec<u8>,
    file_size: u64,
    mut probe: impl FnMut(u64, u64) -> (bool, u64),
    mut read_at: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
) -> io::Result<FileDigest> {
    if buf.len() < MAX {
        buf.resize(MAX, 0);
    }
    let slice = &mut buf[..MAX];
    let mut sd = StreamDigest::new();
    let mut pos = 0u64;
    while pos < file_size {
        let rem = file_size - pos;
        let (is_hole, ext_len) = probe(pos, rem);
        let ext_len = ext_len.min(rem);
        if is_hole && ext_len > 0 {
            sd.update_zeros(ext_len);
            pos += ext_len;
        } else {
            let mut data_rem = ext_len.max(1).min(rem);
            while data_rem > 0 {
                let take = (data_rem as usize).min(slice.len());
                match read_at(pos, &mut slice[..take]) {
                    Ok(0) => return Ok(sd.finalize()),
                    Ok(k) => {
                        sd.update(&slice[..k]);
                        pos += k as u64;
                        data_rem -= k as u64;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(sd.finalize())
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
    chunking: bool,
}

impl Default for StreamDigest {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamDigest {
    pub fn new() -> Self {
        Self { buf: vec![0u8; 2 * MAX], start: 0, end: 0, whole: blake3::Hasher::new(), size: 0, chunks: Vec::new(), chunking: true }
    }

    /// Hash-only variant for files below [`CHUNK_MIN_SIZE`]: allocates no
    /// chunking buffer and produces an empty `chunks` vector.
    pub fn new_hash_only() -> Self {
        Self { buf: Vec::new(), start: 0, end: 0, whole: blake3::Hasher::new(), size: 0, chunks: Vec::new(), chunking: false }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.whole.update(data);
        self.size += data.len() as u64;
        if !self.chunking {
            return;
        }
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

    /// Feed `len` zero bytes (e.g. a sparse hole) without reading from disk,
    /// producing the exact same whole-file hash and chunk list as `update`.
    pub fn update_zeros(&mut self, mut len: u64) {
        self.size += len;
        let mut rem = len;
        while rem > 0 {
            let take = (rem as usize).min(ZEROS.len());
            self.whole.update(&ZEROS[..take]);
            rem -= take as u64;
        }
        if !self.chunking {
            return;
        }
        // Drain any existing non-zero prefix in `self.buf` until all bytes left
        // in the window are zeros (at most 1-2 cuts).
        let mut non_zero_end = self.end;
        while len > 0 && self.start < non_zero_end {
            if self.end == self.buf.len() {
                self.buf.copy_within(self.start..self.end, 0);
                non_zero_end = non_zero_end.saturating_sub(self.start);
                self.end -= self.start;
                self.start = 0;
            }
            let take = ((self.buf.len() - self.end) as u64).min(len) as usize;
            self.buf[self.end..self.end + take].fill(0);
            self.end += take;
            len -= take as u64;
            while self.end - self.start >= MAX {
                let k = cut(&self.buf[self.start..self.end]);
                let id = chunk_id(&self.buf[self.start..self.start + k]);
                self.chunks.push(ChunkRef { len: k as u32, id });
                self.start += k;
            }
        }
        // Any remaining buffered bytes past `non_zero_end` are zeros too; fold
        // them back into `len` so the pure-zero fast path handles the whole run.
        if self.start >= non_zero_end && self.start < self.end {
            len += (self.end - self.start) as u64;
            self.start = 0;
            self.end = 0;
        }
        if self.start == self.end && len >= MAX as u64 {
            let zc = zero_chunk();
            let zlen = zc.len as u64;
            let repeats = (len - MAX as u64) / zlen + 1;
            self.chunks.reserve(repeats as usize);
            for _ in 0..repeats {
                self.chunks.push(zc);
            }
            len -= repeats * zlen;
        }
        if len > 0 {
            debug_assert!(self.start == self.end && (len as usize) < MAX);
            self.start = 0;
            self.end = len as usize;
            self.buf[..self.end].fill(0);
        }
    }

    pub fn finalize(mut self) -> FileDigest {
        if self.chunking {
            while self.start < self.end {
                let k = cut(&self.buf[self.start..self.end]);
                let id = chunk_id(&self.buf[self.start..self.start + k]);
                self.chunks.push(ChunkRef { len: k as u32, id });
                self.start += k;
            }
            self.chunks.shrink_to_fit();
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

    #[test]
    fn sparse_hash_and_chunk_matches_full_read_and_reads_only_data() {
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-sync-chunk-sparse-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Case 1: data at start, middle (unaligned), near end, plus trailing hole (16 MiB).
        let p1 = dir.join("disk1.img");
        {
            let f = fs::File::create(&p1).unwrap();
            f.set_len(16 << 20).unwrap();
            f.write_all_at(&noise(1337, 11), 0).unwrap();
            f.write_all_at(&noise(5000, 22), (3 << 20) + 123).unwrap();
            f.write_all_at(&noise(8192, 33), 12 << 20).unwrap();
        }
        // Case 2: leading 4 MiB hole, small data run, trailing hole (10 MiB).
        let p2 = dir.join("disk2.img");
        {
            let f = fs::File::create(&p2).unwrap();
            f.set_len(10 << 20).unwrap();
            f.write_all_at(&noise(9999, 44), 4 << 20).unwrap();
        }
        // Case 3: all-hole sparse file (4 MiB).
        let p3 = dir.join("disk3.img");
        {
            let f = fs::File::create(&p3).unwrap();
            f.set_len(4 << 20).unwrap();
        }

        for (path, max_data_read) in [(&p1, 256 << 10), (&p2, 256 << 10), (&p3, 64 << 10)] {
            let file_size = fs::metadata(path).unwrap().len();
            let full = hash_and_chunk(fs::File::open(path).unwrap()).unwrap();

            let mut f = fs::File::open(path).unwrap();
            let via_file = hash_and_chunk_file_in(&mut Vec::new(), &mut f, file_size).unwrap();
            assert_eq!(via_file, full, "hash_and_chunk_file_in mismatch for {}", path.display());

            let bytes_read = AtomicU64::new(0);
            let f2 = fs::File::open(path).unwrap();
            let via_counted = hash_and_chunk_sparse_with(
                &mut Vec::new(),
                file_size,
                |off, max_len| probe_extent(&f2, off, max_len),
                |off, dst| {
                    let n = f2.read_at(dst, off)?;
                    bytes_read.fetch_add(n as u64, Ordering::Relaxed);
                    Ok(n)
                },
            )
            .unwrap();
            assert_eq!(via_counted, full, "hash_and_chunk_sparse_with mismatch for {}", path.display());
            assert!(
                bytes_read.load(Ordering::Relaxed) <= max_data_read,
                "expected sparse hashing to read only data runs (<= {max_data_read} B), read {} B for {}",
                bytes_read.load(Ordering::Relaxed),
                path.display()
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
