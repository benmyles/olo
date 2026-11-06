//! Olo's start-up sound: played once as a session starts (Notifications
//! starts with it; not when re-executed by an upgrade, nor in a test or
//! nested session), unless turned off (Preferences ▸ Sound, "Play a
//! sound when Olo starts"). Made here, not shipped: a warm swell, a major
//! ninth chord whose notes come in a moment apart, low to high, each a
//! sine with a little of its octave and twelfth, a soft attack and a long
//! decay, spread across the stereo field, with a light room on it. Written
//! as a WAV in the runtime folder and played as any sound
//! (`sound::play`).

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

/// Stereo samples, interleaved, peaking at −6 dBFS.
pub fn render() -> Vec<i16> {
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

/// The sound as a WAV file in Olo's cache folder (made again each
/// start: a few milliseconds).
pub fn file() -> Option<PathBuf> {
    let samples = render();
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
    let on = olo_kit_base::settings::current().is_none_or(|s| s.startup_sound);
    if !on {
        return;
    }
    if let Some(path) = file() {
        crate::sound::play(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_whole_and_never_clipped() {
        let s = render();
        assert_eq!(s.len(), (LENGTH * RATE as f32) as usize * 2);
        let peak = s.iter().map(|x| (*x as i32).abs()).max().unwrap();
        // −6 dBFS, give or take a sample's rounding.
        assert!((peak - i16::MAX as i32 / 2).abs() < 50, "{peak}");
        // It starts silent and ends silent (no click).
        assert!(s[..20].iter().all(|x| x.abs() < 200));
        assert!(s[s.len() - 20..].iter().all(|x| x.abs() < 50));
    }

    /// `OLO_STARTUP_WAV=/var/tmp/startup.wav cargo test -p olo-notifications startup_wav`:
    /// the sound, to listen to.
    #[test]
    fn startup_wav() {
        let Some(out) = std::env::var_os("OLO_STARTUP_WAV") else { return };
        let made = file().unwrap();
        std::fs::copy(made, out).unwrap();
    }
}
