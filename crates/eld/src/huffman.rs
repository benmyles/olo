//! AAC's Huffman decoding: the scale factor codebook and the eleven
//! spectrum codebooks (quads and pairs, signed or with sign bits, and
//! codebook 11's escapes).

use std::sync::OnceLock;

use crate::{bits::Bits, codebooks as cb};

/// A codebook: for each code length, its codes and what they stand for,
/// sorted by code.
struct Book {
    by_len: Vec<Vec<(u32, u16)>>,
}

impl Book {
    fn new(lens: &[u8], codes: &[u32]) -> Book {
        let longest = *lens.iter().max().unwrap_or(&0) as usize;
        let mut by_len = vec![Vec::new(); longest + 1];
        for (i, (&l, &c)) in lens.iter().zip(codes).enumerate() {
            by_len[l as usize].push((c, i as u16));
        }
        for v in &mut by_len {
            v.sort_unstable();
        }
        Book { by_len }
    }

    /// The next codeword's index.
    fn read(&self, b: &mut Bits<'_>) -> Option<usize> {
        let mut code = 0u32;
        for codes in self.by_len.iter().skip(1) {
            code = (code << 1) | b.read(1)?;
            if let Ok(i) = codes.binary_search_by_key(&code, |e| e.0) {
                return Some(codes[i].1 as usize);
            }
        }
        None
    }
}

fn books() -> &'static [Book; 12] {
    static B: OnceLock<[Book; 12]> = OnceLock::new();
    B.get_or_init(|| {
        [
            Book::new(&cb::SCF_CODEBOOK_LENS, &cb::SCF_CODEBOOK_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK1_LENS, &cb::SPECTRUM_CODEBOOK1_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK2_LENS, &cb::SPECTRUM_CODEBOOK2_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK3_LENS, &cb::SPECTRUM_CODEBOOK3_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK4_LENS, &cb::SPECTRUM_CODEBOOK4_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK5_LENS, &cb::SPECTRUM_CODEBOOK5_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK6_LENS, &cb::SPECTRUM_CODEBOOK6_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK7_LENS, &cb::SPECTRUM_CODEBOOK7_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK8_LENS, &cb::SPECTRUM_CODEBOOK8_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK9_LENS, &cb::SPECTRUM_CODEBOOK9_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK10_LENS, &cb::SPECTRUM_CODEBOOK10_CODES),
            Book::new(&cb::SPECTRUM_CODEBOOK11_LENS, &cb::SPECTRUM_CODEBOOK11_CODES),
        ]
    })
}

/// A scale factor difference's codeword: 0–120 (60 is no change).
pub fn scale_factor(b: &mut Bits<'_>) -> Option<i32> {
    books()[0].read(b).map(|i| i as i32)
}

/// One band's lines with spectrum codebook `cb` (1–11), as quantized
/// values (signed integers, in `out`).
pub fn spectrum(b: &mut Bits<'_>, cb: u8, out: &mut [f32]) -> Option<()> {
    let book = books().get(cb as usize)?;
    let (dims, modulo, signed): (usize, usize, bool) = match cb {
        1 | 2 => (4, 3, true),
        3 | 4 => (4, 3, false),
        5 | 6 => (2, 9, true),
        7 | 8 => (2, 8, false),
        9 | 10 => (2, 13, false),
        11 => (2, 17, false),
        _ => return None,
    };
    for chunk in out.chunks_mut(dims) {
        let mut idx = book.read(b)?;
        let mut v = [0i32; 4];
        for d in (0..dims).rev() {
            v[d] = (idx % modulo) as i32;
            idx /= modulo;
        }
        if signed {
            let off = (modulo / 2) as i32;
            for x in &mut v[..dims] {
                *x -= off;
            }
        } else {
            for x in &mut v[..dims] {
                if *x != 0 && b.bit()? {
                    *x = -*x;
                }
            }
            if cb == 11 {
                for x in &mut v[..dims] {
                    if x.abs() == 16 {
                        let mut n = 4;
                        while b.bit()? {
                            n += 1;
                            if n > 12 {
                                return None;
                            }
                        }
                        let e = (1 << n) + b.read(n)? as i32;
                        *x = x.signum() * e;
                    }
                }
            }
        }
        for (o, x) in chunk.iter_mut().zip(v) {
            *o = x as f32;
        }
    }
    Some(())
}
