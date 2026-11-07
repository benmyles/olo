//! Olo's start-up sound: played once as a session starts (Notifications
//! starts with it; not when re-executed by an upgrade, nor in a test or
//! nested session), unless turned off (Preferences ▸ Sound, "Play a
//! sound when Olo starts"). Made here, not shipped, one of two
//! (`StartupChime`):
//!
//! - Olo, the default (Ben, 2026-11-06: "a startup sound that is like
//!   \"ooollllooooo\" reminiscent of the mac startup sound but saying olo
//!   in the same style"): a start-up chime's big, bright chord (F major,
//!   spread over three octaves, each note two slightly detuned
//!   band-limited saws), voiced: shaped by three formants (resonators at
//!   a vowel's peaks) gliding "ooo", "lll", "ooo" (`olo`), the chord
//!   dipping for the "l" and blooming again, then ringing out.
//! - Swell: a warm swell, a major ninth chord whose notes come in a
//!   moment apart, low to high, each a sine with a little of its octave
//!   and twelfth, a soft attack and a long decay.
//!
//! Either spread across the stereo field with a light room on it, written
//! as a WAV in the cache and played as any sound (`sound::play`).
//! `olo-notifications --startup-sound [olo|swell]` plays one and exits
//! (Preferences ▸ Sound's Play).

use std::path::PathBuf;

/// Samples a second.
const RATE: u32 = 48_000;
/// Its length, seconds.
const LENGTH: f32 = 3.2;

/// The notes (Hz), when each comes in (s), how loud and where (−1 left,
/// 1 right): D♭ major add 9 rising, as a sunrise.
const NOTES: [(f32, f32, f32, f32); 6] = [
    (138.59, 0.00, 0.34, -0.10), // D♭3
    (207.65, 0.07, 0.24, -0.35), // A♭3
    (277.18, 0.14, 0.22, 0.30),  // D♭4
    (349.23, 0.21, 0.18, -0.25), // F4
    (415.30, 0.28, 0.16, 0.40),  // A♭4
    (622.25, 0.38, 0.12, 0.05),  // E♭5 (the ninth, on top)
];

use olo_protocol::StartupChime;

/// Stereo samples of `chime`, interleaved, peaking at −6 dBFS.
pub fn render(chime: StartupChime) -> Vec<i16> {
    let (left, right) = match chime {
        StartupChime::Olo => olo(),
        StartupChime::Swell => swell(),
    };
    finish(left, right)
}

/// How long "olo" is, seconds.
const OLO_LENGTH: f32 = 3.8;

/// The chime's chord (Hz, how loud, where): F major over three octaves,
/// as a start-up chime's (the root doubled low, the third and fifth on
/// top).
const CHORD: [(f32, f32, f32); 6] = [
    (87.31, 0.32, 0.0),    // F2
    (174.61, 0.34, -0.30), // F3
    (261.63, 0.26, 0.32),  // C4
    (349.23, 0.22, -0.22), // F4
    (440.00, 0.17, 0.26),  // A4
    (523.25, 0.11, -0.05), // C5
];

/// A vowel's (or the "l"'s) three formants: (Hz, bandwidth Hz, gain).
type Formants = [(f32, f32, f32); 3];
/// "o", rounded and open; "l", its second formant up (the tongue's tip
/// raised), its first down and weaker; "o" again, darker as it rings out.
const O: Formants = [(450.0, 80.0, 1.0), (820.0, 90.0, 0.55), (2450.0, 140.0, 0.10)];
const L: Formants = [(360.0, 70.0, 0.70), (1180.0, 120.0, 0.42), (2700.0, 170.0, 0.16)];
const O_END: Formants = [(410.0, 90.0, 1.0), (760.0, 100.0, 0.45), (2400.0, 160.0, 0.06)];

/// Where the voice is at `t` seconds: the formants, and how loud the
/// chord is for them (the "l" a dip between the two "o"s).
fn voice_at(t: f32) -> (Formants, f32) {
    let smooth = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    let mix = |a: &Formants, b: &Formants, k: f32| -> Formants {
        let k = smooth(k);
        std::array::from_fn(|i| (a[i].0 + (b[i].0 - a[i].0) * k, a[i].1 + (b[i].1 - a[i].1) * k, a[i].2 + (b[i].2 - a[i].2) * k))
    };
    // "ooo" to 0.62 s; to "lll" by 0.80; "lll" to 1.05; to "ooo" by
    // 1.30; "ooooo" darkening as it rings out.
    let f = if t < 0.62 {
        O
    } else if t < 1.05 {
        mix(&O, &L, (t - 0.62) / 0.18)
    } else if t < 1.30 {
        mix(&L, &O, (t - 1.05) / 0.25)
    } else {
        mix(&O, &O_END, (t - 1.30) / 2.2)
    };
    // The dip, deepest in the "l" (its middle at 0.92 s).
    let dip = 1.0 - 0.42 * (-((t - 0.92) / 0.17).powi(2)).exp();
    (f, dip)
}

/// A band-limited saw's next sample (PolyBLEP: its step smoothed where
/// it wraps), its phase moved on by `dt` (cycles a sample).
fn saw(phase: &mut f32, dt: f32) -> f32 {
    let p = *phase;
    let mut v = 2.0 * p - 1.0;
    if p < dt {
        let x = p / dt;
        v -= x + x - x * x - 1.0;
    } else if p > 1.0 - dt {
        let x = (p - 1.0) / dt;
        v -= x * x + x + x + 1.0;
    }
    *phase += dt;
    if *phase >= 1.0 {
        *phase -= 1.0;
    }
    v
}

/// A band-pass resonator (two poles, its peak at 0 dB: RBJ's), retuned as
/// it goes.
#[derive(Default, Clone, Copy)]
struct Resonator {
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    fn tune(&mut self, f: f32, bw: f32) {
        let w = std::f32::consts::TAU * f / RATE as f32;
        let alpha = w.sin() * (bw / f) / 2.0;
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.a1 = -2.0 * w.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * (x - self.x2) - self.a1 * self.y1 - self.a2 * self.y2;
        (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, y);
        y
    }
}

/// "olo": the chord through the formants, each side its own.
fn olo() -> (Vec<f32>, Vec<f32>) {
    let n = (OLO_LENGTH * RATE as f32) as usize;
    let (mut left, mut right) = (vec![0f32; n], vec![0f32; n]);
    // Each note two saws a few cents apart (a chorus's warmth).
    let mut phases: Vec<[f32; 2]> = CHORD.iter().enumerate().map(|(k, _)| [k as f32 * 0.13 % 1.0, k as f32 * 0.37 % 1.0]).collect();
    let mut bank = [[Resonator::default(); 3]; 2];
    let mut gains = [0f32; 3];
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        // Retuned every 32 samples (a glide's too slow to need more).
        if i % 32 == 0 {
            let (f, _) = voice_at(t);
            for side in &mut bank {
                for (r, (hz, bw, _)) in side.iter_mut().zip(f) {
                    r.tune(hz, bw);
                }
            }
            gains = f.map(|(_, _, g)| g);
        }
        let (_, dip) = voice_at(t);
        // A quick bloom, then a long ring (the low notes longer).
        let attack = (t / 0.09).min(1.0);
        let bloom = attack * attack * (3.0 - 2.0 * attack);
        // A breath of vibrato once it's sounding, and the pitch easing
        // down a little through the "l", as a voice does.
        let vib = 1.0 + 0.0022 * ((t - 0.35).max(0.0) * 6.0).min(1.0) * (std::f32::consts::TAU * 5.3 * t).sin();
        let glide = 1.0 - 0.010 * (-((t - 0.95) / 0.2).powi(2)).exp();
        let (mut l, mut r) = (0f32, 0f32);
        for (k, &(hz, gain, pan)) in CHORD.iter().enumerate() {
            let decay = (-(t - 1.3).max(0.0) * (0.75 + hz / 1400.0)).exp();
            let f = hz * vib * glide;
            let s = 0.5 * (saw(&mut phases[k][0], f * 1.0017 / RATE as f32) + saw(&mut phases[k][1], f * 0.9983 / RATE as f32));
            let x = s * gain * decay;
            l += x * ((1.0 - pan) / 2.0).sqrt();
            r += x * ((1.0 + pan) / 2.0).sqrt();
        }
        let env = bloom * dip;
        let voiced = |side: &mut [Resonator; 3], x: f32| side.iter_mut().zip(gains).map(|(res, g)| res.run(x) * g).sum::<f32>();
        // (A little of the bright chord itself through, for the chime's
        // shimmer.)
        left[i] = (voiced(&mut bank[0], l) + 0.06 * l) * env;
        right[i] = (voiced(&mut bank[1], r) + 0.06 * r) * env;
    }
    (left, right)
}

/// The swell: its notes rising into a ninth chord.
fn swell() -> (Vec<f32>, Vec<f32>) {
    let n = (LENGTH * RATE as f32) as usize;
    let (mut left, mut right) = (vec![0f32; n], vec![0f32; n]);
    for &(f, at, gain, pan) in &NOTES {
        let start = (at * RATE as f32) as usize;
        let (gl, gr) = (((1.0 - pan) / 2.0).sqrt(), ((1.0 + pan) / 2.0).sqrt());
        for i in start..n {
            let t = (i - start) as f32 / RATE as f32;
            // A soft attack, then a long fall (the low notes longer).
            let attack = (t / 0.045).min(1.0);
            let decay = (-t * (0.9 + f / 900.0)).exp();
            let env = attack * attack * (3.0 - 2.0 * attack) * decay;
            let w = std::f32::consts::TAU * f * t;
            // A breath of vibrato, and the octave and twelfth faint.
            let v = 1.0 + 0.0015 * (std::f32::consts::TAU * 5.0 * t).sin();
            let s = (w * v).sin() + 0.18 * (2.0 * w).sin() * (-t * 2.0).exp() + 0.06 * (3.0 * w).sin() * (-t * 3.0).exp();
            let x = s * env * gain;
            left[i] += x * gl;
            right[i] += x * gr;
        }
    }
    (left, right)
}

/// A light room, the end faded, and the whole to −6 dBFS.
fn finish(mut left: Vec<f32>, mut right: Vec<f32>) -> Vec<i16> {
    let n = left.len();
    // A light room: a few combs and allpasses each side (Schroeder's),
    // mixed in under the dry sound.
    let wet_l = room(&left, &[1557, 1617, 1491, 1422], &[225, 556]);
    let wet_r = room(&right, &[1580, 1640, 1514, 1445], &[248, 579]);
    for i in 0..n {
        left[i] = left[i] * 0.82 + wet_l[i] * 0.22;
        right[i] = right[i] * 0.82 + wet_r[i] * 0.22;
    }
    // The last of it faded to nothing, and the whole to −6 dBFS.
    let fade = (0.35 * RATE as f32) as usize;
    for i in n - fade..n {
        let k = (n - i) as f32 / fade as f32;
        left[i] *= k;
        right[i] *= k;
    }
    let peak = left.iter().chain(&right).fold(0f32, |m, x| m.max(x.abs())).max(1e-6);
    let scale = 0.5 / peak;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push((left[i] * scale * i16::MAX as f32) as i16);
        out.push((right[i] * scale * i16::MAX as f32) as i16);
    }
    out
}

/// A Schroeder reverberator's wet signal: parallel combs (delays in
/// samples), then allpasses in series.
fn room(x: &[f32], combs: &[usize], allpasses: &[usize]) -> Vec<f32> {
    let mut wet = vec![0f32; x.len()];
    for &d in combs {
        let mut buf = vec![0f32; d];
        let (mut i, mut lp) = (0, 0f32);
        for (k, s) in x.iter().enumerate() {
            let y = buf[i];
            // (Damped: the room's highs fall faster.)
            lp = y * 0.7 + lp * 0.3;
            buf[i] = s + lp * 0.78;
            wet[k] += y / combs.len() as f32;
            i = (i + 1) % d;
        }
    }
    for &d in allpasses {
        let mut buf = vec![0f32; d];
        let mut i = 0;
        for s in wet.iter_mut() {
            let b = buf[i];
            let y = -*s + b;
            buf[i] = *s + b * 0.5;
            *s = y;
            i = (i + 1) % d;
        }
    }
    wet
}

/// `chime` as a WAV file in Olo's cache folder (made again each start:
/// a few tens of milliseconds).
pub fn file(chime: StartupChime) -> Option<PathBuf> {
    let samples = render(chime);
    // (On disk, in the cache: the runtime folder is memory.)
    let dir = olo_xdg::cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("startup.wav");
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    let data = (samples.len() * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&path, wav).ok()?;
    Some(path)
}

/// Play it, if this is a real session starting and it isn't turned off:
/// once a session (a mark by the session's pid in the runtime folder, so
/// the service re-executed by an upgrade, or started again after a crash,
/// stays quiet).
pub fn at_start() {
    if std::env::var_os("OLO_REAL_SESSION").is_none() {
        return;
    }
    let Some(session) = std::env::var("OLO_SESSION_PID").ok().filter(|p| !p.is_empty()) else { return };
    let mark = olo_xdg::runtime_dir().join(format!("olo-startup-sound-{session}"));
    if mark.exists() {
        return;
    }
    let _ = std::fs::write(&mark, b"");
    let settings = olo_kit_base::settings::current();
    if !settings.as_ref().is_none_or(|s| s.startup_sound) {
        return;
    }
    if let Some(path) = file(settings.map(|s| s.startup_chime).unwrap_or_default()) {
        crate::sound::play(&path);
    }
}

/// `--startup-sound [olo|swell]`: `chime` played now, to its end
/// (Preferences ▸ Sound's Play, an agent's listen).
pub fn play_now(chime: StartupChime) -> i32 {
    let Some(path) = file(chime) else {
        eprintln!("olo-notifications: the start-up sound couldn't be written");
        return 1;
    };
    crate::sound::play_to_end(&path);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_whole_and_never_clipped() {
        for (chime, length) in [(StartupChime::Olo, OLO_LENGTH), (StartupChime::Swell, LENGTH)] {
            let s = render(chime);
            assert_eq!(s.len(), (length * RATE as f32) as usize * 2, "{chime:?}");
            let peak = s.iter().map(|x| (*x as i32).abs()).max().unwrap();
            // −6 dBFS, give or take a sample's rounding.
            assert!((peak - i16::MAX as i32 / 2).abs() < 50, "{chime:?}: {peak}");
            // It starts silent and ends silent (no click).
            assert!(s[..20].iter().all(|x| x.abs() < 200), "{chime:?}");
            assert!(s[s.len() - 20..].iter().all(|x| x.abs() < 50), "{chime:?}");
        }
    }

    /// The "l" is heard: around it the chord's energy moves up from the
    /// "o"'s second formant (700–950 Hz) to the "l"'s (1050–1300 Hz),
    /// and back for the second "o".
    #[test]
    fn olo_says_o_l_o() {
        let s = render(StartupChime::Olo);
        let mono: Vec<f32> = s.chunks_exact(2).map(|c| (c[0] as f32 + c[1] as f32) / 2.0).collect();
        // Energy in a band of a 4096-sample window at `t` (a plain DFT
        // over the band's bins, Hann-windowed).
        let band = |t: f32, lo: f32, hi: f32| -> f32 {
            let n = 4096;
            let at = (t * RATE as f32) as usize - n / 2;
            let w: Vec<f32> = (0..n).map(|i| mono[at + i] * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())).collect();
            let bin = RATE as f32 / n as f32;
            ((lo / bin) as usize..=(hi / bin) as usize)
                .map(|k| {
                    let (mut re, mut im) = (0f32, 0f32);
                    for (i, x) in w.iter().enumerate() {
                        let a = std::f32::consts::TAU * (k * i) as f32 / n as f32;
                        re += x * a.cos();
                        im -= x * a.sin();
                    }
                    re * re + im * im
                })
                .sum()
        };
        let lean = |t: f32| band(t, 1050.0, 1300.0) / band(t, 700.0, 950.0);
        let (o1, l, o2) = (lean(0.40), lean(0.93), lean(1.70));
        assert!(l > 2.0 * o1 && l > 2.0 * o2, "o {o1:.3}, l {l:.3}, o {o2:.3}");
    }

    /// `OLO_STARTUP_WAV=/var/tmp/startup.wav cargo test -p olo-notifications startup_wav`:
    /// the sounds, to listen to ("olo" there, the swell beside it with
    /// "-swell" in its name).
    #[test]
    fn startup_wav() {
        let Some(out) = std::env::var_os("OLO_STARTUP_WAV").map(PathBuf::from) else { return };
        std::fs::copy(file(StartupChime::Olo).unwrap(), &out).unwrap();
        let swell = out.with_file_name(format!("{}-swell.wav", out.file_stem().unwrap().to_string_lossy()));
        std::fs::copy(file(StartupChime::Swell).unwrap(), swell).unwrap();
    }
}
