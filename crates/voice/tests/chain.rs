//! The chain on speech, noise, echo and quiet and loud talkers: what it
//! takes out, what it keeps, what it delays. `cargo test --release -p
//! olo-voice -- --nocapture --test-threads=1` prints the numbers.
//!
//! What the voice keeps is measured frame by frame (10 ms levels), not
//! sample by sample: echo cancellation's high-pass filter shifts the
//! phase of the lowest notes, which a listener doesn't hear but a
//! sample-wise difference counts as damage.

mod speech;

use olo_voice::{FRAME, Model, Noise, Settings, Voice};
use speech::{Lcg, RATE, db, rms, speech};

const VOICES: [&str; 2] = ["slt", "rms"];

/// Run the chain over `mic` (and the speakers, if any) frame by frame;
/// the output lined up with the input (the chain's own delay taken off),
/// and the time a frame took, in µs.
fn run(settings: Settings, mic: &[f32], speakers: Option<&[f32]>) -> (Vec<f32>, f64) {
    let mut v = Voice::new(settings).unwrap();
    let delay = v.latency_samples();
    let mut out = Vec::with_capacity(mic.len());
    let mut frame = [0f32; FRAME];
    let started = std::time::Instant::now();
    for (k, chunk) in mic.as_chunks::<FRAME>().0.iter().enumerate() {
        if let Some(s) = speakers {
            v.render(s[k * FRAME..(k + 1) * FRAME].try_into().unwrap());
        }
        v.capture(chunk, &mut frame);
        out.extend_from_slice(&frame);
    }
    let us = started.elapsed().as_secs_f64() * 1e6 / (mic.len() / FRAME) as f64;
    out.drain(..delay.min(out.len()));
    (out, us)
}

/// How close `out` is to `clean`, sample by sample (signal to distortion,
/// dB).
fn sdr(clean: &[f32], out: &[f32]) -> f32 {
    let n = clean.len().min(out.len());
    let err: Vec<f32> = (0..n).map(|i| out[i] - clean[i]).collect();
    db(rms(&clean[..n])) - db(rms(&err))
}

/// 10 ms levels, dBFS.
fn levels(x: &[f32]) -> Vec<f32> {
    x.as_chunks::<FRAME>().0.iter().map(|f| db(rms(f))).collect()
}

/// Of the frames where `voice` is talking (above -60 dBFS, and louder than
/// the `other` sound the microphone hears with it), the median change in
/// level from `voice` to `out`, and the share of them turned down by more
/// than 6 dB ("chopped").
fn kept(voice: &[f32], other: &[f32], out: &[f32]) -> (f32, f32) {
    let (v, n, o) = (levels(voice), levels(other), levels(out));
    let mut diffs: Vec<f32> = v.iter().zip(&n).zip(&o).filter(|((v, n), _)| **v > -60.0 && **v > **n).map(|((v, _), o)| o - v).collect();
    diffs.sort_by(f32::total_cmp);
    let chopped = diffs.iter().filter(|d| **d < -6.0).count() as f32 / diffs.len() as f32;
    (diffs[diffs.len() / 2], chopped)
}

/// What's left of `out` where `voice` is silent, dBFS.
fn in_pauses(voice: &[f32], out: &[f32]) -> f32 {
    let quiet: Vec<f32> = voice.as_chunks::<FRAME>().0.iter().zip(out.as_chunks::<FRAME>().0.iter()).filter(|(v, _)| rms(*v) < 1e-4).flat_map(|(_, o)| o.iter().copied()).collect();
    db(rms(&quiet))
}

/// Noise as a room has it: broadband hiss, coloured towards the low end,
/// with a hum.
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

/// Noise suppression alone, by `model` (the mode changes only the
/// buffering around the chain, so the tests go through the models).
fn noise_only(model: Model) -> Settings {
    Settings { model, echo_cancellation: false, noise: Noise::Strong, normalize: false, ..Settings::default() }
}

#[test]
fn a_clean_voice_comes_through_untouched() {
    for who in VOICES {
        let clean = speech(who, 0.3);
        for model in Model::ALL {
            let (out, _) = run(noise_only(model), &clean, None);
            let s = sdr(&clean, &out);
            eprintln!("{who} {model:?}, clean: {s:.1} dB SDR");
            assert!(s > 20.0, "{who} {model:?}: a clean voice changed ({s:.1} dB SDR)");
        }
    }
}

#[test]
fn noise_is_taken_out_and_the_voice_kept() {
    for who in VOICES {
        let clean = speech(who, 0.3);
        let noise = room_noise(clean.len(), 0.06, 5);
        let mic: Vec<f32> = clean.iter().zip(&noise).map(|(a, b)| a + b).collect();
        let before = sdr(&clean, &mic);
        for model in Model::ALL {
            let (out, us) = run(noise_only(model), &mic, None);
            let after = sdr(&clean, &out);
            let (level, chopped) = kept(&clean, &noise, &out);
            let (noise_in, noise_out) = (in_pauses(&clean, &mic), in_pauses(&clean, &out));
            eprintln!(
                "{who} {model:?}, noisy: {before:.1} → {after:.1} dB SDR, noise {noise_in:.1} → {noise_out:.1} dBFS, voice {level:+.1} dB ({:.0}% chopped), {us:.0} µs a 10 ms frame",
                chopped * 100.0
            );
            assert!(after - before > 6.0, "{who} {model:?}: only {:.1} dB better", after - before);
            assert!(noise_in - noise_out > 25.0, "{who} {model:?}: noise only {:.1} dB down", noise_in - noise_out);
            assert!(level.abs() < 1.5 && chopped < 0.05, "{who} {model:?}: voice {level:.1} dB, {chopped} chopped");
        }
    }
}

#[test]
fn turning_noise_suppression_down_leaves_more_of_the_room() {
    let clean = speech("slt", 0.3);
    let noise = room_noise(clean.len(), 0.06, 9);
    let mic: Vec<f32> = clean.iter().zip(&noise).map(|(a, b)| a + b).collect();
    for model in Model::ALL {
        let left: Vec<f32> = [Noise::Moderate, Noise::Strong, Noise::Full].into_iter().map(|noise| in_pauses(&clean, &run(Settings { noise, ..noise_only(model) }, &mic, None).0)).collect();
        let input = in_pauses(&clean, &mic);
        eprintln!("{model:?}: noise left in pauses: moderate {:.1}, strong {:.1}, full {:.1} dBFS (in: {input:.1})", left[0], left[1], left[2]);
        // Moderate keeps about 18 dB of it, Strong 30, Full as little as it
        // can. The low-delay model, not looking ahead, lets a little of the
        // noise through at words' ends, and its estimate of a frame's SNR
        // runs about 10 dB under DeepFilterNet3's, so fewer frames of noise
        // alone are gated to silence (2026-11-07, this noise: Strong 23 dB
        // down, Full 25, against 30 and all of it).
        assert!((input - left[0] - 18.0).abs() < 3.0, "{model:?}: {left:?}");
        match model {
            Model::DeepFilterNet3 => {
                assert!((input - left[1] - 30.0).abs() < 3.0, "{model:?}: {left:?}");
                assert!(left[2] < left[1] - 3.0, "{model:?}: {left:?}");
            }
            Model::LowDelay => {
                assert!(input - left[1] > 20.0, "{model:?}: {left:?}");
                assert!(left[2] <= left[1], "{model:?}: {left:?}");
            }
        }
    }
}

/// What the speakers play: chords with a beat and some hiss.
fn music(n: usize) -> Vec<f32> {
    let mut rng = Lcg(3);
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let tau = 2.0 * std::f32::consts::PI;
            let beat = 0.6 + 0.4 * (tau * 2.0 * t).sin().max(0.0);
            0.15 * beat * ((tau * 220.0 * t).sin() + 0.7 * (tau * 277.2 * t).sin() + 0.5 * (tau * 329.6 * t).sin() + 0.3 * (tau * 880.0 * t).sin()) + 0.03 * rng.uniform()
        })
        .collect()
}

/// The speakers as the microphone hears them: 25 ms later (the sound
/// server's buffers and the air), a room's few reflections.
fn echo_of(speakers: &[f32]) -> Vec<f32> {
    let taps = [(1200usize, 0.4f32), (1290, 0.2), (1500, -0.1), (2100, 0.05)];
    (0..speakers.len()).map(|i| taps.iter().filter(|(d, _)| i >= *d).map(|(d, g)| g * speakers[i - d]).sum()).collect()
}

#[test]
fn echo_is_cancelled() {
    let n = 10 * RATE;
    let speakers = music(n);
    let mic = echo_of(&speakers);
    // Once it has found the echo (a couple of seconds).
    let tail = 3 * RATE..n - RATE;
    let alone = Settings { echo_cancellation: true, noise: Noise::Off, normalize: false, ..Settings::default() };
    let (out, us) = run(alone, &mic, Some(&speakers));
    let erle = db(rms(&mic[tail.clone()])) - db(rms(&out[tail.clone()]));
    eprintln!("echo, cancellation alone: {erle:.1} dB less, {us:.0} µs a frame");
    assert!(erle > 20.0, "echo only {erle:.1} dB down");
    let calls = Settings { echo_cancellation: true, normalize: false, ..Settings::default() };
    let (out, us) = run(calls, &mic, Some(&speakers));
    let erle = db(rms(&mic[tail.clone()])) - db(rms(&out[tail]));
    eprintln!("echo, the whole chain: {erle:.1} dB less, {us:.0} µs a frame");
    assert!(erle > 35.0, "echo only {erle:.1} dB down");
}

#[test]
fn talking_over_the_music_keeps_the_voice_and_drops_the_echo() {
    // Both at once ("double talk"): the hard case.
    for who in VOICES {
        let voice = speech(who, 0.3);
        let n = voice.len() + 4 * RATE;
        let speakers = music(n);
        let echo = echo_of(&speakers);
        // The voice starts after 4 s of music alone.
        let mut near = vec![0f32; n];
        near[4 * RATE..].copy_from_slice(&voice);
        let mic: Vec<f32> = near.iter().zip(&echo).map(|(a, b)| a + b).collect();
        for model in Model::ALL {
            let s = Settings { model, echo_cancellation: true, noise: Noise::Strong, normalize: false, ..Settings::default() };
            let (out, us) = run(s, &mic, Some(&speakers));
            let talk = 4 * RATE..out.len();
            let (level, chopped) = kept(&near[talk.clone()], &echo[talk.clone()], &out[talk.clone()]);
            let (echo_in, echo_out) = (in_pauses(&near[talk.clone()], &mic[talk.clone()]), in_pauses(&near[talk.clone()], &out[talk]));
            eprintln!("{who} {model:?}, talking over music: voice {level:+.1} dB ({:.0}% chopped), echo between words {echo_in:.1} → {echo_out:.1} dBFS, {us:.0} µs a frame", chopped * 100.0);
            assert!(level.abs() < 2.0 && chopped < 0.1, "{who} {model:?}: voice {level:.1} dB, {chopped} chopped");
            assert!(echo_in - echo_out > 30.0, "{who} {model:?}: echo only {:.1} dB down", echo_in - echo_out);
        }
    }
}

#[test]
fn a_quiet_voice_is_brought_up_and_a_loud_one_down() {
    let s = Settings { echo_cancellation: false, noise: Noise::Off, normalize: true, ..Settings::default() };
    let mut out_levels = vec![];
    for peak in [0.01, 0.1, 0.9] {
        // Twice through, so the level has settled for the second.
        let mut v = speech("slt", peak);
        v.extend_from_within(..);
        let (out, us) = run(s, &v, None);
        let second = out.len() / 2..out.len();
        let level = db(rms(&out[second.clone()]));
        let top = out[second].iter().fold(0f32, |m, x| m.max(x.abs()));
        eprintln!("normalize: peak {:.0} dBFS in, {level:.1} dBFS rms out, peak {:.1} dBFS, {us:.0} µs a frame", db(peak), db(top));
        assert!(top <= 1.0);
        out_levels.push(level);
    }
    // 39 dB apart in, within a few dB out.
    let spread = out_levels.iter().cloned().fold(f32::MIN, f32::max) - out_levels.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 4.0, "levels {out_levels:?}");
}

/// Where `out` lines up best with `x` over `range` (samples of delay, up
/// to `most`).
fn best_lag(x: &[f32], out: &[f32], range: std::ops::Range<usize>, most: usize) -> usize {
    let c = |l: usize| range.clone().map(|i| x[i] * out[i + l]).sum::<f32>();
    (0..most).max_by(|a, b| c(*a).total_cmp(&c(*b))).unwrap()
}

#[test]
fn the_delay_is_what_it_says() {
    // Noise suppression: where the voice lines up best.
    let clean = speech("slt", 0.3);
    for model in Model::ALL {
        let mut v = Voice::new(noise_only(model)).unwrap();
        let mut out = vec![];
        let mut frame = [0f32; FRAME];
        for c in clean.as_chunks::<FRAME>().0.iter() {
            v.capture(c, &mut frame);
            out.extend_from_slice(&frame);
        }
        let lag = best_lag(&clean, &out, RATE..4 * RATE, 3 * FRAME);
        eprintln!("{model:?}: {} ms", lag as f32 / 48.0);
        assert!(lag.abs_diff(v.latency_samples()) <= 1, "{model:?}: {lag} against {}", v.latency_samples());
    }
    // DeepFilterNet3 looks two frames ahead, the low-delay one none.
    assert_eq!(Voice::new(noise_only(Model::DeepFilterNet3)).unwrap().latency().as_millis(), 30);
    assert_eq!(Voice::new(noise_only(Model::LowDelay)).unwrap().latency().as_millis(), 10);
    // Echo cancellation: where a burst at 3 kHz comes out (a voice's low
    // notes would come later, through the high-pass filter's phase).
    let mut x = vec![0f32; 3 * RATE];
    for k in 0..20 {
        let c = RATE + k * 4800;
        for j in 0..200 {
            let t = j as f32 - 100.0;
            let w = 0.5 + 0.5 * (std::f32::consts::PI * t / 100.0).cos();
            x[c + j] = 0.3 * w * (2.0 * std::f32::consts::PI * 3000.0 * t / RATE as f32).cos();
        }
    }
    let mut v = Voice::new(Settings { echo_cancellation: true, noise: Noise::Off, normalize: false, ..Settings::default() }).unwrap();
    let mut out = vec![];
    let mut frame = [0f32; FRAME];
    for c in x.as_chunks::<FRAME>().0.iter() {
        v.render(&[0.0; FRAME]);
        v.capture(c, &mut frame);
        out.extend_from_slice(&frame);
    }
    let lag = best_lag(&x, &out, RATE..2 * RATE, 2 * FRAME);
    eprintln!("echo cancellation: {} ms", lag as f32 / 48.0);
    assert!(lag.abs_diff(v.latency_samples()) <= 4, "{lag} against {}", v.latency_samples());
}

/// The other person's voice, looped to `n` samples.
fn far_voice(who: &str, n: usize) -> Vec<f32> {
    let mut v = speech(who, 0.5);
    while v.len() < n {
        v.extend_from_within(..);
    }
    v.truncate(n);
    v
}

#[test]
fn the_canceller_learns_a_voices_echo_quickly() {
    // Linear cancellation alone (no residual suppression), the other
    // person talking from the start.
    let n = 10 * RATE;
    let far = far_voice("slt", n);
    let mic = echo_of(&far);
    let mut e = olo_voice::echo::EchoCanceller::new();
    let mut out = vec![];
    let mut frame = [0f32; FRAME];
    let started = std::time::Instant::now();
    for k in 0..n / FRAME {
        e.render(far[k * FRAME..(k + 1) * FRAME].try_into().unwrap());
        e.cancel(mic[k * FRAME..(k + 1) * FRAME].try_into().unwrap(), &mut frame);
        out.extend_from_slice(&frame);
    }
    let us = started.elapsed().as_secs_f64() * 1e6 / (n / FRAME) as f64;
    let erle = |r: std::ops::Range<usize>| db(rms(&mic[r.clone()])) - db(rms(&out[r]));
    let (soon, later) = (erle(2 * RATE..3 * RATE), erle(8 * RATE..10 * RATE));
    eprintln!("a voice's echo, linear alone: {soon:.1} dB less at 2-3 s, {later:.1} dB at 8-10 s; {us:.0} µs a frame");
    assert!(soon > 15.0 && later > 25.0, "{soon:.1}, {later:.1}");
}

#[test]
fn talking_over_the_other_person_keeps_the_voice_and_drops_theirs() {
    // The commonest hard case on a laptop's speakers: both people talking.
    for (near_who, far_who) in [("rms", "slt"), ("slt", "rms")] {
        let voice = speech(near_who, 0.3);
        let n = voice.len() + 4 * RATE;
        let speakers = far_voice(far_who, n);
        let echo = echo_of(&speakers);
        let mut near = vec![0f32; n];
        near[4 * RATE..].copy_from_slice(&voice);
        let mic: Vec<f32> = near.iter().zip(&echo).map(|(a, b)| a + b).collect();
        for model in Model::ALL {
            let s = Settings { model, echo_cancellation: true, noise: Noise::Strong, normalize: false, ..Settings::default() };
            let (out, _) = run(s, &mic, Some(&speakers));
            let talk = 4 * RATE..out.len();
            let (level, chopped) = kept(&near[talk.clone()], &echo[talk.clone()], &out[talk.clone()]);
            let (echo_in, echo_out) = (in_pauses(&near[talk.clone()], &mic[talk.clone()]), in_pauses(&near[talk.clone()], &out[talk]));
            eprintln!("{near_who} over {far_who}, {model:?}: voice {level:+.1} dB ({:.0}% chopped), their voice between words {echo_in:.1} → {echo_out:.1} dBFS", chopped * 100.0);
            assert!(level.abs() < 1.5 && chopped < 0.05, "{near_who} {model:?}: voice {level:.1} dB, {chopped} chopped");
            // What the canceller leaves of their voice, the noise model
            // takes down: DeepFilterNet3 to 57 dB under, the low-delay one
            // (keeping more of a quieter voice, not looking ahead) to 34.
            let least = if model == Model::LowDelay { 30.0 } else { 40.0 };
            assert!(echo_in - echo_out > least, "{near_who} {model:?}: echo only {:.1} dB down", echo_in - echo_out);
        }
    }
}
