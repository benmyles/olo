//! Echo cancellation: what the speakers play, taken back out of what the
//! microphone hears.
//!
//! Three parts, each frame (10 ms, 480 samples at 48 kHz, as the rest of
//! the chain; the canceller adds no delay):
//!
//! 1. **Where the echo is**: the coherence between the microphone and the
//!    speakers at each delay up to [`SPAN`] frames (500 ms: the sound
//!    server's buffers, a Bluetooth speaker's, the air), smoothed over
//!    about a second. The filter is placed a little before the strongest.
//! 2. **The echo path**: a partitioned-block frequency-domain Kalman filter
//!    (Enzner & Vary 2006; partitioned as Kuech et al. 2014), [`TAPS`]
//!    frames (120 ms of room) long, over the whole band: an adaptive model
//!    of the path from speakers to microphone whose step each frequency
//!    works out from how sure it is of the path and how much else is
//!    heard. It adapts in the background; what's heard comes from a
//!    foreground copy, taken from it only while it does clearly better,
//!    and given back when talking over the speakers throws it off (Valin's
//!    two-path scheme, from Speex): fast to converge and to follow a path
//!    that changes, steady while both people talk.
//! 3. **What's left**: [`EchoCanceller::gains`], per frequency, a Wiener
//!    gain from the filter's own estimate of the echo it left (and a share
//!    of what it predicted, for the loudspeaker's distortion), which the
//!    near voice isn't part of, so it keeps its level. Applied in the noise
//!    model's STFT (`Voice`), so no second window.
//!
//! Speed: the FFTs are rustfft's (NEON, AVX, SSE, chosen at run time);
//! everything per frequency is kept as separate real and imaginary arrays
//! and written as straight loops over equal slices, which the compiler
//! turns into NEON or AVX/SSE vector code. A GPU is no help here: a frame
//! is ~50 µs of work, less than one round trip to a GPU queue, and the
//! frames depend on each other in order.

use std::collections::VecDeque;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::FRAME;

/// Frequencies (bins) of a two-frame FFT.
pub const BINS: usize = FRAME + 1;
/// How far back the echo is looked for, in frames.
pub const SPAN: usize = 50;
/// How long the filter is, in frames (partitions).
pub const TAPS: usize = 12;
/// Frames of margin the filter starts before the strongest echo.
const MARGIN: usize = 2;

/// The Kalman filter's state transition: how much of the path is expected
/// to hold from one frame to the next.
const A: f32 = 0.999;
/// The starting uncertainty of every tap.
const P_START: f32 = 10.0;
/// See [`EchoCanceller::adapt`].
const NOISE_WEIGHT: f32 = 0.5;
/// How fast the error's spectrum is smoothed.
const SMOOTH: f32 = 0.5;
/// What the linear model misses beyond what it knows it misses, as a share
/// of the echo it predicts (the loudspeaker's distortion): -30 dB. (Speex's
/// running estimate of it was tried: a near voice talking over music
/// drives it to 0 dB within a second.)
const LEAK: f32 = 0.001;
/// Over-subtraction of the residual echo estimate.
const OVER: f32 = 2.0;
/// The least gain residual suppression leaves (-40 dB).
const FLOOR: f32 = 0.01;
/// The delay estimate's smoothing (about a second of frames with sound).
const COHERENCE_SMOOTH: f32 = 0.99;
/// The bins the delay is judged on: 300 Hz to 4 kHz (where speech and
/// music both have their energy).
const JUDGED: std::ops::Range<usize> = 6..80;
const JUDGED_N: usize = JUDGED.end - JUDGED.start;

/// The sum of `f` over `v`, in eight lanes (a plain float sum is a chain
/// the compiler mustn't reorder, so it wouldn't vectorize).
#[inline]
fn lanes_sum(v: &[f32], f: impl Fn(f32) -> f32) -> f32 {
    let mut acc = [0f32; 8];
    let (chunks, rest) = v.as_chunks::<8>();
    for c in chunks {
        for (a, x) in acc.iter_mut().zip(c) {
            *a += f(*x);
        }
    }
    acc.iter().sum::<f32>() + rest.iter().map(|x| f(*x)).sum::<f32>()
}

fn bins(s: &[f32]) -> &[f32; BINS] {
    s.try_into().expect("a block of BINS")
}
fn bins_mut(s: &mut [f32]) -> &mut [f32; BINS] {
    s.try_into().expect("a block of BINS")
}
fn judged_n(s: &[f32]) -> &[f32; JUDGED_N] {
    s.try_into().expect("a block of JUDGED_N")
}
fn judged_n_mut(s: &mut [f32]) -> &mut [f32; JUDGED_N] {
    s.try_into().expect("a block of JUDGED_N")
}

/// A spectrum, real and imaginary parts apart (so loops over it vectorize).
#[derive(Clone)]
struct Split {
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Split {
    fn zeros(n: usize) -> Split {
        Split { re: vec![0.0; n], im: vec![0.0; n] }
    }
}

pub struct EchoCanceller {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    /// The speakers' spectra, newest first: a ring of SPAN blocks of BINS,
    /// `head` the newest; and their powers.
    x: Split,
    x_power: Vec<f32>,
    head: usize,
    /// Where the filter starts, in frames of delay.
    offset: usize,
    /// The filter (adapting) and its uncertainty, a block of BINS per
    /// partition; and the foreground copy the output comes from.
    w: Split,
    p: Vec<f32>,
    fore: Split,
    /// Smoothed differences of the foreground's and background's error
    /// energies (fast and slow), and their spreads.
    ahead_fast: f32,
    ahead_slow: f32,
    spread_fast: f32,
    spread_slow: f32,
    /// The delay estimate: per delay and judged bin, the smoothed
    /// cross-spectrum with the microphone, and the powers.
    cross: Split,
    cross_x: Vec<f32>,
    cross_d: Vec<f32>,
    /// The delay that has led, and for how many frames.
    leader: usize,
    lead: usize,
    /// The speakers' and the microphone's last frames (the first halves of
    /// the next FFTs).
    far_last: [f32; FRAME],
    mic_last: [f32; FRAME],
    /// Frames from the speakers, not yet matched with the microphone's.
    far_waiting: VecDeque<[f32; FRAME]>,
    /// Which partition's gradient is constrained this frame.
    turn: usize,
    error: Split,
    error_power: Vec<f32>,
    gains: [f32; BINS],
    /// Frames since the speakers last played anything.
    quiet: usize,
    // Scratch.
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    echo: Split,
    residual: Vec<f32>,
    echo_power: Vec<f32>,
    raw: Vec<f32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
}

impl std::fmt::Debug for EchoCanceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EchoCanceller").field("delay", &self.delay()).finish()
    }
}

impl Default for EchoCanceller {
    fn default() -> EchoCanceller {
        EchoCanceller::new()
    }
}

impl EchoCanceller {
    pub fn new() -> EchoCanceller {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(2 * FRAME);
        let inverse = planner.plan_fft_inverse(2 * FRAME);
        let scratch_f = forward.make_scratch_vec();
        let scratch_i = inverse.make_scratch_vec();
        EchoCanceller {
            forward,
            inverse,
            x: Split::zeros(SPAN * BINS),
            x_power: vec![0.0; SPAN * BINS],
            head: 0,
            offset: 0,
            w: Split::zeros(TAPS * BINS),
            p: vec![P_START; TAPS * BINS],
            fore: Split::zeros(TAPS * BINS),
            ahead_fast: 0.0,
            ahead_slow: 0.0,
            spread_fast: 0.0,
            spread_slow: 0.0,
            cross: Split::zeros(SPAN * JUDGED_N),
            cross_x: vec![0.0; SPAN * JUDGED_N],
            cross_d: vec![0.0; JUDGED_N],
            leader: 0,
            lead: 0,
            far_last: [0.0; FRAME],
            mic_last: [0.0; FRAME],
            far_waiting: VecDeque::with_capacity(8),
            turn: 0,
            error: Split::zeros(BINS),
            error_power: vec![0.0; BINS],
            gains: [1.0; BINS],
            quiet: usize::MAX / 2,
            time: vec![0.0; 2 * FRAME],
            spectrum: vec![Complex32::default(); BINS],
            echo: Split::zeros(BINS),
            residual: vec![0.0; BINS],
            echo_power: vec![0.0; BINS],
            raw: vec![0.0; BINS],
            scratch_f,
            scratch_i,
        }
    }

    /// A frame the speakers play.
    pub fn render(&mut self, frame: &[f32; FRAME]) {
        // Matched one for one with the microphone's; if the microphone has
        // stopped asking, don't hoard.
        if self.far_waiting.len() >= 8 {
            self.far_waiting.pop_front();
        }
        self.far_waiting.push_back(*frame);
    }

    /// Where the filter starts (the echo's delay, less a margin), in
    /// samples.
    pub fn delay(&self) -> usize {
        self.offset * FRAME
    }

    /// The residual echo gains for the frame [`EchoCanceller::cancel`] just
    /// made, per bin of a 960-point FFT.
    pub fn gains(&self) -> &[f32; BINS] {
        &self.gains
    }

    /// `time` (overwritten) → `spectrum`.
    fn fft(&mut self) {
        let _ = self.forward.process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch_f);
    }

    /// `spectrum` → `time`, times 960.
    fn ifft(&mut self) {
        // A real signal's spectrum: the ends are real.
        self.spectrum[0].im = 0.0;
        self.spectrum[BINS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(&mut self.spectrum, &mut self.time, &mut self.scratch_i);
    }

    /// The ring block `delay` frames back.
    fn slot(&self, delay: usize) -> std::ops::Range<usize> {
        let s = ((self.head + delay) % SPAN) * BINS;
        s..s + BINS
    }

    /// Take the echo out of `mic`, into `out`.
    pub fn cancel(&mut self, mic: &[f32; FRAME], out: &mut [f32; FRAME]) {
        let far = self.far_waiting.pop_front().unwrap_or([0.0; FRAME]);
        let norm = 1.0 / (2 * FRAME) as f32;
        let loud = lanes_sum(&far, |v| v * v) > 1e-10 * FRAME as f32;
        self.quiet = if loud { 0 } else { self.quiet.saturating_add(1) };

        // The newest speakers' spectrum: their last two frames.
        self.head = (self.head + SPAN - 1) % SPAN;
        self.time[..FRAME].copy_from_slice(&self.far_last);
        self.time[FRAME..].copy_from_slice(&far);
        self.far_last = far;
        self.fft();
        let s = self.slot(0);
        for (((re, im), pw), c) in self.x.re[s.clone()].iter_mut().zip(&mut self.x.im[s.clone()]).zip(&mut self.x_power[s]).zip(&self.spectrum) {
            (*re, *im, *pw) = (c.re, c.im, c.norm_sqr());
        }

        // Nothing has played for longer than the filter reaches: nothing to
        // cancel.
        if self.quiet > self.offset + TAPS {
            out.copy_from_slice(mic);
            self.mic_last = *mic;
            self.gains.fill(1.0);
            return;
        }

        if loud {
            self.find_delay(mic);
        }
        self.mic_last = *mic;

        // The echo each filter predicts, and the microphone less it.
        let mut background = [0f32; FRAME];
        self.predict(false);
        for (b, (m, t)) in background.iter_mut().zip(mic.iter().zip(&self.time[FRAME..])) {
            *b = m - t * norm;
        }
        self.predict(true);
        let mut predicted = [0f32; FRAME];
        for ((p, o), (m, t)) in predicted.iter_mut().zip(out.iter_mut()).zip(mic.iter().zip(&self.time[FRAME..])) {
            *p = t * norm;
            *o = m - *p;
        }
        // Which does better (two-path, as Speex): the background is copied
        // forward when it has done significantly better (the difference in
        // error energy beats its own spread), and reset from the foreground
        // when it has done significantly worse (someone talking over the
        // speakers threw it off).
        if loud {
            let (fore_e, back_e) = (lanes_sum(out, |x| x * x), lanes_sum(&background, |x| x * x));
            let apart: [f32; FRAME] = std::array::from_fn(|i| out[i] - background[i]);
            let between = lanes_sum(&apart, |x| x * x);
            let diff = fore_e - back_e;
            self.ahead_fast = 0.6 * self.ahead_fast + 0.4 * diff;
            self.ahead_slow = 0.85 * self.ahead_slow + 0.15 * diff;
            self.spread_fast = 0.36 * self.spread_fast + 0.16 * fore_e * between;
            self.spread_slow = 0.7225 * self.spread_slow + 0.0225 * fore_e * between;
            let signif = |d: f32, var: f32, k: f32| d * d.abs() > k * var;
            if signif(diff, fore_e * between, 1.0) || signif(self.ahead_fast, self.spread_fast, 0.5) || signif(self.ahead_slow, self.spread_slow, 0.25) {
                self.fore.re.copy_from_slice(&self.w.re);
                self.fore.im.copy_from_slice(&self.w.im);
                // Crossfade, over the frame, from the old foreground's
                // output to the new one's.
                for (i, ((o, p), (m, b))) in out.iter_mut().zip(predicted.iter_mut()).zip(mic.iter().zip(&background)).enumerate() {
                    let w = 0.5 - 0.5 * (std::f32::consts::PI * (i as f32 + 0.5) / FRAME as f32).cos();
                    *o = (1.0 - w) * *o + w * b;
                    *p = m - *o;
                }
                (self.ahead_fast, self.ahead_slow, self.spread_fast, self.spread_slow) = (0.0, 0.0, 0.0, 0.0);
            } else if signif(-diff, 4.0 * fore_e * between, 1.0) || signif(-self.ahead_fast, 4.0 * self.spread_fast, 1.0) || signif(-self.ahead_slow, 4.0 * self.spread_slow, 1.0) {
                self.w.re.copy_from_slice(&self.fore.re);
                self.w.im.copy_from_slice(&self.fore.im);
                background.copy_from_slice(out);
                (self.ahead_fast, self.ahead_slow, self.spread_fast, self.spread_slow) = (0.0, 0.0, 0.0, 0.0);
            }
        }

        // The background's error spectrum (what it adapts on), the output's
        // power (what residual suppression judges), and the prediction's;
        // zero-padded in front.
        self.time[..FRAME].fill(0.0);
        self.time[FRAME..].copy_from_slice(&background);
        self.fft();
        for ((re, im), c) in self.error.re.iter_mut().zip(&mut self.error.im).zip(&self.spectrum) {
            (*re, *im) = (c.re, c.im);
        }
        self.time[..FRAME].fill(0.0);
        self.time[FRAME..].copy_from_slice(out);
        self.fft();
        for (pw, c) in self.error_power.iter_mut().zip(&self.spectrum) {
            *pw = SMOOTH * *pw + (1.0 - SMOOTH) * c.norm_sqr();
        }
        self.time[..FRAME].fill(0.0);
        self.time[FRAME..].copy_from_slice(&predicted);
        self.fft();
        for (pw, c) in self.echo_power.iter_mut().zip(&self.spectrum) {
            *pw = c.norm_sqr();
        }

        // The echo it expects to have left: its uncertainty, seen through
        // the speakers.
        self.residual.fill(0.0);
        for part in 0..TAPS {
            let xs = self.slot(self.offset + part);
            let ps = part * BINS..(part + 1) * BINS;
            for ((r, p), x) in self.residual.iter_mut().zip(&self.p[ps]).zip(&self.x_power[xs]) {
                *r += p * x;
            }
        }

        if loud {
            self.adapt();
        }

        // Residual echo suppression: a Wiener gain from what's left of the
        // echo against what's there.
        for (((g, r), e), there) in self.raw.iter_mut().zip(&self.residual).zip(&self.echo_power).zip(&self.error_power) {
            let left = OVER * (0.5 * r + LEAK * e);
            let there = there.max(1e-12);
            *g = ((there - left).max(0.0) / there).max(FLOOR);
        }
        // Smoothed across neighbouring bins (no lone tones left), and over
        // time: down at once, up over ~30 ms.
        let raw = &self.raw;
        for (k, gain) in self.gains.iter_mut().enumerate() {
            let g = (raw[k.saturating_sub(1)] + 2.0 * raw[k] + raw[(k + 1).min(BINS - 1)]) * 0.25;
            *gain = if g < *gain { g } else { *gain + 0.3 * (g - *gain) };
        }
    }

    /// The echo the background filter (or the foreground) predicts, into
    /// `time` (its last half, times 960).
    fn predict(&mut self, foreground: bool) {
        self.echo.re.fill(0.0);
        self.echo.im.fill(0.0);
        let w = if foreground { &self.fore } else { &self.w };
        for part in 0..TAPS {
            let s = ((self.head + self.offset + part) % SPAN) * BINS;
            let xs = s..s + BINS;
            let ws = part * BINS..(part + 1) * BINS;
            let (xr, xi) = (&self.x.re[xs.clone()], &self.x.im[xs]);
            let (wr, wi) = (&w.re[ws.clone()], &w.im[ws]);
            for ((((er, ei), (xr, xi)), wr), wi) in self.echo.re.iter_mut().zip(self.echo.im.iter_mut()).zip(xr.iter().zip(xi)).zip(wr).zip(wi) {
                *er += wr * xr - wi * xi;
                *ei += wr * xi + wi * xr;
            }
        }
        for (c, (re, im)) in self.spectrum.iter_mut().zip(self.echo.re.iter().zip(&self.echo.im)) {
            *c = Complex32::new(*re, *im);
        }
        self.ifft();
    }

    /// The Kalman update of the filter and of its uncertainty, then a
    /// partition's gradient constraint.
    fn adapt(&mut self) {
        // How much of the error counts as what isn't echo (the Kalman
        // filter's measurement noise): half. Less and it converges a little
        // sooner but follows a near voice while both talk; more and it is
        // slow to follow a path that changes (measured).
        let noise_weight = NOISE_WEIGHT;
        let a2 = A * A;
        for part in 0..TAPS {
            let xs = self.slot(self.offset + part);
            let ws = part * BINS..(part + 1) * BINS;
            // Fixed-size views: no bounds checks, so the loop vectorizes.
            let (xr, xi, xp) = (bins(&self.x.re[xs.clone()]), bins(&self.x.im[xs.clone()]), bins(&self.x_power[xs]));
            let (wr, wi, p) = (bins_mut(&mut self.w.re[ws.clone()]), bins_mut(&mut self.w.im[ws.clone()]), bins_mut(&mut self.p[ws]));
            let (er, ei) = (bins(&self.error.re), bins(&self.error.im));
            let (residual, error_power) = (bins(&self.residual), bins(&self.error_power));
            for k in 0..BINS {
                let d = residual[k] + noise_weight * error_power[k] + 1e-10;
                let mu = p[k] / d;
                // conj(x) · e · mu
                let gr = (xr[k] * er[k] + xi[k] * ei[k]) * mu;
                let gi = (xr[k] * ei[k] - xi[k] * er[k]) * mu;
                wr[k] = A * (wr[k] + gr);
                wi[k] = A * (wi[k] + gi);
                let keep = (1.0 - 0.5 * mu * xp[k]).clamp(0.0, 1.0);
                p[k] = a2 * keep * p[k] + (1.0 - a2) * (wr[k] * wr[k] + wi[k] * wi[k]);
            }
        }
        // Each partition's filter is a frame long, so its second half in
        // time must be zero: one partition a frame, in turn.
        let norm = 1.0 / (2 * FRAME) as f32;
        let ws = self.turn * BINS..(self.turn + 1) * BINS;
        for (c, (re, im)) in self.spectrum.iter_mut().zip(self.w.re[ws.clone()].iter().zip(&self.w.im[ws.clone()])) {
            *c = Complex32::new(*re, *im);
        }
        self.ifft();
        self.time[FRAME..].fill(0.0);
        self.time[..FRAME].iter_mut().for_each(|v| *v *= norm);
        self.fft();
        for ((re, im), c) in self.w.re[ws.clone()].iter_mut().zip(&mut self.w.im[ws]).zip(&self.spectrum) {
            (*re, *im) = (c.re, c.im);
        }
        self.turn = (self.turn + 1) % TAPS;
    }

    /// Follow where the echo is: the delay with the most coherence between
    /// speakers and microphone; once one outside the filter has led, well
    /// ahead of any inside it, for half a second, move the filter there
    /// (keeping what it has learnt where they overlap).
    fn find_delay(&mut self, mic: &[f32; FRAME]) {
        self.time[..FRAME].copy_from_slice(&self.mic_last);
        self.time[FRAME..].copy_from_slice(mic);
        self.fft();
        let c = COHERENCE_SMOOTH;
        let d = &self.spectrum[JUDGED];
        let (dr, di): ([f32; JUDGED_N], [f32; JUDGED_N]) = (std::array::from_fn(|j| d[j].re), std::array::from_fn(|j| d[j].im));
        for (cd, (r, i)) in self.cross_d.iter_mut().zip(dr.iter().zip(&di)) {
            *cd = c * *cd + (1.0 - c) * (r * r + i * i);
        }
        let mut best = (0, 0f32);
        let mut scores = [0f32; SPAN];
        for (delay, slot) in scores.iter_mut().enumerate() {
            let xs = self.slot(delay);
            let judged = xs.start + JUDGED.start..xs.start + JUDGED.end;
            let (xr, xi, xp) = (judged_n(&self.x.re[judged.clone()]), judged_n(&self.x.im[judged.clone()]), judged_n(&self.x_power[judged]));
            let cs = delay * JUDGED_N..(delay + 1) * JUDGED_N;
            let (cr, ci, cx) = (judged_n_mut(&mut self.cross.re[cs.clone()]), judged_n_mut(&mut self.cross.im[cs.clone()]), judged_n_mut(&mut self.cross_x[cs]));
            let cross_d = judged_n(&self.cross_d);
            let mut terms = [0f32; JUDGED_N];
            for j in 0..JUDGED_N {
                // conj(x) · d
                cr[j] = c * cr[j] + (1.0 - c) * (xr[j] * dr[j] + xi[j] * di[j]);
                ci[j] = c * ci[j] + (1.0 - c) * (xr[j] * di[j] - xi[j] * dr[j]);
                cx[j] = c * cx[j] + (1.0 - c) * xp[j];
                terms[j] = (cr[j] * cr[j] + ci[j] * ci[j]) / (cx[j] * cross_d[j] + 1e-20);
            }
            let score = lanes_sum(&terms, |t| t);
            *slot = score;
            if score > best.1 {
                best = (delay, score);
            }
        }
        // Already covered nearly as well where the filter is (steady tones
        // are coherent at every delay; so is a filter that's right): stay.
        let covered = scores[self.offset..self.offset + TAPS].iter().cloned().fold(0f32, f32::max);
        if covered >= 0.7 * best.1 {
            self.lead = 0;
            return;
        }
        // Coherent enough to be an echo at all (mean coherence over 0.2).
        if best.1 < 0.2 * JUDGED_N as f32 {
            self.lead = 0;
            return;
        }
        if best.0 == self.leader {
            self.lead += 1;
        } else {
            (self.leader, self.lead) = (best.0, 1);
        }
        let wanted = self.leader.saturating_sub(MARGIN).min(SPAN - TAPS);
        if self.lead >= 50 && wanted != self.offset {
            self.move_filter(wanted);
        }
    }

    fn move_filter(&mut self, to: usize) {
        let mut w = Split::zeros(TAPS * BINS);
        let mut fore = Split::zeros(TAPS * BINS);
        let mut p = vec![P_START; TAPS * BINS];
        for part in 0..TAPS {
            // The partition now at delay `to + part` was the old one at
            // `to + part - offset`.
            let old = (to + part).checked_sub(self.offset).filter(|o| *o < TAPS);
            if let Some(o) = old {
                let (new, old) = (part * BINS..(part + 1) * BINS, o * BINS..(o + 1) * BINS);
                w.re[new.clone()].copy_from_slice(&self.w.re[old.clone()]);
                w.im[new.clone()].copy_from_slice(&self.w.im[old.clone()]);
                fore.re[new.clone()].copy_from_slice(&self.fore.re[old.clone()]);
                fore.im[new.clone()].copy_from_slice(&self.fore.im[old.clone()]);
                p[new].copy_from_slice(&self.p[old]);
            }
        }
        (self.w, self.fore, self.p, self.offset) = (w, fore, p, to);
    }
}
