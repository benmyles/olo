//! Palms told apart from fingers on a touchpad by a small network trained
//! on the person's own hands (Preferences ▸ Touchpad & Mouse ▸ Train for
//! Your Palm). Nothing like it was open to take (docs/TOUCHPAD.md: Chromium's
//! is for touchscreens, ChromeOS's touchpads go by thresholds), so it is
//! made here, all in Rust:
//!
//! - [`Tracker`] follows each contact over the pad's reports and gives its
//!   [`FEATURES`] at each one: its size and shape, pressure, place, speed,
//!   age, the most it has been, and how many others are down. The touch
//!   filter (app_server's `touchpad.lua`) makes the same ones, the same
//!   way, as it filters (`tests` check the two agree).
//! - [`train`] fits a network of one hidden layer (`HIDDEN` units) to
//!   labelled samples, on the CPU in milliseconds, and picks the
//!   threshold: a contact is a palm once its score passes it within its
//!   first [`DECIDE_MS`], and the threshold is set so that hardly a finger
//!   (`FINGER_LOSS`) of those kept back from training is taken for one.
//! - [`score`] runs it ([`PalmModel`], kept in the settings).

use std::collections::HashMap;

pub use olo_protocol::touchpad::{PadAxes, PalmModel, TouchContact};

/// How many features a contact has.
pub const FEATURES: usize = 14;
/// The hidden layer's size.
pub const HIDDEN: usize = 24;
/// A contact is judged in its first this many milliseconds; after, it
/// stays what it was (a finger found to be a palm mid-gesture would stop
/// the gesture).
pub const DECIDE_MS: u32 = 250;
/// The share of fingers kept back from training that may be taken for
/// palms, at most, when choosing the threshold.
pub const FINGER_LOSS: f32 = 0.01;

/// What [`Tracker`] keeps of a contact.
#[derive(Clone, Debug)]
struct Track {
    down_us: u64,
    x: f32,
    y: f32,
    at_us: u64,
    speed: f32,
    peak_size: f32,
    peak_pressure: f32,
    first_size: f32,
}

/// Each contact's history, from which its features are made.
#[derive(Default, Debug)]
pub struct Tracker {
    tracks: HashMap<i32, Track>,
}

impl Tracker {
    /// The features of each contact in a report at `time_us` (the pad's
    /// `axes`), as `(id, features, age in ms)`.
    pub fn frame(&mut self, axes: &PadAxes, time_us: u64, contacts: &[TouchContact]) -> Vec<(i32, [f32; FEATURES], u32)> {
        self.tracks.retain(|id, _| contacts.iter().any(|c| c.id == *id));
        let w = (axes.x_max - axes.x_min).max(1) as f32;
        let h = (axes.y_max - axes.y_min).max(1) as f32;
        let size_max = axes.size_max.max(1) as f32;
        let pressure_max = axes.pressure_max.max(1) as f32;
        let others = (contacts.len().saturating_sub(1)).min(4) as f32 / 4.0;
        let mut out = Vec::with_capacity(contacts.len());
        for c in contacts {
            let x = (c.x - axes.x_min) as f32 / w;
            let y = (c.y - axes.y_min) as f32 / h;
            let size = c.major.max(c.minor) as f32 / size_max;
            let small = c.major.min(c.minor) as f32 / size_max;
            let pressure = c.pressure as f32 / pressure_max;
            let t = self.tracks.entry(c.id).or_insert(Track { down_us: time_us, x, y, at_us: time_us, speed: 0.0, peak_size: 0.0, peak_pressure: 0.0, first_size: size });
            let dt = time_us.saturating_sub(t.at_us) as f32 / 1e6;
            if dt > 0.0 {
                // Pad widths a second, smoothed.
                let moved = ((x - t.x).powi(2) + (y - t.y).powi(2)).sqrt() / dt;
                t.speed = 0.7 * t.speed + 0.3 * moved;
            }
            t.x = x;
            t.y = y;
            t.at_us = time_us;
            t.peak_size = t.peak_size.max(size);
            t.peak_pressure = t.peak_pressure.max(pressure);
            let age_ms = (time_us.saturating_sub(t.down_us) / 1000) as u32;
            let f = [
                size,
                small,
                pressure,
                pressure / (size + 0.01),
                x,
                y,
                x.min(1.0 - x).min(y).min(1.0 - y),
                (age_ms as f32 / 1000.0).min(1.0),
                t.speed.min(5.0) / 5.0,
                t.peak_size,
                t.peak_pressure,
                others,
                // Its shape (round, or long like a palm's heel), and how
                // it has grown since it landed (a palm settles).
                small / (size + 0.01),
                (size / (t.first_size + 0.01)).min(3.0) / 3.0,
            ];
            out.push((c.id, f, age_ms));
        }
        out
    }
}

/// Olo's own model, its weights written by hand from what was measured
/// on a MacBook pad (docs/TOUCHPAD.md): the starting point the person's
/// training refines (`train` starts from it), and what the filter runs
/// until they have trained. Each hidden unit is one sign of a palm:
/// large now; large at its largest; light for its size near an edge (the
/// resting right-edge palm the size rule misses); still with others down
/// (typing); heavy and large; small and moving (a finger: against).
pub fn baseline() -> PalmModel {
    // Typical values and spreads, so units see each feature on one scale.
    let mean = vec![0.12, 0.09, 0.01, 0.08, 0.5, 0.5, 0.22, 0.15, 0.06, 0.14, 0.012, 0.12, 0.75, 0.4];
    let std = vec![0.12, 0.09, 0.02, 0.08, 0.3, 0.3, 0.14, 0.2, 0.1, 0.13, 0.02, 0.22, 0.2, 0.15];
    let mut w1 = vec![0.0f32; HIDDEN * FEATURES];
    let mut b1 = vec![0.0f32; HIDDEN];
    let mut set = |j: usize, b: f32, w: &[(usize, f32)]| {
        b1[j] = b;
        for &(i, v) in w {
            w1[j * FEATURES + i] = v;
        }
    };
    // 0: large now (size ≥ ~0.3 of the axis: 1500 units).
    set(0, -1.2, &[(0, 1.0), (1, 0.4)]);
    // 1: large at its largest.
    set(1, -1.2, &[(9, 1.0)]);
    // 2: light for its size, near an edge, hardly moving.
    set(2, 0.2, &[(3, -0.8), (6, -0.8), (8, -0.5), (0, 0.4)]);
    // 3: still, with others down (palms resting while typing).
    set(3, -0.3, &[(8, -0.7), (11, 0.8), (0, 0.3)]);
    // 4: heavy and large.
    set(4, -0.8, &[(2, 0.7), (10, 0.5), (0, 0.5)]);
    // 5: a finger: small and moving (against).
    set(5, 0.4, &[(0, -0.8), (8, 0.8)]);
    // 6: small for its pressure (a fingertip pressing): against.
    set(6, 0.0, &[(3, 0.9), (0, -0.5)]);
    // 7: long rather than round, and large (a palm's heel).
    set(7, -0.6, &[(12, -0.8), (0, 0.6)]);
    // 8: grown since it landed (a palm settling), and large.
    set(8, -0.5, &[(13, 0.9), (0, 0.3)]);
    // 9: at the bottom corners, large (where palms rest).
    set(9, -0.8, &[(5, 0.6), (6, -0.6), (0, 0.5)]);
    // 10–23: spare, each looking lightly at a pair of features, so
    // training has room to learn what the ones above don't say.
    for j in 10..HIDDEN {
        let (a, b) = ((j * 5) % FEATURES, (j * 7 + 3) % FEATURES);
        set(j, 0.0, &[(a, 0.15), (b, -0.15)]);
    }
    let mut w2 = vec![0.0f32; HIDDEN];
    w2[..10].copy_from_slice(&[1.1, 0.9, 1.0, 0.7, 0.8, -1.0, -0.9, 0.6, 0.5, 0.6]);
    PalmModel { mean, std, w1, b1, w2, b2: -1.0, threshold: 0.0, decide_ms: DECIDE_MS, ..Default::default() }
}

/// The model's score for a contact's features: above its threshold, a
/// palm.
pub fn score(m: &PalmModel, f: &[f32; FEATURES]) -> f32 {
    let mut out = m.b2;
    for (j, &w2) in m.w2.iter().enumerate().take(HIDDEN) {
        let mut a = m.b1.get(j).copied().unwrap_or(0.0);
        for (i, &x) in f.iter().enumerate() {
            let z = (x - m.mean.get(i).copied().unwrap_or(0.0)) / m.std.get(i).copied().unwrap_or(1.0);
            a += m.w1.get(j * FEATURES + i).copied().unwrap_or(0.0) * z;
        }
        out += w2 * a.max(0.0);
    }
    out
}

/// One contact's samples: its features at each report in its first
/// [`DECIDE_MS`], and whether it was a palm.
#[derive(Clone, Debug, Default)]
pub struct Labelled {
    pub frames: Vec<[f32; FEATURES]>,
    pub palm: bool,
    /// Its largest size (the pad's units), for the size rule's verdict.
    pub size: i32,
}

/// How a model did on the contacts kept back from training, beside the
/// size rule (`palm_size`) on the same ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub contacts: usize,
    pub palms: usize,
    pub fingers: usize,
    pub palms_caught: f32,
    pub fingers_lost: f32,
    pub size_rule_palms_caught: f32,
    pub size_rule_fingers_lost: f32,
}

/// A small, seeded random number generator (the tests' made-up contacts).
#[cfg(test)]
struct Rng(u64);

#[cfg(test)]
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.next().max(1e-7), self.next());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

/// Fit a model to `contacts` (every fifth kept back to judge it), saying
/// how far along it is (0 to 1) as it goes; with how it did. None with
/// too few of either kind.
pub fn train(contacts: &[Labelled], palm_size: u32, mut progress: impl FnMut(f32)) -> Option<(PalmModel, Report)> {
    let (mut held, mut used): (Vec<&Labelled>, Vec<&Labelled>) = (Vec::new(), Vec::new());
    for (i, c) in contacts.iter().enumerate() {
        if i % 5 == 4 { held.push(c) } else { used.push(c) }
    }
    let palms = used.iter().filter(|c| c.palm).count();
    let fingers = used.len() - palms;
    if palms < 3 || fingers < 3 {
        return None;
    }
    // Each report a sample, each kind weighed equally.
    let samples: Vec<(&[f32; FEATURES], bool)> = used.iter().flat_map(|c| c.frames.iter().map(move |f| (f, c.palm))).collect();
    if samples.is_empty() {
        return None;
    }
    let n = samples.len() as f32;
    let mut mean = [0.0f32; FEATURES];
    for (f, _) in &samples {
        for i in 0..FEATURES {
            mean[i] += f[i] / n;
        }
    }
    let mut std = [0.0f32; FEATURES];
    for (f, _) in &samples {
        for i in 0..FEATURES {
            std[i] += (f[i] - mean[i]).powi(2) / n;
        }
    }
    let std = std.map(|v| v.sqrt().max(1e-3));
    let z: Vec<([f32; FEATURES], f32)> = samples.iter().map(|(f, palm)| (std::array::from_fn(|i| (f[i] - mean[i]) / std[i]), if *palm { 1.0 } else { 0.0 })).collect();
    let palm_frames = z.iter().filter(|(_, y)| *y > 0.5).count().max(1) as f32;
    let finger_frames = (z.len() as f32 - palm_frames).max(1.0);
    let weight = |y: f32| if y > 0.5 { 0.5 / palm_frames } else { 0.5 / finger_frames };

    // From Olo's own model (its weights in the baseline's scale, taken
    // to this data's), refined by full-batch Adam on the weighted log
    // loss: fine tuning, so a few of the person's samples go a long way.
    let base = baseline();
    let mut w1: Vec<f32> = (0..HIDDEN * FEATURES).map(|k| base.w1[k] * std[k % FEATURES] / base.std[k % FEATURES]).collect();
    let mut b1: Vec<f32> = (0..HIDDEN).map(|j| base.b1[j] + (0..FEATURES).map(|i| base.w1[j * FEATURES + i] * (mean[i] - base.mean[i]) / base.std[i]).sum::<f32>()).collect();
    let mut w2 = base.w2.clone();
    let mut b2 = base.b2;
    let params = HIDDEN * FEATURES + HIDDEN + HIDDEN + 1;
    let (mut m, mut v) = (vec![0.0f32; params], vec![0.0f32; params]);
    let (lr, beta1, beta2, l2) = (0.01f32, 0.9f32, 0.999f32, 1e-4f32);
    const EPOCHS: usize = 400;
    let mut hidden = [0.0f32; HIDDEN];
    for epoch in 1..=EPOCHS {
        let mut g = vec![0.0f32; params];
        for (x, y) in &z {
            let mut out = b2;
            for j in 0..HIDDEN {
                let mut a = b1[j];
                for i in 0..FEATURES {
                    a += w1[j * FEATURES + i] * x[i];
                }
                hidden[j] = a.max(0.0);
                out += w2[j] * hidden[j];
            }
            let p = 1.0 / (1.0 + (-out).exp());
            let d = (p - y) * weight(*y);
            let (gw1, rest) = g.split_at_mut(HIDDEN * FEATURES);
            let (gb1, rest) = rest.split_at_mut(HIDDEN);
            let (gw2, gb2) = rest.split_at_mut(HIDDEN);
            gb2[0] += d;
            for j in 0..HIDDEN {
                gw2[j] += d * hidden[j];
                if hidden[j] > 0.0 {
                    let dh = d * w2[j];
                    gb1[j] += dh;
                    for i in 0..FEATURES {
                        gw1[j * FEATURES + i] += dh * x[i];
                    }
                }
            }
        }
        let t = epoch as i32;
        let mut k = 0;
        let mut step = |p: &mut f32, g: f32, k: &mut usize| {
            let g = g + l2 * *p;
            m[*k] = beta1 * m[*k] + (1.0 - beta1) * g;
            v[*k] = beta2 * v[*k] + (1.0 - beta2) * g * g;
            let mh = m[*k] / (1.0 - beta1.powi(t));
            let vh = v[*k] / (1.0 - beta2.powi(t));
            *p -= lr * mh / (vh.sqrt() + 1e-8);
            *k += 1;
        };
        for (p, gi) in w1.iter_mut().zip(&g[..HIDDEN * FEATURES]) {
            step(p, *gi, &mut k);
        }
        for (p, gi) in b1.iter_mut().zip(&g[HIDDEN * FEATURES..HIDDEN * FEATURES + HIDDEN]) {
            step(p, *gi, &mut k);
        }
        for (p, gi) in w2.iter_mut().zip(&g[HIDDEN * FEATURES + HIDDEN..params - 1]) {
            step(p, *gi, &mut k);
        }
        step(&mut b2, g[params - 1], &mut k);
        if epoch % 20 == 0 {
            progress(epoch as f32 / EPOCHS as f32);
        }
    }
    let mut model = PalmModel { mean: mean.to_vec(), std: std.to_vec(), w1, b1, w2, b2, threshold: 0.0, decide_ms: DECIDE_MS, ..Default::default() };

    // The threshold: a contact's verdict is its highest score in its
    // first `DECIDE_MS`; the lowest that takes at most `FINGER_LOSS` of
    // the fingers kept back (of all, if too few were kept back).
    let judged: Vec<&Labelled> = if held.iter().filter(|c| !c.palm).count() >= 5 { held.clone() } else { contacts.iter().collect() };
    let verdict = |c: &Labelled| c.frames.iter().map(|f| score(&model, f)).fold(f32::MIN, f32::max);
    let mut finger_scores: Vec<f32> = judged.iter().filter(|c| !c.palm).map(|c| verdict(c)).collect();
    finger_scores.sort_by(|a, b| a.total_cmp(b));
    let allowed = (finger_scores.len() as f32 * FINGER_LOSS).floor() as usize;
    let at = finger_scores.len().saturating_sub(allowed + 1);
    model.threshold = finger_scores.get(at).map_or(0.0, |s| s.max(0.0) + 1e-3);

    let report = judge(&model, &held, palm_size);
    model.contacts = contacts.len() as u32;
    model.palms_caught = report.palms_caught;
    model.fingers_lost = report.fingers_lost;
    model.trained = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    progress(1.0);
    Some((model, report))
}

/// Whether the model takes contact `c` for a palm.
pub fn is_palm(m: &PalmModel, c: &Labelled) -> bool {
    c.frames.iter().any(|f| score(m, f) > m.threshold)
}

/// How `m` does on `contacts`, beside the size rule.
pub fn judge(m: &PalmModel, contacts: &[&Labelled], palm_size: u32) -> Report {
    let palms: Vec<&&Labelled> = contacts.iter().filter(|c| c.palm).collect();
    let fingers: Vec<&&Labelled> = contacts.iter().filter(|c| !c.palm).collect();
    let share = |n: usize, of: usize| if of == 0 { 0.0 } else { n as f32 / of as f32 };
    let by_size = |c: &Labelled| c.size >= palm_size as i32;
    Report {
        contacts: contacts.len(),
        palms: palms.len(),
        fingers: fingers.len(),
        palms_caught: share(palms.iter().filter(|c| is_palm(m, c)).count(), palms.len()),
        fingers_lost: share(fingers.iter().filter(|c| is_palm(m, c)).count(), fingers.len()),
        size_rule_palms_caught: share(palms.iter().filter(|c| by_size(c)).count(), palms.len()),
        size_rule_fingers_lost: share(fingers.iter().filter(|c| by_size(c)).count(), fingers.len()),
    }
}

/// The model as the touch filter's configuration has it (a Lua table).
pub fn lua(m: &PalmModel) -> String {
    let list = |v: &[f32]| v.iter().map(|x| format!("{x:e}")).collect::<Vec<_>>().join(", ");
    format!(
        "{{ mean = {{ {} }}, std = {{ {} }}, w1 = {{ {} }}, b1 = {{ {} }}, w2 = {{ {} }}, b2 = {:e}, threshold = {:e}, decide_ms = {}, features = {FEATURES}, hidden = {HIDDEN} }}",
        list(&m.mean),
        list(&m.std),
        list(&m.w1),
        list(&m.b1),
        list(&m.w2),
        m.b2,
        m.threshold,
        m.decide_ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contacts like a MacBook pad's: fingers small, light and moving;
    /// palms large, heavier and still, near an edge; and some that are
    /// hard to call (a light right-edge palm, a finger pressed flat).
    fn made(seed: u64, count: usize) -> Vec<Labelled> {
        let axes = PadAxes::macbook();
        let mut rng = Rng(seed);
        let mut out = Vec::new();
        for k in 0..count {
            let palm = k % 2 == 0;
            let mut tracker = Tracker::default();
            let (mut x, mut y) = if palm { (axes.x_max - 1500 + (rng.next() * 1000.0) as i32, axes.y_min + 800 + (rng.next() * 3000.0) as i32) } else { ((rng.next() * 8000.0) as i32 - 4000, 2000 + (rng.next() * 5000.0) as i32) };
            let (vx, vy) = if palm { (0, 0) } else { ((rng.normal() * 120.0) as i32, (rng.normal() * 120.0) as i32) };
            let hard = rng.next() < 0.15;
            let size = if palm { if hard { 700 + (rng.next() * 400.0) as i32 } else { 1700 + (rng.next() * 2000.0) as i32 } } else if hard { 900 + (rng.next() * 150.0) as i32 } else { 300 + (rng.next() * 500.0) as i32 };
            let pressure = if palm { if hard { 3 + (rng.next() * 10.0) as i32 } else { 50 + (rng.next() * 400.0) as i32 } } else { 10 + (rng.next() * 50.0) as i32 };
            let mut frames = Vec::new();
            for f in 0..30u64 {
                x += vx;
                y += vy;
                let c = TouchContact { id: k as i32, x, y, major: size, minor: (size as f32 * 0.8) as i32, pressure };
                let t = 1_000_000 + f * 8000;
                for (_, feats, age) in tracker.frame(&axes, t, &[c]) {
                    if age <= DECIDE_MS {
                        frames.push(feats);
                    }
                }
            }
            out.push(Labelled { frames, palm, size });
        }
        out
    }

    #[test]
    fn learns_palms_without_losing_fingers() {
        let contacts = made(7, 400);
        let mut steps = 0;
        let (model, report) = train(&contacts, 1600, |_| steps += 1).expect("trained");
        assert!(steps >= 10);
        assert!(report.fingers_lost <= 0.05, "{report:?}");
        // It catches what the size rule misses (the light right-edge palms).
        assert!(report.palms_caught > report.size_rule_palms_caught, "{report:?}");
        assert!(report.palms_caught > 0.9, "{report:?}");
        // On contacts it never saw.
        let fresh = made(99, 200);
        let r = judge(&model, &fresh.iter().collect::<Vec<_>>(), 1600);
        assert!(r.palms_caught > 0.85 && r.fingers_lost < 0.05, "{r:?}");
    }

    #[test]
    fn the_baseline_beats_the_size_rule_untrained() {
        let m = baseline();
        let contacts = made(3, 300);
        let r = judge(&m, &contacts.iter().collect::<Vec<_>>(), 1600);
        assert!(r.palms_caught > r.size_rule_palms_caught && r.palms_caught > 0.85, "{r:?}");
        assert!(r.fingers_lost < 0.1, "{r:?}");
        assert!(lua(&m).contains("hidden = 24") && lua(&m).contains("features = 14"));
    }

    #[test]
    fn too_few_to_train() {
        let contacts = made(1, 4);
        assert!(train(&contacts, 1600, |_| {}).is_none());
    }

    #[test]
    fn features_follow_a_contact() {
        let axes = PadAxes::macbook();
        let mut t = Tracker::default();
        let c = |x: i32, p: i32| TouchContact { id: 3, x, y: 4500, major: 500, minor: 400, pressure: p };
        let a = t.frame(&axes, 0, &[c(0, 20)]);
        let b = t.frame(&axes, 100_000, &[c(1540, 40)]);
        assert_eq!(a[0].2, 0);
        assert_eq!(b[0].2, 100);
        // A tenth of the pad's width in a tenth of a second, smoothed: 0.3.
        assert!((b[0].1[8] - 0.3 / 5.0).abs() < 1e-3, "{:?}", b[0].1);
        assert!((b[0].1[10] - 40.0 / 6000.0).abs() < 1e-6);
        // Gone, then back with the same id: a new contact.
        t.frame(&axes, 200_000, &[]);
        assert_eq!(t.frame(&axes, 300_000, &[c(0, 20)])[0].2, 0);
    }
}
