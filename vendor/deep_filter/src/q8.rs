//! Olo: the big matrix-vector products in 8 bits. DeepFilterNet's GRUs
//! (512 wide in DeepFilterNet3-LL: twelve 1536 × 512 matrices, 19 MB in
//! f16) are read whole every frame for one vector each, so a frame's time
//! is the time to stream their weights (each one 58 µs in f16 at 27 GB/s
//! on one core, 2026-11-07). Here each weight is a byte, scaled by its row
//! (the largest of the row is ±127), and the vector is made 15-bit
//! integers (scaled by its largest, ±16383): half the bytes of f16, small
//! enough to stay in the last-level cache, and summed as integers. With
//! AVX-VNNI the vector is split into its high and low bytes, and each
//! takes one `vpdpbusd` for 32 weights (the weights' bytes as they are);
//! with AVX2 alone the weights are widened to 16 bits for `vpmaddwd`;
//! elsewhere a plain loop. All three give the same sums, to the bit.
//!
//! `q8_weights` puts a `Q8MatVec` in place of each such product of a
//! typed model (inside scans too): an EinSum of a constant 2-D f32 weight
//! and an activation that is one vector, whose output is one vector.

use std::sync::Arc;

use tract_core::internal::*;
use tract_core::ops::einsum::EinSum;
use tract_core::{args_1, as_op, op_as_typed_op, op_out_of_plan};

/// Rows are taken in fours, and each row's weights 32 at a time.
const ROWS: usize = 4;
const LANES: usize = 32;
/// The vector's largest as an integer; a row's sum, up to `MAX_K`
/// products of ±127 × ±16383, fits an i32.
const X_MAX: f32 = 16383.0;
const MAX_K: usize = 1024;
/// Only matrices at least this big (smaller ones stay in tract's kernels).
const MIN_LEN: usize = 65536;

/// How the sums are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    Vnni,
    Avx2,
    Plain,
}

impl Kernel {
    /// The best this processor has.
    pub fn best() -> Kernel {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                return if std::is_x86_feature_detected!("avxvnni") { Kernel::Vnni } else { Kernel::Avx2 };
            }
        }
        Kernel::Plain
    }
}

/// A matrix of `n` rows of `k`, in bytes: each block of four rows stored
/// 32 columns of each row at a time (two cache lines a step), `k` rounded
/// up to 32 and `n` to four with zeros.
pub struct Q8 {
    n: usize,
    k: usize,
    kp: usize,
    q: Vec<i8>,
    /// Each row's scale (a weight is `q × scale`), and the sum of its bytes.
    scale: Vec<f32>,
    sum: Vec<i32>,
    kernel: Kernel,
}

impl std::fmt::Debug for Q8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Q8({}×{}, {:?})", self.n, self.k, self.kernel)
    }
}

impl Q8 {
    /// `w[r][c]` is `at(r, c)`, for `n` rows of `k` (at most `MAX_K`).
    pub fn new(n: usize, k: usize, at: impl Fn(usize, usize) -> f32) -> Q8 {
        assert!(k <= MAX_K);
        let kp = k.div_ceil(LANES) * LANES;
        let np = n.div_ceil(ROWS) * ROWS;
        let mut q = vec![0i8; np * kp];
        let mut scale = vec![0f32; np];
        let mut sum = vec![0i32; np];
        for r in 0..n {
            let most = (0..k).map(|c| at(r, c).abs()).fold(0f32, f32::max);
            let s = if most > 0.0 { most / 127.0 } else { 1.0 };
            scale[r] = s;
            let (block, row) = (r / ROWS, r % ROWS);
            for c in 0..k {
                let v = (at(r, c) / s).round().clamp(-127.0, 127.0) as i8;
                q[block * ROWS * kp + (c / LANES) * ROWS * LANES + row * LANES + c % LANES] = v;
                sum[r] += v as i32;
            }
        }
        Q8 { n, k, kp, q, scale, sum, kernel: Kernel::best() }
    }

    /// Summed by `kernel` (one this processor has), to compare them.
    pub fn with(mut self, kernel: Kernel) -> Q8 {
        self.kernel = kernel;
        self
    }

    /// The weights' bytes.
    pub fn bytes(&self) -> usize {
        self.q.len() + self.scale.len() * 4 + self.sum.len() * 4
    }

    /// `y = W x`.
    pub fn matvec(&self, x: &[f32], y: &mut [f32]) {
        assert!(x.len() == self.k && y.len() == self.n);
        let most = x.iter().fold(0f32, |m, v| m.max(v.abs()));
        if !most.is_finite() {
            // Not a number in, not a number out (as the float product).
            y.fill(f32::NAN);
            return;
        }
        if most == 0.0 {
            y.fill(0.0);
            return;
        }
        let sx = most / X_MAX;
        thread_local!(static SCRATCH: std::cell::RefCell<Scratch> = const { std::cell::RefCell::new(Scratch { x: Vec::new(), high: Vec::new(), low: Vec::new(), sums: Vec::new() }) });
        SCRATCH.with_borrow_mut(|t| {
            t.x.clear();
            t.x.extend(x.iter().map(|v| (v / sx).round() as i16));
            t.x.resize(self.kp, 0);
            t.sums.clear();
            t.sums.resize(self.sum.len(), 0);
            match self.kernel {
                #[cfg(target_arch = "x86_64")]
                Kernel::Vnni => {
                    // The vector as its high byte (offset to be unsigned:
                    // the rows' sums take the offset back) and its low.
                    t.high.clear();
                    t.high.extend(t.x.iter().map(|&v| ((v >> 8) + 128) as u8));
                    t.low.clear();
                    t.low.extend(t.x.iter().map(|&v| v as u8));
                    // SAFETY: AVX-VNNI is there (`Kernel::best`); `q` holds
                    // `sums.len() / 4` blocks of 4 × `kp`, `high` and `low`
                    // hold `kp`.
                    unsafe { sums_vnni(&self.q, self.kp, &t.high, &t.low, &self.sum, &mut t.sums) }
                }
                #[cfg(target_arch = "x86_64")]
                Kernel::Avx2 => {
                    // SAFETY: AVX2 is there; the sizes as above.
                    unsafe { sums_avx2(&self.q, self.kp, &t.x, &mut t.sums) }
                }
                _ => sums_plain(&self.q, self.kp, &t.x, &mut t.sums),
            }
            for ((y, s), w) in y.iter_mut().zip(&t.sums).zip(&self.scale) {
                *y = *s as f32 * w * sx;
            }
        })
    }
}

struct Scratch {
    x: Vec<i16>,
    high: Vec<u8>,
    low: Vec<u8>,
    sums: Vec<i32>,
}

/// Each row's sum of its weights times `x`, four rows at a time.
fn sums_plain(q: &[i8], kp: usize, x: &[i16], out: &mut [i32]) {
    for (b, out) in out.chunks_exact_mut(ROWS).enumerate() {
        let block = &q[b * ROWS * kp..(b + 1) * ROWS * kp];
        for (step, w) in block.chunks_exact(ROWS * LANES).enumerate() {
            let x = &x[step * LANES..(step + 1) * LANES];
            for (r, out) in out.iter_mut().enumerate() {
                *out += w[r * LANES..(r + 1) * LANES].iter().zip(x).map(|(&w, &x)| w as i32 * x as i32).sum::<i32>();
            }
        }
    }
}

/// Four rows' eight lanes each added up: after two horizontal adds each
/// half holds the four rows' sums of its half.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn add_up(a0: std::arch::x86_64::__m256i, a1: std::arch::x86_64::__m256i, a2: std::arch::x86_64::__m256i, a3: std::arch::x86_64::__m256i) -> std::arch::x86_64::__m128i {
    use std::arch::x86_64::*;
    let s = _mm256_hadd_epi32(_mm256_hadd_epi32(a0, a1), _mm256_hadd_epi32(a2, a3));
    _mm_add_epi32(_mm256_castsi256_si128(s), _mm256_extracti128_si256::<1>(s))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn sums_avx2(q: &[i8], kp: usize, x: &[i16], out: &mut [i32]) {
    use std::arch::x86_64::*;
    let steps = kp / LANES;
    let mut w = q.as_ptr();
    let xp = x.as_ptr();
    for out in out.chunks_exact_mut(ROWS) {
        let mut a = [_mm256_setzero_si256(); ROWS];
        for s in 0..steps {
            // SAFETY: within `x` (`kp` long) and the block (`ROWS × kp`).
            unsafe {
                let x0 = _mm256_loadu_si256(xp.add(s * LANES) as *const __m256i);
                let x1 = _mm256_loadu_si256(xp.add(s * LANES + 16) as *const __m256i);
                for (r, a) in a.iter_mut().enumerate() {
                    let w0 = _mm256_cvtepi8_epi16(_mm_loadu_si128(w.add(r * LANES) as *const __m128i));
                    let w1 = _mm256_cvtepi8_epi16(_mm_loadu_si128(w.add(r * LANES + 16) as *const __m128i));
                    *a = _mm256_add_epi32(*a, _mm256_add_epi32(_mm256_madd_epi16(w0, x0), _mm256_madd_epi16(w1, x1)));
                }
                w = w.add(ROWS * LANES);
            }
        }
        // SAFETY: `out` is four long.
        unsafe { _mm_storeu_si128(out.as_mut_ptr() as *mut __m128i, add_up(a[0], a[1], a[2], a[3])) };
    }
}

/// As `sums_avx2`, with the vector in bytes: `high` (its high byte + 128)
/// and `low`; a row's sum is 256 × (high · w − 128 × Σw) + low · w.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,avxvnni")]
unsafe fn sums_vnni(q: &[i8], kp: usize, high: &[u8], low: &[u8], sum: &[i32], out: &mut [i32]) {
    use std::arch::x86_64::*;
    let steps = kp / LANES;
    let mut w = q.as_ptr();
    for (b, out) in out.chunks_exact_mut(ROWS).enumerate() {
        let mut h = [_mm256_setzero_si256(); ROWS];
        let mut l = [_mm256_setzero_si256(); ROWS];
        for s in 0..steps {
            // SAFETY: within `high` and `low` (`kp` long) and the block.
            unsafe {
                let hv = _mm256_loadu_si256(high.as_ptr().add(s * LANES) as *const __m256i);
                let lv = _mm256_loadu_si256(low.as_ptr().add(s * LANES) as *const __m256i);
                for r in 0..ROWS {
                    let wv = _mm256_loadu_si256(w.add(r * LANES) as *const __m256i);
                    h[r] = _mm256_dpbusd_avx_epi32(h[r], hv, wv);
                    l[r] = _mm256_dpbusd_avx_epi32(l[r], lv, wv);
                }
                w = w.add(ROWS * LANES);
            }
        }
        // SAFETY: `sum` and `out` hold this block's four rows.
        unsafe {
            let s = _mm_loadu_si128(sum.as_ptr().add(b * ROWS) as *const __m128i);
            let hs = _mm_sub_epi32(add_up(h[0], h[1], h[2], h[3]), _mm_slli_epi32::<7>(s));
            let v = _mm_add_epi32(_mm_slli_epi32::<8>(hs), add_up(l[0], l[1], l[2], l[3]));
            _mm_storeu_si128(out.as_mut_ptr() as *mut __m128i, v);
        }
    }
}

/// `W x` for one vector `x`, its output in `out`'s shape.
#[derive(Debug, Clone)]
pub struct Q8MatVec {
    pub w: Arc<Q8>,
    pub out: TVec<usize>,
}

impl PartialEq for Q8MatVec {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.w, &other.w) && self.out == other.out
    }
}
impl Eq for Q8MatVec {}

impl Op for Q8MatVec {
    fn name(&self) -> StaticName {
        "Q8MatVec".into()
    }

    fn info(&self) -> TractResult<Vec<String>> {
        Ok(vec![format!("{:?}", self.w)])
    }

    op_as_typed_op!();
}

impl EvalOp for Q8MatVec {
    op_out_of_plan!();

    fn eval(&self, _ctx: &EvalContext, inputs: TVec<TValue>) -> TractResult<TVec<TValue>> {
        let x = args_1!(inputs);
        let x = x.try_as_plain_ram()?;
        let mut y = Tensor::zero::<f32>(&self.out)?;
        self.w.matvec(x.as_slice::<f32>()?, y.try_as_plain_ram_mut()?.as_slice_mut::<f32>()?);
        Ok(tvec!(y.into_tvalue()))
    }
}

impl TypedOp for Q8MatVec {
    fn output_facts(&self, _inputs: &[&TypedFact]) -> TractResult<TVec<TypedFact>> {
        Ok(tvec!(f32::fact(&*self.out)))
    }

    as_op!();
}

/// Each EinSum of a typed model (and its scans' bodies) that multiplies a
/// constant f32 matrix (of at least `MIN_LEN`) by one vector made a
/// `Q8MatVec`; how many.
pub fn q8_weights(model: &mut TypedModel) -> TractResult<usize> {
    let mut changed = 0;
    for node in model.nodes.iter_mut() {
        if let Some(scan) = node.op_as_mut::<tract_core::ops::scan::Scan>() {
            changed += q8_weights(&mut scan.body)?;
        }
    }
    let ids: Vec<usize> = model.nodes.iter().filter(|n| n.op_is::<EinSum>()).map(|n| n.id).collect();
    for id in ids {
        let node = model.node(id).clone();
        let op = node.op_as::<EinSum>().unwrap();
        if op.operating_dt != f32::datum_type() || op.q_params.is_some() || node.inputs.len() != 2 || node.outputs.len() != 1 {
            continue;
        }
        let out = model.outlet_fact(OutletId::new(id, 0))?;
        let Some(os) = out.shape.as_concrete().map(|s| s.to_vec()) else { continue };
        for w in 0..2 {
            let a = 1 - w;
            let Some(konst) = model.outlet_fact(node.inputs[w])?.konst.clone() else { continue };
            if konst.rank() != 2 || konst.datum_type() != f32::datum_type() || konst.len() < MIN_LEN {
                continue;
            }
            let x = model.outlet_fact(node.inputs[a])?;
            if x.datum_type != f32::datum_type() || x.konst.is_some() {
                continue;
            }
            let Some(xs) = x.shape.as_concrete().map(|s| s.to_vec()) else { continue };
            if xs.is_empty() || os.is_empty() {
                continue;
            }
            // The weight's summed axis is the vector's last, and its other
            // the output's last (the rest of both all ones).
            let is_k = |ax: &tract_core::axes::Axis| ax.inputs[a].as_slice() == [xs.len() - 1] && ax.outputs[0].is_empty();
            let is_n = |ax: &tract_core::axes::Axis| ax.inputs[a].is_empty() && ax.outputs[0].as_slice() == [os.len() - 1];
            let (ax0, ax1) = (op.axes.axis((InOut::In(w), 0))?, op.axes.axis((InOut::In(w), 1))?);
            let transposed = if is_n(ax0) && is_k(ax1) {
                false
            } else if is_k(ax0) && is_n(ax1) {
                true
            } else {
                continue;
            };
            let (n, k) = if transposed { (konst.shape()[1], konst.shape()[0]) } else { (konst.shape()[0], konst.shape()[1]) };
            if xs.iter().product::<usize>() != k || os.iter().product::<usize>() != n || k > MAX_K {
                continue;
            }
            let plain = konst.try_as_plain_ram()?;
            let v = plain.as_slice::<f32>()?;
            let q = if transposed { Q8::new(n, k, |r, c| v[c * n + r]) } else { Q8::new(n, k, |r, c| v[r * k + c]) };
            let mut patch = TypedModelPatch::default();
            let tap = patch.tap_model(model, node.inputs[a])?;
            let y = patch.wire_node(&node.name, Q8MatVec { w: Arc::new(q), out: os.iter().copied().collect() }, &[tap])?;
            patch.shunt_outside(model, OutletId::new(id, 0), y[0])?;
            patch.apply(model)?;
            changed += 1;
            break;
        }
    }
    Ok(changed)
}
