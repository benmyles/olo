//! A noise suppressor without look-ahead, for a least-delay choice, set
//! against DeepFilterNet3's (two frames of it: 30 ms in all) on what the
//! chain's tests use: room noise under a voice, and a quieter voice heard
//! first (what DeepFilterNet3's own low-latency model failed: it turned
//! the next voice down and chopped it). Each one's own delay is found
//! (`best_lag`), not taken on trust. `cargo test --release -p olo-voice
//! --test least_delay -- --ignored --nocapture`.

mod speech;

use olo_voice::{FRAME, Mode, Noise, Settings, Voice};
use speech::{Lcg, RATE, db, rms, speech};

fn levels(x: &[f32]) -> Vec<f32> {
    x.as_chunks::<FRAME>().0.iter().map(|f| db(rms(f))).collect()
}

/// Of the frames where `voice` talks above what else the microphone has,
/// the median level change to `out` and the share turned down by > 6 dB.
fn kept(voice: &[f32], other: &[f32], out: &[f32]) -> (f32, f32) {
    let (v, n, o) = (levels(voice), levels(other), levels(out));
    let mut diffs: Vec<f32> = v.iter().zip(&n).zip(&o).filter(|((v, n), _)| **v > -60.0 && **v > **n).map(|((v, _), o)| o - v).collect();
    diffs.sort_by(f32::total_cmp);
    let chopped = diffs.iter().filter(|d| **d < -6.0).count() as f32 / diffs.len() as f32;
    (diffs[diffs.len() / 2], chopped)
}

fn in_pauses(voice: &[f32], out: &[f32]) -> f32 {
    let quiet: Vec<f32> = voice.as_chunks::<FRAME>().0.iter().zip(out.as_chunks::<FRAME>().0.iter()).filter(|(v, _)| rms(*v) < 1e-4).flat_map(|(_, o)| o.iter().copied()).collect();
    db(rms(&quiet))
}

fn room_noise(n: usize, level: f32, seed: u32) -> Vec<f32> {
    let mut rng = Lcg(seed);
    let mut low = 0f32;
    (0..n)
        .map(|i| {
            let w = rng.uniform();
            low += 0.05 * (w - low);
            let hum = (2.0 * std::f32::consts::PI * 50.0 * i as f32 / RATE as f32).sin();
            level * (0.5 * w + 2.0 * low + 0.3 * hum)
        })
        .collect()
}

/// The lag (samples) at which `out` best matches `x`, over five seconds
/// from where `x` starts to sound (not a quieter sound before it).
fn best_lag(x: &[f32], out: &[f32], most: usize) -> usize {
    let start = x.iter().position(|v| v.abs() > 0.01).unwrap_or(0);
    (0..most).max_by(|&a, &b| corr(x, out, start, a).total_cmp(&corr(x, out, start, b))).unwrap_or(0)
}

fn corr(x: &[f32], out: &[f32], start: usize, lag: usize) -> f32 {
    let end = (start + 5 * RATE).min(x.len()).min(out.len().saturating_sub(lag));
    (start..end).map(|i| x[i] * out[i + lag]).sum()
}

/// DeepFilterNet3 as the chain has it (Strong), unaligned output; with
/// `model`, another DeepFilterNet model's tarball (the low-latency one).
fn df3(mic: &[f32]) -> Vec<f32> {
    df_with(mic, None)
}

fn df_with(mic: &[f32], model: Option<&[u8]>) -> Vec<f32> {
    let s = Settings { mode: Mode::Calls, echo_cancellation: false, noise: Noise::Strong, normalize: false, ..Settings::default() };
    let mut v = match model {
        Some(m) => Voice::with_model(s, m).unwrap(),
        None => Voice::new(s).unwrap(),
    };
    let mut out = Vec::with_capacity(mic.len());
    let mut f = [0f32; FRAME];
    for c in mic.as_chunks::<FRAME>().0 {
        v.capture(c, &mut f);
        out.extend_from_slice(&f);
    }
    out
}

/// RNNoise (nnnoiseless), unaligned output, and a frame's time (µs).
fn rnnoise(mic: &[f32]) -> (Vec<f32>, f64) {
    let mut d = nnnoiseless::DenoiseState::new();
    let mut out = Vec::with_capacity(mic.len());
    let (mut i, mut o) = ([0f32; FRAME], [0f32; FRAME]);
    let t = std::time::Instant::now();
    for c in mic.as_chunks::<FRAME>().0 {
        for (a, b) in i.iter_mut().zip(c) {
            *a = b * 32768.0;
        }
        d.process_frame(&mut o, &i);
        out.extend(o.iter().map(|v| v / 32768.0));
    }
    (out, t.elapsed().as_secs_f64() * 1e6 / (mic.len() / FRAME) as f64)
}

fn report(name: &str, clean: &[f32], other: &[f32], mic: &[f32], raw: &[f32]) {
    let lag = best_lag(clean, raw, 4 * FRAME);
    let out = &raw[lag.min(raw.len())..];
    let (level, chopped) = kept(clean, other, out);
    let (n_in, n_out) = (in_pauses(clean, mic), in_pauses(clean, out));
    let n = clean.len().min(out.len());
    let err: Vec<f32> = (0..n).map(|i| out[i] - clean[i]).collect();
    let sdr = db(rms(&clean[..n])) - db(rms(&err));
    eprintln!("  {name}: {:.1} ms behind; SDR {sdr:.1} dB; voice {level:+.1} dB ({:.0}% chopped); between words {n_in:.1} → {n_out:.1} dBFS", lag as f32 * 1000.0 / RATE as f32, chopped * 100.0);
}

#[test]
#[ignore]
fn least_delay_against_deepfilternet() {
    for who in ["slt", "rms"] {
        // Room noise under the voice.
        let clean = speech(who, 0.3);
        let noise = room_noise(clean.len(), 0.06, 5);
        let mic: Vec<f32> = clean.iter().zip(&noise).map(|(a, b)| a + b).collect();
        let ll = std::env::var("OLO_DF3_LL").ok().and_then(|p| std::fs::read(p).ok());
        // A frame's time, each: back to back, and paced as live (a frame
        // every 10 ms, the caches cold between), on whatever core runs it.
        for (name, m) in [("DeepFilterNet3", None), ("DeepFilterNet3-LL", ll.as_deref())] {
            if name.ends_with("LL") && m.is_none() {
                continue;
            }
            let s = Settings { mode: Mode::Calls, echo_cancellation: false, noise: Noise::Strong, normalize: false, ..Settings::default() };
            let mut v = match m {
                Some(m) => Voice::with_model(s, m).unwrap(),
                None => Voice::new(s).unwrap(),
            };
            let mut f = [0f32; FRAME];
            let frames = clean.as_chunks::<FRAME>().0;
            let t = std::time::Instant::now();
            for c in frames {
                v.capture(c, &mut f);
            }
            let back = t.elapsed().as_secs_f64() * 1e6 / frames.len() as f64;
            // On a fast core, as the engine's thread asks (util_min).
            let _ = olo_system::priority::util_min_self(1024);
            let mut paced: Vec<f64> = frames
                .iter()
                .take(500)
                .map(|c| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    let t = std::time::Instant::now();
                    v.capture(c, &mut f);
                    t.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            paced.sort_by(f64::total_cmp);
            eprintln!("{who}: {name} {back:.0} µs a frame back to back; paced: median {:.0}, 99th {:.0}, most {:.0} µs", paced[paced.len() / 2], paced[paced.len() * 99 / 100], paced[paced.len() - 1]);
        }
        eprintln!("{who}, room noise:");
        report("DeepFilterNet3", &clean, &noise, &mic, &df3(&mic));
        if let Some(m) = &ll {
            report("DeepFilterNet3-LL", &clean, &noise, &mic, &df_with(&mic, Some(m)));
        }
        let (r, us) = rnnoise(&mic);
        report(&format!("RNNoise ({us:.0} µs a frame)"), &clean, &noise, &mic, &r);
        // A clean voice: untouched (signal to distortion, aligned).
        let silent = vec![0f32; clean.len()];
        eprintln!("{who}, clean:");
        report("DeepFilterNet3", &clean, &silent, &clean, &df3(&clean));
        if let Some(m) = &ll {
            report("DeepFilterNet3-LL", &clean, &silent, &clean, &df_with(&clean, Some(m)));
        }
        // Another voice heard first, 40 dB quieter, then this one.
        let other_who = if who == "slt" { "rms" } else { "slt" };
        let first = speech(other_who, 0.003);
        let mut voice = vec![0f32; first.len()];
        voice.extend_from_slice(&clean);
        let mut other = first.clone();
        other.resize(voice.len(), 0.0);
        let quiet_noise = room_noise(voice.len(), 0.004, 7);
        let others: Vec<f32> = other.iter().zip(&quiet_noise).map(|(a, b)| a + b).collect();
        let mic: Vec<f32> = voice.iter().zip(&others).map(|(a, b)| a + b).collect();
        eprintln!("{who}, after another voice 40 dB quieter:");
        report("DeepFilterNet3", &voice, &others, &mic, &df3(&mic));
        if let Some(m) = &ll {
            report("DeepFilterNet3-LL", &voice, &others, &mic, &df_with(&mic, Some(m)));
        }
        report("RNNoise", &voice, &others, &mic, &rnnoise(&mic).0);
    }
}
