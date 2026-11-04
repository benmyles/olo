//! The model: [`F`] features of a candidate in, a score out, through
//! [`H`] hidden units (ReLU). Its weights, written by hand to start
//! ([`Weights::baseline`]) and fine-tuned on the person's history, and
//! its arithmetic three ways: plain Rust (the reference), and SIMD
//! written by hand for the processor (NEON on aarch64; AVX2 with FMA on
//! x86_64, found at run time, else SSE2). The GPU's is olo-complete's
//! (its WGSL, the same arithmetic).
//!
//! The hidden layer is laid out input-major (`w1[i * H + j]`: input `i`
//! to hidden unit `j`), so a row's [`H`] hidden units are one vector:
//! each input broadcast and multiplied into a row of weights.

use serde::{Deserialize, Serialize};

pub use crate::index::F;

/// Hidden units.
pub const H: usize = 16;
/// How many numbers the model is: both layers' weights and biases.
pub const PARAMS: usize = F * H + H + H + 1;

/// The model's weights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub w1: Vec<f32>,
    pub b1: Vec<f32>,
    pub w2: Vec<f32>,
    pub b2: f32,
}

impl Weights {
    /// Written by hand: each hidden unit passes one feature on (ReLU of a
    /// feature at least 0 is itself), weighed as a person would rank
    /// suggestions: what follows the last command, what's used here and
    /// lately, then how often, the hour and the day; long completions a
    /// little less likely. Fine-tuning starts from it and learns how the
    /// features go together.
    pub fn baseline() -> Weights {
        let mut w1 = vec![0.0; F * H];
        for i in 0..F.min(H) {
            w1[i * H + i] = 1.0;
        }
        // In FEATURE_NAMES' order.
        let w2 = vec![0.6, 1.0, 1.6, 1.2, 0.4, 1.8, 0.6, 0.3, 0.6, 0.3, -0.3, 0.0, 0.8, 0.3, 0.7, 0.6];
        Weights { w1, b1: vec![0.0; H], w2, b2: 0.0 }
    }

    pub fn zeros() -> Weights {
        Weights { w1: vec![0.0; F * H], b1: vec![0.0; H], w2: vec![0.0; H], b2: 0.0 }
    }

    /// All [`PARAMS`] numbers in one row (for an optimizer).
    pub fn flat(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(PARAMS);
        v.extend_from_slice(&self.w1);
        v.extend_from_slice(&self.b1);
        v.extend_from_slice(&self.w2);
        v.push(self.b2);
        v
    }

    pub fn from_flat(v: &[f32]) -> Weights {
        let (w1, rest) = v.split_at(F * H);
        let (b1, rest) = rest.split_at(H);
        let (w2, rest) = rest.split_at(H);
        Weights { w1: w1.to_vec(), b1: b1.to_vec(), w2: w2.to_vec(), b2: rest[0] }
    }

    /// Whether it's whole (every vector its length, every number finite).
    pub fn sound(&self) -> bool {
        self.w1.len() == F * H && self.b1.len() == H && self.w2.len() == H && self.flat().iter().all(|x| x.is_finite())
    }
}

/// How the arithmetic is done on the processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cpu {
    /// Plain Rust, one number at a time: the reference.
    Scalar,
    /// Written for the processor's vectors.
    Simd,
}

/// Which vectors this processor has, for [`Cpu::Simd`].
pub fn simd_name() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    return "NEON";
    #[cfg(target_arch = "x86_64")]
    return if x86::has_avx2_fma() { "AVX2 with FMA" } else { "SSE2" };
    #[allow(unreachable_code)]
    "none (plain Rust)"
}

/// Scores of `rows` candidates (`x`: `rows × F`), each row's hidden units
/// kept in `hidden` (`rows × H`) for a backward pass.
pub fn forward(cpu: Cpu, w: &Weights, x: &[f32], hidden: &mut [f32], scores: &mut [f32]) {
    let rows = scores.len();
    debug_assert!(x.len() >= rows * F && hidden.len() >= rows * H);
    for r in 0..rows {
        let (xr, hr) = (&x[r * F..(r + 1) * F], &mut hidden[r * H..(r + 1) * H]);
        scores[r] = match cpu {
            Cpu::Scalar => scalar::forward_row(w, xr, hr),
            Cpu::Simd => simd_forward_row(w, xr, hr),
        };
    }
}

/// The gradient of a loss whose derivative by each score is `dscores`,
/// added into `g` (same shape as the weights).
pub fn backward(cpu: Cpu, w: &Weights, x: &[f32], hidden: &[f32], dscores: &[f32], g: &mut Weights) {
    for (r, &ds) in dscores.iter().enumerate() {
        if ds == 0.0 {
            continue;
        }
        let (xr, hr) = (&x[r * F..(r + 1) * F], &hidden[r * H..(r + 1) * H]);
        match cpu {
            Cpu::Scalar => scalar::backward_row(w, xr, hr, ds, g),
            Cpu::Simd => simd_backward_row(w, xr, hr, ds, g),
        }
    }
}

mod scalar {
    use super::{F, H, Weights};

    pub fn forward_row(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
        h[..H].copy_from_slice(&w.b1);
        for i in 0..F {
            let xi = x[i];
            for j in 0..H {
                h[j] += xi * w.w1[i * H + j];
            }
        }
        let mut s = w.b2;
        for j in 0..H {
            h[j] = h[j].max(0.0);
            s += h[j] * w.w2[j];
        }
        s
    }

    pub fn backward_row(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
        g.b2 += ds;
        let mut dh = [0.0f32; H];
        for j in 0..H {
            g.w2[j] += ds * h[j];
            dh[j] = if h[j] > 0.0 { ds * w.w2[j] } else { 0.0 };
            g.b1[j] += dh[j];
        }
        for i in 0..F {
            let xi = x[i];
            for j in 0..H {
                g.w1[i * H + j] += xi * dh[j];
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn simd_forward_row(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
    // SAFETY: NEON is always there on aarch64; every pointer is a slice
    // of at least 16 floats (H, and a row of w1), checked by the asserts.
    assert!(w.w1.len() >= F * H && w.b1.len() >= H && w.w2.len() >= H && x.len() >= F && h.len() >= H);
    unsafe { neon::forward_row(w, x, h) }
}

#[cfg(target_arch = "aarch64")]
fn simd_backward_row(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
    assert!(w.w2.len() >= H && x.len() >= F && h.len() >= H && g.w1.len() >= F * H && g.b1.len() >= H && g.w2.len() >= H);
    // SAFETY: as forward_row's.
    unsafe { neon::backward_row(w, x, h, ds, g) }
}

#[cfg(target_arch = "x86_64")]
fn simd_forward_row(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
    assert!(w.w1.len() >= F * H && w.b1.len() >= H && w.w2.len() >= H && x.len() >= F && h.len() >= H);
    // SAFETY: AVX2 and FMA are used only when the processor has them
    // (checked once); SSE2 is part of x86_64. Slices as asserted.
    unsafe { if x86::has_avx2_fma() { x86::forward_row_avx(w, x, h) } else { x86::forward_row_sse(w, x, h) } }
}

#[cfg(target_arch = "x86_64")]
fn simd_backward_row(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
    assert!(w.w2.len() >= H && x.len() >= F && h.len() >= H && g.w1.len() >= F * H && g.b1.len() >= H && g.w2.len() >= H);
    // SAFETY: as forward_row's.
    unsafe { if x86::has_avx2_fma() { x86::backward_row_avx(w, x, h, ds, g) } else { x86::backward_row_sse(w, x, h, ds, g) } }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn simd_forward_row(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
    scalar::forward_row(w, x, h)
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn simd_backward_row(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
    scalar::backward_row(w, x, h, ds, g)
}

/// NEON: the 16 hidden units as four vectors of four.
// (Each kernel is one unsafe block: loads and stores through slices the
// callers' asserts measured.)
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_op_in_unsafe_fn)]
mod neon {
    use std::arch::aarch64::*;

    use super::{F, H, Weights};

    #[target_feature(enable = "neon")]
    pub unsafe fn forward_row(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
        let (w1, b1, w2) = (w.w1.as_ptr(), w.b1.as_ptr(), w.w2.as_ptr());
        let mut acc = [vld1q_f32(b1), vld1q_f32(b1.add(4)), vld1q_f32(b1.add(8)), vld1q_f32(b1.add(12))];
        for i in 0..F {
            let xi = vdupq_n_f32(*x.get_unchecked(i));
            let row = w1.add(i * H);
            for (k, a) in acc.iter_mut().enumerate() {
                *a = vfmaq_f32(*a, xi, vld1q_f32(row.add(4 * k)));
            }
        }
        let zero = vdupq_n_f32(0.0);
        let mut s = vdupq_n_f32(0.0);
        for (k, a) in acc.iter().enumerate() {
            let r = vmaxq_f32(*a, zero);
            vst1q_f32(h.as_mut_ptr().add(4 * k), r);
            s = vfmaq_f32(s, r, vld1q_f32(w2.add(4 * k)));
        }
        vaddvq_f32(s) + w.b2
    }

    #[target_feature(enable = "neon")]
    pub unsafe fn backward_row(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
        g.b2 += ds;
        let d = vdupq_n_f32(ds);
        let zero = vdupq_n_f32(0.0);
        let mut dh = [zero; 4];
        for (k, dhk) in dh.iter_mut().enumerate() {
            let hk = vld1q_f32(h.as_ptr().add(4 * k));
            let gw2 = g.w2.as_mut_ptr().add(4 * k);
            vst1q_f32(gw2, vfmaq_f32(vld1q_f32(gw2), d, hk));
            // ReLU's gradient: the unit's share where it was on.
            let on = vcgtq_f32(hk, zero);
            *dhk = vbslq_f32(on, vmulq_f32(d, vld1q_f32(w.w2.as_ptr().add(4 * k))), zero);
            let gb1 = g.b1.as_mut_ptr().add(4 * k);
            vst1q_f32(gb1, vaddq_f32(vld1q_f32(gb1), *dhk));
        }
        for i in 0..F {
            let xi = vdupq_n_f32(*x.get_unchecked(i));
            let row = g.w1.as_mut_ptr().add(i * H);
            for (k, dhk) in dh.iter().enumerate() {
                let p = row.add(4 * k);
                vst1q_f32(p, vfmaq_f32(vld1q_f32(p), xi, *dhk));
            }
        }
    }
}

/// x86_64: the 16 hidden units as two AVX vectors of eight (with FMA),
/// or four SSE2 ones of four.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_op_in_unsafe_fn)]
mod x86 {
    use std::arch::x86_64::*;
    use std::sync::OnceLock;

    use super::{F, H, Weights};

    pub fn has_avx2_fma() -> bool {
        static HAS: OnceLock<bool> = OnceLock::new();
        *HAS.get_or_init(|| is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma"))
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn hsum256(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        hsum128(_mm_add_ps(lo, hi))
    }

    #[target_feature(enable = "sse2")]
    unsafe fn hsum128(v: __m128) -> f32 {
        let shuf = _mm_shuffle_ps(v, v, 0b10_11_00_01);
        let sums = _mm_add_ps(v, shuf);
        let shuf = _mm_movehl_ps(shuf, sums);
        _mm_cvtss_f32(_mm_add_ss(sums, shuf))
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn forward_row_avx(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
        let (w1, b1, w2) = (w.w1.as_ptr(), w.b1.as_ptr(), w.w2.as_ptr());
        let (mut a0, mut a1) = (_mm256_loadu_ps(b1), _mm256_loadu_ps(b1.add(8)));
        for i in 0..F {
            let xi = _mm256_set1_ps(*x.get_unchecked(i));
            let row = w1.add(i * H);
            a0 = _mm256_fmadd_ps(xi, _mm256_loadu_ps(row), a0);
            a1 = _mm256_fmadd_ps(xi, _mm256_loadu_ps(row.add(8)), a1);
        }
        let zero = _mm256_setzero_ps();
        let (r0, r1) = (_mm256_max_ps(a0, zero), _mm256_max_ps(a1, zero));
        _mm256_storeu_ps(h.as_mut_ptr(), r0);
        _mm256_storeu_ps(h.as_mut_ptr().add(8), r1);
        let s = _mm256_fmadd_ps(r1, _mm256_loadu_ps(w2.add(8)), _mm256_mul_ps(r0, _mm256_loadu_ps(w2)));
        hsum256(s) + w.b2
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn backward_row_avx(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
        g.b2 += ds;
        let d = _mm256_set1_ps(ds);
        let zero = _mm256_setzero_ps();
        let mut dh = [zero; 2];
        for (k, dhk) in dh.iter_mut().enumerate() {
            let hk = _mm256_loadu_ps(h.as_ptr().add(8 * k));
            let gw2 = g.w2.as_mut_ptr().add(8 * k);
            _mm256_storeu_ps(gw2, _mm256_fmadd_ps(d, hk, _mm256_loadu_ps(gw2)));
            let on = _mm256_cmp_ps(hk, zero, _CMP_GT_OQ);
            *dhk = _mm256_and_ps(on, _mm256_mul_ps(d, _mm256_loadu_ps(w.w2.as_ptr().add(8 * k))));
            let gb1 = g.b1.as_mut_ptr().add(8 * k);
            _mm256_storeu_ps(gb1, _mm256_add_ps(_mm256_loadu_ps(gb1), *dhk));
        }
        for i in 0..F {
            let xi = _mm256_set1_ps(*x.get_unchecked(i));
            let row = g.w1.as_mut_ptr().add(i * H);
            _mm256_storeu_ps(row, _mm256_fmadd_ps(xi, dh[0], _mm256_loadu_ps(row)));
            _mm256_storeu_ps(row.add(8), _mm256_fmadd_ps(xi, dh[1], _mm256_loadu_ps(row.add(8))));
        }
    }

    #[target_feature(enable = "sse2")]
    pub unsafe fn forward_row_sse(w: &Weights, x: &[f32], h: &mut [f32]) -> f32 {
        let (w1, b1, w2) = (w.w1.as_ptr(), w.b1.as_ptr(), w.w2.as_ptr());
        let mut acc = [_mm_loadu_ps(b1), _mm_loadu_ps(b1.add(4)), _mm_loadu_ps(b1.add(8)), _mm_loadu_ps(b1.add(12))];
        for i in 0..F {
            let xi = _mm_set1_ps(*x.get_unchecked(i));
            let row = w1.add(i * H);
            for (k, a) in acc.iter_mut().enumerate() {
                *a = _mm_add_ps(*a, _mm_mul_ps(xi, _mm_loadu_ps(row.add(4 * k))));
            }
        }
        let zero = _mm_setzero_ps();
        let mut s = zero;
        for (k, a) in acc.iter().enumerate() {
            let r = _mm_max_ps(*a, zero);
            _mm_storeu_ps(h.as_mut_ptr().add(4 * k), r);
            s = _mm_add_ps(s, _mm_mul_ps(r, _mm_loadu_ps(w2.add(4 * k))));
        }
        hsum128(s) + w.b2
    }

    #[target_feature(enable = "sse2")]
    pub unsafe fn backward_row_sse(w: &Weights, x: &[f32], h: &[f32], ds: f32, g: &mut Weights) {
        g.b2 += ds;
        let d = _mm_set1_ps(ds);
        let zero = _mm_setzero_ps();
        let mut dh = [zero; 4];
        for (k, dhk) in dh.iter_mut().enumerate() {
            let hk = _mm_loadu_ps(h.as_ptr().add(4 * k));
            let gw2 = g.w2.as_mut_ptr().add(4 * k);
            _mm_storeu_ps(gw2, _mm_add_ps(_mm_loadu_ps(gw2), _mm_mul_ps(d, hk)));
            let on = _mm_cmpgt_ps(hk, zero);
            *dhk = _mm_and_ps(on, _mm_mul_ps(d, _mm_loadu_ps(w.w2.as_ptr().add(4 * k))));
            let gb1 = g.b1.as_mut_ptr().add(4 * k);
            _mm_storeu_ps(gb1, _mm_add_ps(_mm_loadu_ps(gb1), *dhk));
        }
        for i in 0..F {
            let xi = _mm_set1_ps(*x.get_unchecked(i));
            let row = g.w1.as_mut_ptr().add(i * H);
            for (k, dhk) in dh.iter().enumerate() {
                let p = row.add(4 * k);
                _mm_storeu_ps(p, _mm_add_ps(_mm_loadu_ps(p), _mm_mul_ps(xi, *dhk)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weights and rows from a little generator: the same each run.
    fn noise(seed: u32, n: usize) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn simd_does_the_reference_arithmetic() {
        let w = Weights::from_flat(&noise(7, PARAMS));
        let rows = 33;
        let x = noise(11, rows * F);
        let (mut h0, mut h1) = (vec![0.0; rows * H], vec![0.0; rows * H]);
        let (mut s0, mut s1) = (vec![0.0; rows], vec![0.0; rows]);
        forward(Cpu::Scalar, &w, &x, &mut h0, &mut s0);
        forward(Cpu::Simd, &w, &x, &mut h1, &mut s1);
        for (a, b) in s0.iter().zip(&s1).chain(h0.iter().zip(&h1)) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b} ({})", simd_name());
        }
        let ds = noise(13, rows);
        let (mut g0, mut g1) = (Weights::zeros(), Weights::zeros());
        backward(Cpu::Scalar, &w, &x, &h0, &ds, &mut g0);
        backward(Cpu::Simd, &w, &x, &h1, &ds, &mut g1);
        for (a, b) in g0.flat().iter().zip(g1.flat()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn the_baseline_weighs_its_features_as_written() {
        let w = Weights::baseline();
        let mut x = [0.0; F];
        x[5] = 1.0;
        let (mut h, mut s) = ([0.0; H], [0.0]);
        forward(Cpu::Simd, &w, &x, &mut h, &mut s);
        assert_eq!(s[0], w.w2[5]);
        assert!(w.sound() && Weights::from_flat(&w.flat()) == w);
    }
}
