//! `olo-voice`: what Olo does to every microphone's sound, frame by frame
//! (10 ms at 48 kHz), in real time:
//!
//! 1. **Echo cancellation** ([`echo`]): a fullband frequency-domain
//!    Kalman filter, told what the speakers play, then residual echo
//!    suppression from its own estimate of what it left. No delay of its
//!    own. (WebRTC's AEC3, measured here first, cancels linearly only
//!    below 8 kHz, takes ~10 s to converge on speech, adds 9 ms, and its
//!    suppressor chops the near voice when both talk.)
//! 2. **Noise suppression**: DeepFilterNet3 (vendored libDF, run by tract
//!    with NEON or AVX), with its two frames of look-ahead: 30 ms. (Its
//!    low-latency model, 10 ms, was measured and left out: when another
//!    voice was heard first, even 40 dB quieter, it turns the next voice
//!    down 5 dB and chops a third of it; upstream's build does the same. A
//!    television, a colleague, the far end's echo: calls have that all
//!    the time.) Residual echo suppression's gains are applied in its
//!    spectrum after the network (it never sees them), so the two share one
//!    STFT and its 10 ms.
//! 3. **Level normalization**: WebRTC's AGC2 (Sonora): an adaptive
//!    digital gain steered by its neural voice detector, then a limiter.
//!
//! Each can be turned off ([`Settings`]). [`Voice::latency`] says what the
//! chain adds.

use std::time::{Duration, Instant};

pub mod echo;

use ndarray::{ArrayView2, ArrayViewMut2};

/// Samples a frame (10 ms at 48 kHz).
pub const FRAME: usize = 480;
pub const RATE: u32 = 48_000;

pub use olo_protocol::voice::{Mode, Noise, Settings};

static MODEL: &[u8] = include_bytes!("../../../vendor/deep_filter/models/DeepFilterNet3_onnx.tar.gz");

/// The chain, for one microphone.
pub struct Voice {
    settings: Settings,
    echo: Option<echo::EchoCanceller>,
    noise: Option<df::tract::DfTract>,
    /// The STFT residual echo suppression works in when noise suppression
    /// (which has one) is off.
    spectral: Option<Box<Spectral>>,
    /// The model's look-ahead, in frames.
    lookahead: usize,
    level: Option<sonora::AudioProcessing>,
    scratch: [f32; FRAME],
    /// What each stage does, counted when asked ([`Voice::measure`]).
    measured: Option<Box<Measured>>,
}

/// What one stage of the chain did over a stretch of sound: its time,
/// and the sound's energy going in and coming out.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stage {
    pub frames: u64,
    pub ns: u64,
    pub worst_ns: u64,
    pub energy_in: f64,
    pub energy_out: f64,
}

impl Stage {
    /// How much it turned the sound down (negative) or up, in dB.
    pub fn change_db(&self) -> f64 {
        if self.energy_in <= 1e-12 || self.energy_out <= 1e-12 { 0.0 } else { 10.0 * (self.energy_out / self.energy_in).log10() }
    }

    /// Its time a frame, on average.
    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.ns / self.frames.max(1))
    }

    fn count(&mut self, t: Instant, before: f64, after: &[f32; FRAME]) {
        let ns = t.elapsed().as_nanos() as u64;
        self.frames += 1;
        self.ns += ns;
        self.worst_ns = self.worst_ns.max(ns);
        self.energy_in += before;
        self.energy_out += energy(after);
    }
}

/// Each stage's figures ([`Voice::measure`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Measured {
    pub echo: Stage,
    pub noise: Stage,
    pub level: Stage,
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| (*v as f64) * (*v as f64)).sum()
}

/// What runs the chain's arithmetic on this computer: the processor's
/// vector instructions its kernels (tract's, the FFTs') pick at run time.
pub fn accelerated_by() -> String {
    #[cfg(target_arch = "aarch64")]
    {
        "NEON (aarch64)".into()
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx512f") {
            "AVX-512 (x86-64)".into()
        } else if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
            "AVX2 and FMA (x86-64)".into()
        } else {
            "SSE (x86-64)".into()
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        format!("no vector instructions ({})", std::env::consts::ARCH)
    }
}

/// A recording put through the chain with some settings, at its own pace
/// ([`trial`]).
#[derive(Debug, Clone)]
pub struct Trial {
    pub settings: Settings,
    /// What came out, lined up with what went in (the chain's delay taken
    /// off).
    pub out: Vec<f32>,
    /// What the chain delays the sound by (its look-ahead).
    pub latency: Duration,
    /// Its time a frame: the median and the worst.
    pub frame_median: Duration,
    pub frame_worst: Duration,
    pub measured: Measured,
}

/// `input` (48 kHz mono) through the chain with `settings`, frame by
/// frame as the live service does, timed. Echo cancellation is left off:
/// a recording has no speakers' sound beside it to take out.
pub fn trial(settings: Settings, input: &[f32]) -> Result<Trial, String> {
    let settings = Settings { enabled: true, echo_cancellation: false, ..settings };
    let mut voice = Voice::new(settings)?;
    voice.measure();
    let delay = voice.latency_samples();
    let frames = input.len().div_ceil(FRAME) + delay.div_ceil(FRAME);
    let mut out = Vec::with_capacity(frames * FRAME);
    let mut times = Vec::with_capacity(frames);
    let (mut frame, mut processed) = ([0.0f32; FRAME], [0.0f32; FRAME]);
    for i in 0..frames {
        let from = (i * FRAME).min(input.len());
        let to = ((i + 1) * FRAME).min(input.len());
        frame.fill(0.0);
        frame[..to - from].copy_from_slice(&input[from..to]);
        let t = Instant::now();
        voice.capture(&frame, &mut processed);
        times.push(t.elapsed());
        out.extend_from_slice(&processed);
    }
    let out: Vec<f32> = out.into_iter().skip(delay).take(input.len()).collect();
    times.sort();
    let measured = voice.measured().copied().unwrap_or_default();
    Ok(Trial { settings, out, latency: voice.latency(), frame_median: times.get(times.len() / 2).copied().unwrap_or_default(), frame_worst: times.last().copied().unwrap_or_default(), measured })
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice").field("settings", &self.settings).finish()
    }
}

fn stream() -> sonora::StreamConfig {
    sonora::StreamConfig::new(RATE, 1)
}

impl Voice {
    /// Set up the chain (loading a model takes a moment: off the
    /// real-time thread).
    pub fn new(settings: Settings) -> Result<Voice, String> {
        let on = settings.enabled;
        let echo = (on && settings.echo_cancellation).then(echo::EchoCanceller::new);
        let (noise, lookahead) = if on && settings.noise != Noise::Off {
            let params = df::tract::DfParams::from_bytes(MODEL).map_err(|e| e.to_string())?;
            // Frames of noise alone (under -10 dB local SNR) are taken down
            // to the limit; otherwise both decoders always run (35 is the
            // model's own SNR ceiling). The library's defaults skip the deep
            // filter above 20 dB, leaving the low band to the ERB mask,
            // which isn't trained for it: with look-ahead, that chops the
            // voice wherever it is clean.
            let runtime = df::tract::RuntimeParams::default_with_ch(1).with_atten_lim(settings.noise.limit_db()).with_thresholds(-10.0, 35.0, 35.0);
            let d = df::tract::DfTract::new(params, &runtime).map_err(|e| e.to_string())?;
            let la = d.lookahead;
            (Some(d), la)
        } else {
            (None, 0)
        };
        let level = (on && settings.normalize).then(|| {
            let config = sonora::Config {
                gain_controller2: Some(sonora::config::GainController2 {
                    // Loud talkers brought down as well as quiet ones up.
                    adaptive_digital: Some(sonora::config::AdaptiveDigital {
                        max_attenuation_db: 20.0,
                        // From where it is, not 15 dB up (WebRTC's, for quiet
                        // laptop microphones): the first words of a loud one
                        // came out at -3 dBFS.
                        initial_gain_db: 0.0,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            sonora::AudioProcessing::builder().config(config).capture_config(stream()).render_config(stream()).build()
        });
        let spectral = (echo.is_some() && noise.is_none()).then(|| Box::new(Spectral::new()));
        Ok(Voice { settings, echo, noise, spectral, lookahead, level, scratch: [0.0; FRAME], measured: None })
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// Count what each stage does from now (its time, the energy in and
    /// out): for trying settings out ([`trial`]); the live path doesn't.
    pub fn measure(&mut self) {
        self.measured = Some(Box::default());
    }

    pub fn measured(&self) -> Option<&Measured> {
        self.measured.as_deref()
    }

    /// What the chain delays the sound by, in samples: the STFT's window (a
    /// frame beyond the frame) and the noise model's look-ahead.
    pub fn latency_samples(&self) -> usize {
        if self.noise.is_some() || self.spectral.is_some() { FRAME * (1 + self.lookahead) } else { 0 }
    }

    /// [`Voice::latency_samples`] as a time.
    pub fn latency(&self) -> Duration {
        Duration::from_secs_f64(self.latency_samples() as f64 / RATE as f64)
    }

    /// What the speakers are playing (for echo cancellation), a frame.
    pub fn render(&mut self, frame: &[f32; FRAME]) {
        if let Some(e) = &mut self.echo {
            e.render(frame);
        }
    }

    /// The microphone's frame, processed into `out`.
    pub fn capture(&mut self, frame: &[f32; FRAME], out: &mut [f32; FRAME]) {
        // (Counted only when asked: a branch a stage otherwise.)
        let counting = self.measured.is_some();
        let start = |x: &[f32; FRAME]| counting.then(|| (Instant::now(), energy(x)));
        out.copy_from_slice(frame);
        if let Some(e) = &mut self.echo {
            let t = start(out);
            e.cancel(frame, out);
            if let (Some((t, before)), Some(m)) = (t, &mut self.measured) {
                m.echo.count(t, before, out);
            }
        }
        let gains = self.echo.as_ref().map(|e| &e.gains()[..]);
        let t = start(out);
        let denoised = self.noise.is_some();
        if let Some(d) = &mut self.noise {
            self.scratch.copy_from_slice(out);
            let noisy = ArrayView2::from_shape((1, FRAME), &self.scratch[..]).expect("a frame");
            let enhanced = ArrayViewMut2::from_shape((1, FRAME), &mut out[..]).expect("a frame");
            if d.process_with_gains(noisy, enhanced, gains).is_err() {
                out.copy_from_slice(&self.scratch);
            }
        } else if let (Some(s), Some(g)) = (&mut self.spectral, gains) {
            self.scratch.copy_from_slice(out);
            s.apply(&self.scratch, g, out);
        }
        if denoised && let (Some((t, before)), Some(m)) = (t, &mut self.measured) {
            m.noise.count(t, before, out);
        }
        if let Some(l) = &mut self.level {
            let t = start(out);
            self.scratch.copy_from_slice(out);
            if l.process_capture_f32(&[&self.scratch], &mut [&mut out[..]]).is_err() {
                out.copy_from_slice(&self.scratch);
            }
            if let (Some((t, before)), Some(m)) = (t, &mut self.measured) {
                m.level.count(t, before, out);
            }
        }
    }
}

/// An STFT the same as DeepFilterNet's (960-point Vorbis window, 480 hop),
/// to apply residual echo gains in when noise suppression is off.
struct Spectral {
    state: df::DFState,
    spectrum: Vec<df::Complex32>,
}

impl Spectral {
    fn new() -> Spectral {
        Spectral { state: df::DFState::new(RATE as usize, 2 * FRAME, FRAME, 32, 2), spectrum: vec![df::Complex32::default(); echo::BINS] }
    }

    fn apply(&mut self, input: &[f32; FRAME], gains: &[f32], out: &mut [f32; FRAME]) {
        self.state.analysis(input, &mut self.spectrum);
        for (x, g) in self.spectrum.iter_mut().zip(gains) {
            *x *= *g;
        }
        self.state.synthesis(&mut self.spectrum, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice-like tone that comes and goes, in steady noise.
    fn sample(seconds: f32) -> Vec<f32> {
        let n = (RATE as f32 * seconds) as usize;
        let mut seed = 12345u32;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let t = i as f32 / RATE as f32;
                let voiced = if (t * 2.0).fract() < 0.5 { 0.2 * ((2.0 * std::f32::consts::PI * 220.0 * t).sin() + 0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) } else { 0.0 };
                voiced + 0.05 * noise
            })
            .collect()
    }

    /// A trial says what each stage did, and what comes out lines up with
    /// what went in.
    #[test]
    fn a_trial_measures_each_stage() {
        let input = sample(3.0);
        let off = trial(Settings { noise: Noise::Off, normalize: false, ..Settings::default() }, &input).unwrap();
        assert_eq!(off.out.len(), input.len());
        assert_eq!(off.latency, Duration::ZERO);
        assert!(off.out.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-6), "nothing on: the sound as it was");
        let strong = trial(Settings { noise: Noise::Strong, normalize: false, ..Settings::default() }, &input).unwrap();
        assert_eq!(strong.out.len(), input.len());
        assert_eq!(strong.latency, Duration::from_millis(30));
        assert!(strong.measured.noise.frames > 0 && strong.measured.noise.change_db() < 0.0, "{:?}", strong.measured.noise);
        assert!(strong.frame_worst >= strong.frame_median && strong.frame_median > Duration::ZERO);
        // Lined up: the tone where it was (it sounds a quarter second in
        // every half), against the noise alone after it.
        let e = |x: &[f32], from: f32, to: f32| energy(&x[(from * RATE as f32) as usize..(to * RATE as f32) as usize]);
        assert!(e(&strong.out, 1.02, 1.23) > 4.0 * e(&strong.out, 1.27, 1.48));
        // The noise alone: taken well down.
        assert!(e(&strong.out, 1.27, 1.48) < 0.25 * e(&input, 1.27, 1.48), "{} {}", e(&strong.out, 1.27, 1.48), e(&input, 1.27, 1.48));
        assert!(!accelerated_by().is_empty());
    }

    /// The live path's time a frame, not measuring (`cargo test --release
    /// -p olo-voice live_frame_time -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live_frame_time() {
        let input = sample(20.0);
        let mut v = Voice::new(Settings { enabled: true, echo_cancellation: false, ..Settings::default() }).unwrap();
        let (mut f, mut o) = ([0.0f32; FRAME], [0.0f32; FRAME]);
        let mut times = Vec::new();
        for chunk in input.chunks_exact(FRAME) {
            f.copy_from_slice(chunk);
            let t = Instant::now();
            v.capture(&f, &mut o);
            times.push(t.elapsed());
        }
        times.sort();
        eprintln!("live frame: median {:?}, 99th {:?}", times[times.len() / 2], times[times.len() * 99 / 100]);
    }
}
