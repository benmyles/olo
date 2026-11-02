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

use std::time::Duration;

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
            let runtime = df::tract::RuntimeParams::default_with_ch(1)
                .with_atten_lim(settings.noise.limit_db())
                .with_thresholds(-10.0, 35.0, 35.0);
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
                    adaptive_digital: Some(sonora::config::AdaptiveDigital { max_attenuation_db: 20.0, ..Default::default() }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            sonora::AudioProcessing::builder().config(config).capture_config(stream()).render_config(stream()).build()
        });
        let spectral = (echo.is_some() && noise.is_none()).then(|| Box::new(Spectral::new()));
        Ok(Voice { settings, echo, noise, spectral, lookahead, level, scratch: [0.0; FRAME] })
    }

    pub fn settings(&self) -> Settings {
        self.settings
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
        out.copy_from_slice(frame);
        if let Some(e) = &mut self.echo {
            e.cancel(frame, out);
        }
        let gains = self.echo.as_ref().map(|e| &e.gains()[..]);
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
        if let Some(l) = &mut self.level {
            self.scratch.copy_from_slice(out);
            if l.process_capture_f32(&[&self.scratch], &mut [&mut out[..]]).is_err() {
                out.copy_from_slice(&self.scratch);
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
