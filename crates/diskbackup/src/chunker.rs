//! Content-defined chunks (FastCDC's gear hash, normalized): a cut where the
//! hash of the bytes just before says, so an insertion moves the cuts only
//! near it and the chunks after it are the same as before.

/// Chunk sizes: none smaller (but a file's last), about this, none larger.
pub const MIN: usize = 256 << 10;
pub const AVG: usize = 1 << 20;
pub const MAX: usize = 4 << 20;

/// The gear: a random-looking u64 for each byte value, always the same
/// (splitmix64 from a fixed seed), so cuts are the same everywhere.
const GEAR: [u64; 256] = {
    let mut g = [0u64; 256];
    let mut x: u64 = 0x6f6c_6f2d_6469_736b; // "olo-disk"
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
/// match before the average size (two bits more), easier after (two
/// fewer), so sizes gather near it.
const BITS: u32 = AVG.trailing_zeros();
const MASK_HARD: u64 = !0u64 << (64 - (BITS + 2));
const MASK_EASY: u64 = !0u64 << (64 - (BITS - 2));

/// How long `data`'s first chunk is (all of it if it's the end of the
/// input and no longer than `MAX`; `data` holds at least `MAX` bytes
/// otherwise).
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

/// Read `r` a chunk at a time, handing each to `each` (which may stop it).
pub fn chunks(mut r: impl std::io::Read, mut each: impl FnMut(&[u8]) -> crate::Result<()>) -> crate::Result<()> {
    let mut buf = vec![0u8; 2 * MAX];
    let (mut start, mut end) = (0usize, 0usize);
    let mut eof = false;
    loop {
        // Enough to find a cut (MAX), unless the input ends sooner.
        while !eof && end - start < MAX {
            if end == buf.len() {
                buf.copy_within(start..end, 0);
                end -= start;
                start = 0;
            }
            match r.read(&mut buf[end..]) {
                Ok(0) => eof = true,
                Ok(k) => end += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn all(data: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        chunks(data, |c| {
            out.push(c.to_vec());
            Ok(())
        })
        .unwrap();
        out
    }

    /// Chunks put together are the input; each within the sizes; about the
    /// average.
    #[test]
    fn sizes() {
        let data = noise(40 << 20, 1);
        let c = all(&data);
        assert_eq!(c.concat(), data);
        assert!(c[..c.len() - 1].iter().all(|c| (MIN..=MAX).contains(&c.len())));
        let mean = data.len() / c.len();
        assert!((AVG / 2..AVG * 2).contains(&mean), "{mean}");
        assert_eq!(all(b"tiny"), vec![b"tiny".to_vec()]);
        assert!(all(b"").is_empty());
    }

    /// Bytes put in the middle change only the chunks near them: the rest
    /// are the same, so they're kept once.
    #[test]
    fn an_insertion_moves_only_nearby_cuts() {
        let data = noise(30 << 20, 7);
        let mut edited = data[..15 << 20].to_vec();
        edited.extend_from_slice(b"a few bytes typed in the middle");
        edited.extend_from_slice(&data[15 << 20..]);
        let (a, b) = (all(&data), all(&edited));
        let before: std::collections::HashSet<&Vec<u8>> = a.iter().collect();
        let new = b.iter().filter(|c| !before.contains(c)).count();
        assert!(new <= 2, "{new} of {} chunks new", b.len());
    }
}
