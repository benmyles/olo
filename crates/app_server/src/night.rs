//! Night light: warm the screens by scaling the CRTC gamma ramps (or,
//! where there are none, the colour matrix) toward the white point of a
//! lower colour temperature. It costs nothing to
//! draw and leaves screenshots untouched (they read the rendered frame,
//! not the scan-out). Changes fade over [`FADE`].

use std::time::{Duration, Instant};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use tracing::info;

use crate::state::{AnvilState, Backend};

/// How long switching night light on or off takes.
pub const FADE: Duration = Duration::from_millis(1500);
const STEP: Duration = Duration::from_millis(50);

/// Neutral: the white point of an sRGB display.
pub const NEUTRAL: u32 = 6500;

/// Red, green and blue gains (0..=1) for a colour temperature in Kelvin,
/// relative to 6500 K (Tanner Helland's fit to the blackbody colours).
pub fn white_point(kelvin: u32) -> [f64; 3] {
    fn raw(kelvin: f64) -> [f64; 3] {
        let t = kelvin / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2) };
        let g = if t <= 66.0 { 99.470_802_586_1 * t.ln() - 161.119_568_166_1 } else { 288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2) };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
        };
        [r, g, b].map(|c| c.clamp(0.0, 255.0) / 255.0)
    }
    let kelvin = kelvin.clamp(1000, NEUTRAL) as f64;
    let (c, n) = (raw(kelvin), raw(NEUTRAL as f64));
    std::array::from_fn(|i| (c[i] / n[i]).clamp(0.0, 1.0))
}

/// Gamma ramps of `len` entries scaled by `gains`.
pub fn ramps(len: usize, gains: [f64; 3]) -> [Vec<u16>; 3] {
    let last = len.saturating_sub(1).max(1) as f64;
    gains.map(|g| (0..len).map(|i| (i as f64 / last * g * 65535.0).round() as u16).collect())
}

/// The same warmth for a display showing HDR, whose signal is PQ: each
/// channel's light scaled as SDR's ramp scales it (a gain `g` on gamma
/// 2.2's signal is `g`^2.2 on its light), then encoded again. (Scaling
/// PQ's codes themselves would dim the blue far more.)
pub fn ramps_pq(len: usize, gains: [f64; 3]) -> [Vec<u16>; 3] {
    let last = len.saturating_sub(1).max(1) as f64;
    gains.map(|g| {
        let light = g.clamp(0.0, 1.0).powf(2.2);
        (0..len).map(|i| (olo_color::pq_encode(olo_color::pq_decode(i as f64 / last) * light) * 65535.0).round() as u16).collect()
    })
}

/// Where a fade is.
#[derive(Debug, Clone, Copy)]
pub struct Fade {
    from: [f64; 3],
    to: [f64; 3],
    start: Instant,
}

impl Fade {
    fn at(&self, now: Instant) -> ([f64; 3], bool) {
        let t = (now.saturating_duration_since(self.start).as_secs_f64() / FADE.as_secs_f64()).min(1.0);
        // Ease in and out.
        let e = t * t * (3.0 - 2.0 * t);
        (std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * e), t >= 1.0)
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Fade the screens to the night light's temperature (`Some`) or back
    /// to neutral.
    pub fn set_night_light(&mut self, kelvin: Option<u32>) {
        let to = white_point(kelvin.unwrap_or(NEUTRAL));
        // At start-up, straight there: the screens may still hold a
        // previous app_server's ramps (a reload keeps the warmth).
        let Some(from) = self.white_point else {
            self.white_point = Some(to);
            self.backend_data.set_white_point(to);
            return;
        };
        if self.night_fade.is_none() && from == to {
            return;
        }
        info!(?kelvin, "night light");
        let running = self.night_fade.is_some();
        self.night_fade = Some(Fade { from, to, start: Instant::now() });
        if running {
            return;
        }
        let _ = self.handle.insert_source(Timer::immediate(), |_, _, state| {
            let Some(fade) = state.night_fade else { return TimeoutAction::Drop };
            let (gains, done) = fade.at(Instant::now());
            state.white_point = Some(gains);
            state.backend_data.set_white_point(gains);
            if done {
                state.night_fade = None;
                TimeoutAction::Drop
            } else {
                TimeoutAction::ToDuration(STEP)
            }
        });
    }
}

/// A DRM colour matrix (`struct drm_color_ctm`: nine S31.32 sign-magnitude
/// numbers, row by row) scaling each channel by its gain.
pub fn ctm(gains: [f64; 3]) -> [u64; 9] {
    let fixed = |g: f64| (g.clamp(0.0, 1.0) * (1u64 << 32) as f64).round() as u64;
    let mut m = [0; 9];
    for (i, g) in gains.into_iter().enumerate() {
        m[i * 4] = fixed(g);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_white_and_warm_is_red() {
        let n = white_point(NEUTRAL);
        assert!(n.iter().all(|c| (c - 1.0).abs() < 1e-9));
        let w = white_point(3600);
        assert_eq!(w[0], 1.0);
        assert!(w[1] < 0.9 && w[1] > 0.6, "{w:?}");
        assert!(w[2] < w[1] && w[2] > 0.3, "{w:?}");
        assert_eq!(white_point(20000), n);
    }

    #[test]
    fn ctm_is_a_diagonal_of_gains() {
        let m = ctm([1.0, 0.5, 0.25]);
        assert_eq!(m, [1 << 32, 0, 0, 0, 1 << 31, 0, 0, 0, 1 << 30]);
    }

    #[test]
    fn ramps_scale_linearly() {
        let [r, g, b] = ramps(256, [1.0, 0.5, 0.0]);
        assert_eq!((r[0], r[255]), (0, 65535));
        assert_eq!(g[255], 32768);
        assert!(b.iter().all(|&v| v == 0));
    }

    #[test]
    fn fades_ease_to_the_end() {
        let start = Instant::now();
        let f = Fade { from: [1.0; 3], to: [1.0, 0.5, 0.0], start };
        assert_eq!(f.at(start).0, [1.0; 3]);
        let (mid, done) = f.at(start + FADE / 2);
        assert!(!done && (mid[1] - 0.75).abs() < 1e-9);
        assert_eq!(f.at(start + FADE * 2), ([1.0, 0.5, 0.0], true));
    }

    #[test]
    fn warmth_in_hdr_is_light_scaled() {
        let [r, _, b] = super::ramps_pq(1024, [1.0, 0.9, 0.6]);
        // Red untouched; blue's light at 0.6^2.2 of what it was.
        assert!(r.iter().enumerate().all(|(i, v)| (f64::from(*v) - i as f64 / 1023.0 * 65535.0).abs() <= 1.0));
        let at = 600;
        let before = olo_color::pq_decode(at as f64 / 1023.0);
        let after = olo_color::pq_decode(f64::from(b[at]) / 65535.0);
        assert!((after / before - 0.6f64.powf(2.2)).abs() < 0.01, "{}", after / before);
    }
}
