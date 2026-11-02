//! The inverse transform: the middle half of an inverse MDCT of N lines,
//! which is a DCT-IV reversed and negated, done with an N/2-point complex
//! FFT (rustfft: NEON or AVX as the processor has).

use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex32};

pub struct Imdct {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    pre: Vec<Complex32>,
    post: Vec<Complex32>,
    work: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Imdct {
    pub fn new(n: usize) -> Imdct {
        let m = n / 2;
        let fft = FftPlanner::new().plan_fft_forward(m);
        let pi = std::f64::consts::PI;
        let tw = |a: f64| Complex32::new(a.cos() as f32, -a.sin() as f32);
        Imdct {
            n,
            pre: (0..m).map(|k| tw(pi * (4 * k + 1) as f64 / (4 * n) as f64)).collect(),
            post: (0..m).map(|k| tw(pi * k as f64 / n as f64)).collect(),
            work: vec![Complex32::default(); m],
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
        }
    }

    /// `out[j]` = the inverse MDCT of `x` at `j + N/2`, times `scale`.
    pub fn half(&mut self, x: &[f32], out: &mut [f32], scale: f32) {
        let (n, m) = (self.n, self.n / 2);
        for k in 0..m {
            self.work[k] = Complex32::new(x[2 * k], x[n - 1 - 2 * k]) * self.pre[k];
        }
        self.fft.process_with_scratch(&mut self.work, &mut self.scratch);
        // u = DCT-IV(x); out[j] = -u[N - 1 - j].
        for k in 0..m {
            let d = self.work[k] * self.post[k];
            out[n - 1 - 2 * k] = -d.re * scale;
            out[2 * k] = d.im * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_the_definition() {
        let n = 480;
        let x: Vec<f32> = (0..n).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let mut out = vec![0f32; n];
        super::Imdct::new(n).half(&x, &mut out, 1.0);
        let pi = std::f64::consts::PI;
        for j in [0, 1, 17, 239, 240, 241, 479] {
            let t = (j + n / 2) as f64;
            let want: f64 = (0..n).map(|k| x[k] as f64 * (pi / n as f64 * (t + 0.5 + n as f64 / 2.0) * (k as f64 + 0.5)).cos()).sum();
            assert!((out[j] as f64 - want).abs() < 1e-3, "{j}: {} vs {want}", out[j]);
        }
    }
}
