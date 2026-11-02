//! From the AirPods' microphone clock to the sound server's: a
//! windowed-sinc resampler (Kaiser, 32 taps, 512 phases interpolated)
//! whose ratio is steered, a few hundredths of a percent either way, to
//! keep what's waiting at its target. Two clocks never agree exactly; this
//! follows the difference smoothly, rather than dropping or repeating
//! samples (a click each time). If the input ever runs dry (Bluetooth
//! silent for longer than the buffer), the output fades out over 5 ms and
//! back in once there is enough again, never cut.

use std::collections::VecDeque;

/// Half the taps.
const HALF: usize = 16;
const TAPS: usize = 2 * HALF;
const PHASES: usize = 512;
/// The most the ratio is steered (0.5%; drift is far smaller).
const MOST_CORRECTION: f64 = 0.005;

pub struct Resampler {
    /// Input rate over output rate.
    nominal: f64,
    /// TAPS coefficients for each of PHASES + 1 fractional positions.
    table: Vec<f32>,
    /// Input not yet used, and where the next output falls in it (the
    /// first `HALF - 1` samples are the filter's past).
    input: VecDeque<f32>,
    pos: f64,
    /// What should wait, in input samples, and the steering's smoothed
    /// error.
    target: usize,
    error: f64,
    primed: bool,
    /// The fade: 1 playing, falling to 0 when dry, rising when back.
    gain: f32,
    last: f32,
    /// The fade's step a sample (5 ms).
    fade: f32,
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..30 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

impl Resampler {
    /// From `from` Hz to `to` Hz, keeping `target` input samples waiting.
    pub fn new(from: u32, to: u32, target: usize) -> Resampler {
        // A little below the lower Nyquist, in cycles per input sample.
        let cutoff = 0.45 * (to.min(from) as f64 / from as f64);
        let beta = 8.0;
        let mut table = Vec::with_capacity((PHASES + 1) * TAPS);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let mut row = [0f64; TAPS];
            for (j, h) in row.iter_mut().enumerate() {
                // Tap j is input sample (i - HALF + 1 + j); the output
                // falls at i + frac.
                let t = j as f64 - (HALF as f64 - 1.0) - frac;
                let x = 2.0 * cutoff * t;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let w = (t / HALF as f64).abs();
                let window = if w >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - w * w).sqrt()) / bessel_i0(beta) };
                *h = 2.0 * cutoff * sinc * window;
            }
            let sum: f64 = row.iter().sum();
            table.extend(row.iter().map(|h| (h / sum) as f32));
        }
        Resampler {
            nominal: from as f64 / to as f64,
            table,
            input: VecDeque::from(vec![0.0; HALF - 1]),
            pos: 0.0,
            target,
            error: 0.0,
            primed: false,
            gain: 0.0,
            last: 0.0,
            fade: 1.0 / (0.005 * to as f32),
        }
    }

    /// Input samples waiting (beyond the filter's past).
    pub fn waiting(&self) -> usize {
        (self.input.len() as f64 - self.pos - (HALF - 1) as f64).max(0.0) as usize
    }

    /// Take input.
    pub fn push(&mut self, samples: impl IntoIterator<Item = f32>) {
        self.input.extend(samples);
    }

    /// Keep `target` input samples waiting from now on.
    pub fn set_target(&mut self, target: usize) {
        self.target = target.max(1);
    }

    /// Forget everything (a new stream).
    pub fn reset(&mut self) {
        self.input.clear();
        self.input.extend(std::iter::repeat_n(0.0, HALF - 1));
        (self.pos, self.error, self.primed, self.gain, self.last) = (0.0, 0.0, false, 0.0, 0.0);
    }

    /// Fill `out` (the output rate), steering towards the target.
    pub fn pull(&mut self, out: &mut [f32]) {
        let waiting = self.waiting();
        if !self.primed && waiting >= self.target {
            self.primed = true;
        }
        // Steer: smoothed (about 0.4 s) and proportional, so the jitter of
        // Bluetooth's bursts moves the pitch by a cent or two at most.
        let e = (waiting as f64 - self.target as f64) / self.target as f64;
        self.error += 0.05 * (e - self.error);
        let step = self.nominal * (1.0 + (0.002 * self.error).clamp(-MOST_CORRECTION, MOST_CORRECTION));
        let fade = self.fade;
        for o in out.iter_mut() {
            let i = self.pos as usize;
            let have = self.primed && i + TAPS <= self.input.len();
            let v = if have {
                let frac = self.pos - i as f64;
                let p = frac * PHASES as f64;
                let (pi, pf) = (p as usize, (p - p.floor()) as f32);
                let (a, b) = (&self.table[pi * TAPS..pi * TAPS + TAPS], &self.table[(pi + 1) * TAPS..(pi + 2) * TAPS]);
                let (s0, s1) = self.input.as_slices();
                let mut acc = 0f32;
                for j in 0..TAPS {
                    let k = i + j;
                    let x = if k < s0.len() { s0[k] } else { s1[k - s0.len()] };
                    acc += x * (a[j] + (b[j] - a[j]) * pf);
                }
                self.pos += step;
                self.gain = (self.gain + fade).min(1.0);
                self.last = acc;
                acc
            } else {
                // Dry: fade what was last heard; start again once primed.
                self.primed = false;
                self.gain = (self.gain - fade).max(0.0);
                self.last
            };
            *o = v * self.gain;
        }
        // Let go of what's behind the filter.
        let used = (self.pos as usize).min(self.input.len());
        self.input.drain(..used);
        self.pos -= used as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_comes_through_at_the_new_rate() {
        // 1 kHz at 64 kHz in, 48 kHz out: still 1 kHz, full level.
        let mut r = Resampler::new(64_000, 48_000, 3840);
        let tone = |n: usize| (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 64_000.0).sin();
        r.push((0..64_000).map(tone));
        let mut out = vec![0f32; 24_000];
        r.pull(&mut out);
        let tail = &out[12_000..];
        let peak = tail.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
        // Zero crossings: 2 a cycle, 1000 cycles a second.
        let crossings = tail.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((crossings as i32 - 250).abs() <= 1, "{crossings}");
    }

    #[test]
    fn dry_fades_rather_than_cuts() {
        let mut r = Resampler::new(64_000, 48_000, 480);
        r.push(std::iter::repeat_n(0.5, 4000));
        let mut out = vec![0f32; 4000];
        r.pull(&mut out);
        // Ran out part way: the output falls off over ~5 ms, no step.
        let steps = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(steps < 0.01, "largest step {steps}");
        assert!(*out.last().unwrap() < 0.01);
    }

    #[test]
    fn follows_a_faster_clock_without_dropping() {
        // The input 0.2% fast: the buffer stays near its target.
        let mut r = Resampler::new(64_000, 48_000, 3840);
        r.push(std::iter::repeat_n(0.1, 3840));
        let mut out = vec![0f32; 1024];
        for _ in 0..2000 {
            let n = (1024.0 * 64_000.0 / 48_000.0 * 1.002) as usize;
            r.push(std::iter::repeat_n(0.1, n));
            r.pull(&mut out);
        }
        let w = r.waiting();
        assert!((3000..6000).contains(&w), "waiting {w}");
        assert!(out.iter().all(|x| (x - 0.1).abs() < 1e-3));
    }
}
