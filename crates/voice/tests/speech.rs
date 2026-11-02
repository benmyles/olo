//! Test signals. The speech is two of CMU Flite's voices reading made-up
//! sentences (tests/data/README.md), 16 kHz, brought up to 48 kHz here;
//! the noise and the "music" are made up on the spot.

pub const RATE: usize = 48_000;

/// A deterministic noise source (uniform, -1..1).
pub struct Lcg(pub u32);
impl Lcg {
    pub fn uniform(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// A voice reading, at 48 kHz, its peak at `level`. `which`: "slt" (a
/// woman's voice) or "rms" (a man's).
pub fn speech(which: &str, level: f32) -> Vec<f32> {
    let bytes = match which {
        "slt" => &include_bytes!("data/slt.raw")[..],
        _ => &include_bytes!("data/rms.raw")[..],
    };
    let x: Vec<f32> = bytes.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c) as f32 / 32768.0).collect();
    // 3× up: a Kaiser-windowed sinc a little under 8 kHz, by polyphase.
    const HALF: isize = 32;
    let fc = 7_600.0 / RATE as f64;
    let bessel = |x: f64| {
        let (mut s, mut t) = (1.0, 1.0);
        for k in 1..30 {
            t *= (x / (2.0 * k as f64)).powi(2);
            s += t;
        }
        s
    };
    let taps: Vec<f64> = (-HALF * 3..=HALF * 3)
        .map(|n| {
            let n = n as f64;
            let sinc = if n == 0.0 { 1.0 } else { (std::f64::consts::PI * 2.0 * fc * n).sin() / (std::f64::consts::PI * 2.0 * fc * n) };
            let w = n / (HALF * 3) as f64;
            3.0 * 2.0 * fc * sinc * bessel(9.0 * (1.0 - w * w).max(0.0).sqrt()) / bessel(9.0)
        })
        .collect();
    let mut out = vec![0f32; x.len() * 3];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0f64;
        // Input sample k sits at output 3k; tap index (i - 3k) + 3·HALF.
        let lo = (i as isize - 3 * HALF).div_euclid(3).max(0);
        let hi = ((i as isize + 3 * HALF) / 3).min(x.len() as isize - 1);
        for k in lo..=hi {
            let t = i as isize - 3 * k + 3 * HALF;
            if (0..taps.len() as isize).contains(&t) {
                acc += x[k as usize] as f64 * taps[t as usize];
            }
        }
        *o = acc as f32;
    }
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
    out.iter_mut().for_each(|v| *v *= level / peak);
    out
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

pub fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}
