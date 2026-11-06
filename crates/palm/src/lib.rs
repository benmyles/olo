//! Palms told apart from fingers on a touchpad by a small network trained
//! on the person's own hands (Preferences ▸ Touchpad & Mouse ▸ Train for
//! Your Palm). Nothing like it was open to take (docs/TOUCHPAD.md: Chromium's
//! is for touchscreens, ChromeOS's touchpads go by thresholds), so it is
//! made here, all in Rust:
//!
//! - [`Tracker`] follows each contact over the pad's reports, from when it
//!   first touches the glass, and gives its [`FEATURES`] at each one: its
//!   size and shape, pressure, place, speed, age, the most it has been,
//!   and how many others are down. The touch
//!   filter (app_server's `touchpad.lua`) makes the same ones, the same
//!   way, as it filters (`tests` check the two agree).
//! - [`train`] fits a network of one hidden layer (`HIDDEN` units) to
//!   labelled samples, on the CPU in milliseconds, and picks the
//!   threshold: a contact is a palm once its score passes it within its
//!   first [`DECIDE_MS`], and the threshold is set so that hardly a finger
//!   (`FINGER_LOSS`) of those kept back from training is taken for one.
//! - [`score`] runs it ([`PalmModel`], kept in the settings).
//! - [`default_model`] is what runs until the person trains: [`baseline`]'s
//!   hand-written units, trained on real hands (the trainer's runs kept
//!   on disk, `make_default_model`) and written into `default_model.rs`.
//!   Training starts from it, and keeps the person's only if it does at
//!   least as well.

use std::collections::HashMap;

pub use olo_protocol::touchpad::{PadAxes, PalmModel, TouchContact};

mod kind;
pub use kind::{Capture, Kind, MT_TOOL_PALM, THINKPAD_FEATURES, THINKPAD_HIDDEN, thinkpad_baseline, thinkpad_model};

/// How many features a contact has, at most: a frame's slots (a
/// MacBook's has all of them; each [`Kind`] its own number, the rest 0).
pub const FEATURES: usize = 18;
/// A MacBook model's hidden layer (each model's is its own: `b1`'s
/// length).
pub const HIDDEN: usize = 32;
/// A contact is judged in its first this many milliseconds on the glass;
/// after, it stays what it was (a finger found to be a palm mid-gesture
/// would stop the gesture). On the glass, not since the pad first saw
/// it: a palm settling hovers first (Ben's runs: 15 of 35 palms still
/// hovering 40 ms after they appeared, some for all 250 ms; no finger
/// past 56 ms), and judged from when it appeared, a slow one was never
/// judged touching at all.
pub const DECIDE_MS: u32 = 250;
/// Not before it's this old (five of the pad's reports): in its first
/// report nothing has moved, so a finger landing at the edge looks like
/// a heel; by now its speed shows. A heel hidden 40 ms late moves
/// nothing (the filter counts no motion for it) and can't tap.
pub const MIN_AGE_MS: u32 = 40;
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
    /// What the firmware has said of it (a ThinkPad's).
    said: kind::Said,
}

/// Each contact's history, from which its features are made: from its
/// first report on the glass (some pressure, on a pad that reports it;
/// before that it hovers, and isn't followed).
#[derive(Debug)]
pub struct Tracker {
    tracks: HashMap<i32, Track>,
    /// Palms that just ended, when (microseconds): on a pad whose
    /// firmware says palms, the kernel ends a palm right after reporting
    /// it (its "pending palm" release) and it comes back at the same
    /// place under a new id, sometimes called a finger from then on (a
    /// ThinkPad's heel: Ben's hands, `data/`). It is the same contact.
    ended: Vec<(u64, Track)>,
    /// Microseconds since a key was last pressed, as the caller knows
    /// (u64::MAX: never): palms rest while typing.
    pub since_key_us: u64,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker { tracks: HashMap::new(), ended: Vec::new(), since_key_us: u64::MAX }
    }
}

/// A palm that ends and a contact that lands within this long
/// (microseconds) and this near (millimetres) are one contact (the
/// kernel's palm ended and back: 7 ms apart in Ben's captures).
pub const AGAIN_US: u64 = 40_000;
pub const AGAIN_MM: f32 = 5.0;

impl Tracker {
    /// The features of each contact on the glass in a report at
    /// `time_us` (the pad's `axes`), as `(id, features, age in ms)`; a
    /// contact hovering, not touching yet, has none.
    pub fn frame(&mut self, axes: &PadAxes, time_us: u64, contacts: &[TouchContact]) -> Vec<(i32, [f32; FEATURES], u32)> {
        let kind = Kind::of(axes);
        // Palms ending, kept a moment for their contact to come back.
        let gone: Vec<i32> = self.tracks.keys().copied().filter(|id| !contacts.iter().any(|c| c.id == *id)).collect();
        for id in gone {
            if let Some(t) = self.tracks.remove(&id)
                && kind.firmware_palms()
                && t.said.palm_reports > 0
            {
                self.ended.push((time_us, t));
            }
        }
        self.ended.retain(|(at, _)| time_us.saturating_sub(*at) <= AGAIN_US);
        let touching = |c: &TouchContact| kind.touching(axes, c);
        let w = (axes.x_max - axes.x_min).max(1) as f32;
        let h = (axes.y_max - axes.y_min).max(1) as f32;
        let size_max = axes.size_max.max(1) as f32;
        let pressure_max = axes.pressure_max.max(1) as f32;
        let others = (contacts.len().saturating_sub(1)).min(4) as f32 / 4.0;
        let mut out = Vec::with_capacity(contacts.len());
        for c in contacts {
            if !self.tracks.contains_key(&c.id) && !touching(c) {
                continue;
            }
            let x = (c.x - axes.x_min) as f32 / w;
            let y = (c.y - axes.y_min) as f32 / h;
            // A palm the kernel ended a moment ago, back under a new id.
            if !self.tracks.contains_key(&c.id) && !self.ended.is_empty() {
                let mm = |dx: f32, res: i32, span: f32| dx * span / res.max(1) as f32;
                let near = |t: &Track| mm(x - t.x, axes.x_res, w).hypot(mm(y - t.y, axes.y_res, h)) <= AGAIN_MM;
                if let Some(i) = self.ended.iter().position(|(_, t)| near(t)) {
                    let (_, t) = self.ended.remove(i);
                    self.tracks.insert(c.id, t);
                }
            }
            // The nearest other contact, in pad widths (1: none).
            let nearest = contacts.iter().filter(|o| o.id != c.id).map(|o| (((o.x - c.x) as f32 / w).powi(2) + ((o.y - c.y) as f32 / w).powi(2)).sqrt()).fold(1.0f32, f32::min);
            let orientation = if axes.orientation_max > 0 { (c.orientation as f32 / axes.orientation_max as f32).clamp(-1.0, 1.0) } else { 0.0 };
            let width = (c.width as f32 / c.major.max(1) as f32).min(2.0) / 2.0;
            let size = c.major.max(c.minor) as f32 / size_max;
            let small = c.major.min(c.minor) as f32 / size_max;
            let pressure = c.pressure as f32 / pressure_max;
            let t = self.tracks.entry(c.id).or_insert(Track { down_us: time_us, x, y, at_us: time_us, speed: 0.0, peak_size: 0.0, peak_pressure: 0.0, first_size: size, said: Default::default() });
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
            if kind == Kind::ThinkPad {
                let f = kind::thinkpad_features(&mut t.said, c, t.speed, pressure, age_ms);
                out.push((c.id, f, age_ms));
                continue;
            }
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
                // The nearest other contact (a palm sits far from the
                // typing fingers; scrolling fingers sit close), its
                // angle, and its approaching size against its size.
                nearest,
                orientation,
                width,
                // Seconds since a key was pressed (typing: palms rest).
                (self.since_key_us as f32 / 1e6).min(1.0),
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
    // The size and place ones as the units were written against (a
    // fingertip's size is 0: one spread under a palm's); pressure for
    // its size (3), shape (12), growth (13), approaching size (16) and
    // time since a key (17) as real touches on a MacBook pad have them
    // (two training runs, palms and fingers weighed alike): guessed, they
    // made every real fingertip look light, long and settling.
    let mean = vec![0.12, 0.09, 0.01, 0.028, 0.5, 0.5, 0.22, 0.15, 0.06, 0.14, 0.012, 0.12, 0.5, 0.86, 0.6, 0.0, 0.92, 0.89];
    let std = vec![0.12, 0.09, 0.02, 0.027, 0.3, 0.3, 0.14, 0.2, 0.1, 0.13, 0.02, 0.22, 0.12, 0.25, 0.3, 0.5, 0.14, 0.28];
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
    set(2, -0.2, &[(3, -0.8), (6, -0.8), (8, -0.5), (0, 0.6)]);
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
    set(8, -1.2, &[(13, 0.6), (0, 0.9)]);
    // 9: at the bottom corners, large (where palms rest).
    set(9, -0.8, &[(5, 0.6), (6, -0.6), (0, 0.5)]);
    // 10: very large, whatever else (nothing a finger does).
    set(10, -2.2, &[(0, 1.4), (9, 0.6)]);
    // 11: large and still (resting), old enough to be sure.
    set(11, -0.9, &[(0, 0.8), (8, -0.9), (7, 0.4)]);
    // 12: light and large, just landed (a palm coming down).
    set(12, -0.4, &[(0, 0.7), (3, -0.6), (7, -0.6)]);
    // 13: long, at an edge, still (a heel on the rim).
    set(13, -0.6, &[(12, -0.7), (6, -0.8), (8, -0.5), (0, 0.4)]);
    // 14: many others down and large-ish (typing with both heels).
    set(14, -0.5, &[(11, 1.0), (0, 0.4), (8, -0.3)]);
    // 15: heavy for a finger, at its peak large (a pressed palm).
    set(15, -0.7, &[(10, 1.0), (9, 0.6)]);
    // 16: a finger: small, round and young (a tap landing).
    set(16, 0.3, &[(0, -0.7), (12, 0.6), (7, -0.4)]);
    // 17: a finger: small and in the middle of the pad.
    set(17, 0.2, &[(0, -0.6), (6, 0.8)]);
    // 18: a finger: medium, moving fast (pointing).
    set(18, 0.0, &[(8, 1.0), (0, -0.3)]);
    // 19: a finger: one other down, both small (two-finger scrolling).
    set(19, 0.1, &[(11, 0.5), (0, -0.8), (8, 0.4)]);
    // 20: a finger: heavier than its size says (a fingertip pressing).
    set(20, 0.0, &[(3, 1.0), (0, -0.4), (9, -0.3)]);
    // 21: a finger: small at its largest, however old.
    set(21, 0.3, &[(9, -1.0), (7, 0.3)]);
    // 22: far from the others while others are down, large (a heel
    // resting away from the typing fingers).
    set(22, -1.0, &[(14, 0.9), (11, 1.2), (0, 0.4)]);
    // 23: a finger: close to one other, both small (two fingers).
    set(23, 0.2, &[(14, -0.9), (11, 0.6), (0, -0.5)]);
    // 24: while typing (a key just pressed), large-ish and still.
    set(24, -0.6, &[(17, -1.0), (0, 0.5), (8, -0.4)]);
    // 25: over a side edge (a one-handed rest): at the edge, still,
    // some size, and some at its largest (what's on the glass of a heel
    // half over the rim; growing since it landed says nothing: real
    // fingertips grow as much).
    set(25, -0.5, &[(6, -1.0), (8, -0.6), (0, 0.3), (9, 0.3)]);
    // 26: resting low on the pad, still, long and light, and of some
    // size (a hand's heel below the finger pointing, on a tall pad: not
    // at the edge, so 9, 13 and 25 don't see it, and short of palm
    // size); a thumb waiting to click there is rounder and smaller, and
    // pressing one is heavy for its size.
    set(26, -1.5, &[(5, 0.8), (0, 0.8), (8, -0.5), (3, -0.5), (12, -0.4)]);
    // 27–31: spare, each looking lightly at a pair of features (the
    // angle and approaching size among them), so training has room to
    // learn what the ones above don't say.
    for j in 27..HIDDEN {
        let (a, b) = ((j * 5) % FEATURES, (j * 7 + 3) % FEATURES);
        set(j, 0.0, &[(a, 0.15), (b, -0.15)]);
    }
    let mut w2 = vec![0.0f32; HIDDEN];
    w2[..27].copy_from_slice(&[1.1, 0.9, 1.0, 0.7, 0.8, -1.0, -0.9, 0.6, 0.5, 0.6, 1.2, 0.8, 0.6, 0.7, 0.6, 0.7, -0.8, -0.7, -0.8, -0.7, -0.7, -0.9, 0.7, -0.7, 0.8, 0.6, 0.9]);
    // (A palm past 2: on real hands, with the size rule beside it, as the
    // filter has them, the lowest that loses no finger the size rule
    // doesn't; at 0 it took half of a run's resting fingertips.)
    PalmModel { kind: "macbook".into(), mean, std, w1, b1, w2, b2: -1.0, threshold: 2.0, decide_ms: DECIDE_MS, min_age_ms: MIN_AGE_MS, ..Default::default() }
}

mod default_model;

/// What runs until the person has trained, and where their training
/// starts: [`baseline`] trained on real hands (`default_model.rs`, made by
/// `make_default_model` from the trainer's runs; how it does there, on
/// touches kept back, is in its numbers and docs/TOUCHPAD.md).
pub fn default_model() -> PalmModel {
    use default_model as d;
    if d::MEAN.len() != FEATURES || d::W1.len() != HIDDEN * FEATURES {
        return baseline();
    }
    PalmModel {
        kind: "macbook".into(),
        mean: d::MEAN.to_vec(),
        std: d::STD.to_vec(),
        lo: d::LO.to_vec(),
        hi: d::HI.to_vec(),
        w1: d::W1.to_vec(),
        b1: d::B1.to_vec(),
        w2: d::W2.to_vec(),
        b2: d::B2,
        threshold: d::THRESHOLD,
        decide_ms: DECIDE_MS,
        min_age_ms: MIN_AGE_MS,
        trained: 0,
        contacts: d::CONTACTS,
        palms_caught: d::PALMS_CAUGHT,
        fingers_lost: d::FINGERS_LOST,
    }
}

/// Whether `m` has a model's shape: its features (as many as its kind's)
/// and hidden units each with their weights.
pub fn fits(m: &PalmModel) -> bool {
    let (n, h) = (m.mean.len(), m.b1.len());
    n == Kind::of_model(m).features() && h > 0 && m.std.len() == n && m.w1.len() == h * n && m.w2.len() == h
}

/// The model's score for a contact's features: above its threshold, a
/// palm. (Its size is its own: `mean`'s features, `b1`'s hidden units.)
pub fn score(m: &PalmModel, f: &[f32; FEATURES]) -> f32 {
    let n = m.mean.len().min(FEATURES);
    let held = held(m, f);
    let mut out = m.b2;
    for (j, &w2) in m.w2.iter().enumerate() {
        let mut a = m.b1.get(j).copied().unwrap_or(0.0);
        for (i, &x) in held.iter().enumerate().take(n) {
            let z = (x - m.mean[i]) / m.std.get(i).copied().unwrap_or(1.0);
            a += m.w1.get(j * n + i).copied().unwrap_or(0.0) * z;
        }
        out += w2 * a.max(0.0);
    }
    out
}

/// Features `f` held within what model `m` was trained on (`lo`, `hi`).
fn held(m: &PalmModel, f: &[f32; FEATURES]) -> [f32; FEATURES] {
    let n = m.mean.len().min(FEATURES);
    if m.lo.len() != n || m.hi.len() != n {
        return *f;
    }
    std::array::from_fn(|i| if i < n { f[i].max(m.lo[i]).min(m.hi[i]) } else { f[i] })
}

/// One contact's samples: its features at each report in its first
/// [`DECIDE_MS`], and whether it was a palm.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Labelled {
    pub frames: Vec<[f32; FEATURES]>,
    pub palm: bool,
    /// Its largest size (the pad's units), for the size rule's verdict.
    pub size: i32,
}

/// How a model did on the contacts kept back from training, beside the
/// size rule (`palm_size`) and Olo's own model ([`baseline`]) on the same
/// ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub contacts: usize,
    pub palms: usize,
    pub fingers: usize,
    pub palms_caught: f32,
    pub fingers_lost: f32,
    pub size_rule_palms_caught: f32,
    pub size_rule_fingers_lost: f32,
    pub baseline_palms_caught: f32,
    pub baseline_fingers_lost: f32,
}

/// How much worse a finger taken for a palm is than a palm missed (a
/// finger lost stops what the person was doing).
const FINGER_COST: f32 = 4.0;

impl Report {
    /// Whether the trained model is worth keeping over Olo's own: no worse
    /// on the contacts kept back, a finger lost counting [`FINGER_COST`]
    /// palms missed. (No better is kept: it is fitted to the person's
    /// hands.)
    pub fn better_than_baseline(&self) -> bool {
        let ours = self.palms_caught - FINGER_COST * self.fingers_lost;
        let theirs = self.baseline_palms_caught - FINGER_COST * self.baseline_fingers_lost;
        ours >= theirs - 1e-4
    }
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

/// Fit a model for pad `kind` to `contacts` (every fifth kept back to
/// judge it), saying how far along it is (0 to 1) as it goes; with how it
/// did. None with too few of either kind. From the kind's own model
/// ([`Kind::base_model`]), which it's judged against.
pub fn train(kind: Kind, contacts: &[Labelled], palm_size: u32, progress: impl FnMut(f32)) -> Option<(PalmModel, Report)> {
    train_from(&kind.base_model(), contacts, palm_size, progress)
}

/// How strongly training holds the weights near where they started.
fn l2_from_start() -> f32 {
    #[cfg(test)]
    if let Some(v) = std::env::var("OLO_PALM_L2").ok().and_then(|v| v.parse().ok()) {
        return v;
    }
    // (From the runs, judged across sittings: at 1e-4, toward zero as
    // before, a still fingertip scored 9.6 against a threshold of 3; from
    // 3e-3 it's well under, and the touches kept back do best there.)
    3e-3
}

/// [`train`], from `base` and judged against it.
pub fn train_from(base: &PalmModel, contacts: &[Labelled], palm_size: u32, mut progress: impl FnMut(f32)) -> Option<(PalmModel, Report)> {
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
    // The base's own size: its kind's features, its hidden units.
    let (nf, nh) = (base.mean.len().min(FEATURES), base.b1.len());
    if nf == 0 || nh == 0 || base.w1.len() != nh * nf || base.w2.len() != nh {
        return None;
    }
    let n = samples.len() as f32;
    let mut mean = vec![0.0f32; nf];
    for (f, _) in &samples {
        for i in 0..nf {
            mean[i] += f[i] / n;
        }
    }
    let mut std = vec![0.0f32; nf];
    for (f, _) in &samples {
        for i in 0..nf {
            std[i] += (f[i] - mean[i]).powi(2) / n;
        }
    }
    let std: Vec<f32> = std.iter().map(|v| v.sqrt().max(1e-3)).collect();
    let (mut lo, mut hi) = (vec![f32::MAX; nf], vec![f32::MIN; nf]);
    for (f, _) in &samples {
        for i in 0..nf {
            (lo[i], hi[i]) = (lo[i].min(f[i]), hi[i].max(f[i]));
        }
    }
    let z: Vec<(Vec<f32>, f32)> = samples.iter().map(|(f, palm)| ((0..nf).map(|i| (f[i] - mean[i]) / std[i]).collect(), if *palm { 1.0 } else { 0.0 })).collect();
    let palm_frames = z.iter().filter(|(_, y)| *y > 0.5).count().max(1) as f32;
    let finger_frames = (z.len() as f32 - palm_frames).max(1.0);
    let weight = |y: f32| if y > 0.5 { 0.5 / palm_frames } else { 0.5 / finger_frames };

    // From Olo's own model (its weights in its own scale, taken to this
    // data's), refined by full-batch Adam on the weighted log loss: fine
    // tuning, so a few of the person's samples go a long way.
    let mut w1: Vec<f32> = (0..nh * nf).map(|k| base.w1[k] * std[k % nf] / base.std[k % nf]).collect();
    let mut b1: Vec<f32> = (0..nh).map(|j| base.b1[j] + (0..nf).map(|i| base.w1[j * nf + i] * (mean[i] - base.mean[i]) / base.std[i]).sum::<f32>()).collect();
    let mut w2 = base.w2.clone();
    let mut b2 = base.b2;
    let params = nh * nf + nh + nh + 1;
    let (mut m, mut v) = (vec![0.0f32; params], vec![0.0f32; params]);
    // Held near where it started (`l2` on how far each weight has gone
    // from it, not on the weight): a few hundred touches can't say what
    // a contact unlike any of them is, so there the units keep what they
    // were written to mean; on its own, fine tuning drifted until a
    // still fingertip mid-pad scored as a palm.
    let start: Vec<f32> = w1.iter().chain(&b1).chain(&w2).copied().chain([b2]).collect();
    let (lr, beta1, beta2, l2) = (0.01f32, 0.9f32, 0.999f32, l2_from_start());
    const EPOCHS: usize = 400;
    let mut hidden = vec![0.0f32; nh];
    for epoch in 1..=EPOCHS {
        let mut g = vec![0.0f32; params];
        for (x, y) in &z {
            let mut out = b2;
            for j in 0..nh {
                let mut a = b1[j];
                for i in 0..nf {
                    a += w1[j * nf + i] * x[i];
                }
                hidden[j] = a.max(0.0);
                out += w2[j] * hidden[j];
            }
            let p = 1.0 / (1.0 + (-out).exp());
            let d = (p - y) * weight(*y);
            let (gw1, rest) = g.split_at_mut(nh * nf);
            let (gb1, rest) = rest.split_at_mut(nh);
            let (gw2, gb2) = rest.split_at_mut(nh);
            gb2[0] += d;
            for j in 0..nh {
                gw2[j] += d * hidden[j];
                if hidden[j] > 0.0 {
                    let dh = d * w2[j];
                    gb1[j] += dh;
                    for i in 0..nf {
                        gw1[j * nf + i] += dh * x[i];
                    }
                }
            }
        }
        let t = epoch as i32;
        let mut k = 0;
        let mut step = |p: &mut f32, g: f32, k: &mut usize| {
            let g = g + l2 * (*p - start[*k]);
            m[*k] = beta1 * m[*k] + (1.0 - beta1) * g;
            v[*k] = beta2 * v[*k] + (1.0 - beta2) * g * g;
            let mh = m[*k] / (1.0 - beta1.powi(t));
            let vh = v[*k] / (1.0 - beta2.powi(t));
            *p -= lr * mh / (vh.sqrt() + 1e-8);
            *k += 1;
        };
        for (p, gi) in w1.iter_mut().zip(&g[..nh * nf]) {
            step(p, *gi, &mut k);
        }
        for (p, gi) in b1.iter_mut().zip(&g[nh * nf..nh * nf + nh]) {
            step(p, *gi, &mut k);
        }
        for (p, gi) in w2.iter_mut().zip(&g[nh * nf + nh..params - 1]) {
            step(p, *gi, &mut k);
        }
        step(&mut b2, g[params - 1], &mut k);
        if epoch % 20 == 0 {
            progress(epoch as f32 / EPOCHS as f32);
        }
    }
    // (Judged as its kind is: from its base's age on, to its base's
    // decision.)
    let mut model = PalmModel { kind: base.kind.clone(), mean, std, lo, hi, w1, b1, w2, b2, threshold: 0.0, decide_ms: base.decide_ms, min_age_ms: base.min_age_ms, ..Default::default() };

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

    let mut report = judge(&model, &held, palm_size);
    let base = judge(base, &held, palm_size);
    (report.baseline_palms_caught, report.baseline_fingers_lost) = (base.palms_caught, base.fingers_lost);
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
        ..Default::default()
    }
}

/// A training run as kept on disk ([`keep_samples`]).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Run {
    /// Whether each contact's features follow it from its first touch, as
    /// [`Tracker`] does since 2026-11-03; before, from when the pad first
    /// saw it, hovering included: a run kept as a bare list of contacts.
    pub from_touch: bool,
    /// The kind of pad it was on ([`Kind::id`]; "": a MacBook's), whose
    /// features its contacts have.
    #[serde(default)]
    pub kind: String,
    pub contacts: Vec<Labelled>,
}

impl Run {
    /// A run from its file's bytes, either way it was kept.
    pub fn parse(bytes: &[u8]) -> Option<Run> {
        serde_json::from_slice::<Run>(bytes).ok().or_else(|| Some(Run { from_touch: false, kind: String::new(), contacts: serde_json::from_slice(bytes).ok()? }))
    }

    /// Its contacts as [`Tracker`] follows them now: one kept before the
    /// features followed a contact from its first touch made as near to
    /// that as its features allow (frames before its first with some
    /// pressure dropped, its age counted from that one, its growth and
    /// peaks since; the frames now too young dropped, and contacts that
    /// never touched in the window). From when it touched exactly, when
    /// it touched by the window's first report (40 ms: most do).
    pub fn on_the_glass(&self) -> Vec<Labelled> {
        if self.from_touch {
            return self.contacts.clone();
        }
        self.contacts
            .iter()
            .filter_map(|c| {
                let first = c.frames.iter().position(|f| f[2] > 0.0)?;
                if first == 0 {
                    return Some(c.clone());
                }
                let (born, first_size) = (c.frames[first][7], c.frames[first][0]);
                let (mut peak_size, mut peak_pressure) = (0.0f32, 0.0f32);
                let frames: Vec<[f32; FEATURES]> = c.frames[first..]
                    .iter()
                    .map(|f| {
                        let mut f = *f;
                        f[7] -= born;
                        peak_size = peak_size.max(f[0]);
                        peak_pressure = peak_pressure.max(f[2]);
                        (f[9], f[10]) = (peak_size, peak_pressure);
                        f[13] = (f[0] / (first_size + 0.01)).min(3.0) / 3.0;
                        f
                    })
                    .filter(|f| f[7] * 1000.0 >= MIN_AGE_MS as f32 - 0.5)
                    .collect();
                (!frames.is_empty()).then(|| Labelled { frames, ..c.clone() })
            })
            .collect()
    }
}

/// Keep `contacts` (a training run's samples) in `dir`, as JSON named by
/// the time, for Olo's own model to be tuned on real hands
/// (`make_default_model`, docs/TOUCHPAD.md): the path written, or why
/// not.
pub fn keep_samples(dir: &std::path::Path, kind: Kind, contacts: &[Labelled]) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("run-{secs}.json"));
    let bytes = serde_json::to_vec(&Run { from_touch: true, kind: kind.id().to_owned(), contacts: contacts.to_vec() }).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The model as the touch filter's configuration has it (a Lua table).
pub fn lua(m: &PalmModel) -> String {
    let list = |v: &[f32]| v.iter().map(|x| format!("{x:e}")).collect::<Vec<_>>().join(", ");
    format!(
        "{{ kind = {:?}, mean = {{ {} }}, std = {{ {} }}, lo = {{ {} }}, hi = {{ {} }}, w1 = {{ {} }}, b1 = {{ {} }}, w2 = {{ {} }}, b2 = {:e}, threshold = {:e}, decide_ms = {}, min_age_ms = {}, features = {}, hidden = {} }}",
        Kind::of_model(m).id(),
        list(&m.mean),
        list(&m.std),
        list(if m.lo.len() == m.mean.len() && m.hi.len() == m.mean.len() { &m.lo } else { &[] }),
        list(if m.lo.len() == m.mean.len() && m.hi.len() == m.mean.len() { &m.hi } else { &[] }),
        list(&m.w1),
        list(&m.b1),
        list(&m.w2),
        m.b2,
        m.threshold,
        m.decide_ms,
        m.min_age_ms,
        m.mean.len(),
        m.b1.len()
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
            let (mut x, mut y) = if palm {
                (axes.x_max - 1500 + (rng.next() * 1000.0) as i32, axes.y_min + 800 + (rng.next() * 3000.0) as i32)
            } else {
                ((rng.next() * 8000.0) as i32 - 4000, 2000 + (rng.next() * 5000.0) as i32)
            };
            let (vx, vy) = if palm { (0, 0) } else { ((rng.normal() * 120.0) as i32, (rng.normal() * 120.0) as i32) };
            let hard = rng.next() < 0.15;
            let size = if palm {
                if hard { 700 + (rng.next() * 400.0) as i32 } else { 1700 + (rng.next() * 2000.0) as i32 }
            } else if hard {
                900 + (rng.next() * 150.0) as i32
            } else {
                300 + (rng.next() * 500.0) as i32
            };
            let pressure = if palm { if hard { 3 + (rng.next() * 10.0) as i32 } else { 50 + (rng.next() * 400.0) as i32 } } else { 10 + (rng.next() * 50.0) as i32 };
            let mut frames = Vec::new();
            for f in 0..30u64 {
                x += vx;
                y += vy;
                let c = TouchContact { id: k as i32, x, y, major: size, minor: (size as f32 * 0.8) as i32, pressure, ..Default::default() };
                let t = 1_000_000 + f * 8000;
                for (_, feats, age) in tracker.frame(&axes, t, &[c]) {
                    if (MIN_AGE_MS..=DECIDE_MS).contains(&age) {
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
        let (model, report) = train(Kind::MacBook, &contacts, 1600, |_| steps += 1).expect("trained");
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
        eprintln!("the baseline, untrained: {r:?}");
        // (Made-up contacts, as the units were first written against: at
        // least as good as the size rule here. Real hands are the measure:
        // `captures::on_real_hands`, docs/TOUCHPAD.md.)
        assert!(r.palms_caught >= r.size_rule_palms_caught && r.palms_caught > 0.85, "{r:?}");
        assert!(r.fingers_lost < 0.1, "{r:?}");
        assert!(lua(&m).contains("hidden = 32") && lua(&m).contains("features = 18"));
    }

    /// One hand's heel half over the pad's left or right rim (what the
    /// glass sees: a smallish, long, light, still contact at the very
    /// edge, growing as it settles, alone), against fingers scrolling
    /// along the right edge (moving, round, small).
    fn made_edge(seed: u64, count: usize) -> Vec<Labelled> {
        let axes = PadAxes::macbook();
        let mut rng = Rng(seed);
        let mut out = Vec::new();
        for k in 0..count {
            let palm = k % 2 == 0;
            let mut tracker = Tracker::default();
            let left = rng.next() < 0.5;
            let (mut x, mut y) = if palm {
                (if left { axes.x_min + (rng.next() * 300.0) as i32 } else { axes.x_max - (rng.next() * 300.0) as i32 }, 3000 + (rng.next() * 5000.0) as i32)
            } else {
                (axes.x_max - 200 - (rng.next() * 500.0) as i32, 1500 + (rng.next() * 3000.0) as i32)
            };
            let vy = if palm { 0 } else { 120 + (rng.next() * 120.0) as i32 };
            let full = if palm { 700 + (rng.next() * 800.0) as i32 } else { 300 + (rng.next() * 300.0) as i32 };
            let pressure = if palm { 4 + (rng.next() * 56.0) as i32 } else { 10 + (rng.next() * 40.0) as i32 };
            let mut frames = Vec::new();
            for f in 0..30u64 {
                y += vy;
                // A settling heel grows over its first frames.
                let size = if palm { (full as f32 * (0.7 + 0.3 * (f as f32 / 8.0).min(1.0))) as i32 } else { full };
                let minor = (size as f32 * if palm { 0.5 } else { 0.85 }) as i32;
                let c = TouchContact { id: k as i32, x, y, major: size, minor, pressure, ..Default::default() };
                let _ = &mut x;
                for (_, feats, age) in tracker.frame(&axes, 1_000_000 + f * 8000, &[c]) {
                    if (MIN_AGE_MS..=DECIDE_MS).contains(&age) {
                        frames.push(feats);
                    }
                }
            }
            out.push(Labelled { frames, palm, size: full });
        }
        out
    }

    #[test]
    fn the_baseline_catches_a_palm_over_the_edge() {
        let m = baseline();
        let contacts = made_edge(11, 300);
        let r = judge(&m, &contacts.iter().collect::<Vec<_>>(), 1600);
        eprintln!("the baseline at the edge: {r:?}");
        // The size rule misses every one of these (too small); the
        // baseline catches most without taking the edge fingers.
        assert_eq!(r.size_rule_palms_caught, 0.0, "{r:?}");
        assert!(r.palms_caught > 0.85, "{r:?}");
        assert!(r.fingers_lost < 0.1, "{r:?}");
    }

    /// A hand's heel resting low on the pad, as it falls while a finger
    /// of that hand points above it (on a 16" MacBook's tall pad), against
    /// a thumb clicking at the bottom. Made up from what the pad reports
    /// of palms and fingers elsewhere (docs/TOUCHPAD.md) until real ones
    /// are captured (the trainer's "Rest your hand low"): the heel half
    /// over the bottom edge (long along it, light, still) or all on the
    /// glass but short of palm size; settling, it hovers first, up to
    /// 200 ms. The thumb: rounder, at most palm-sized by half, resting
    /// there or pressing past a click (one resting, called a palm, would
    /// lose its click: a palm can't click). Half of each with a finger
    /// moving above.
    fn made_low(seed: u64, count: usize) -> Vec<Labelled> {
        let axes = PadAxes::macbook();
        let h = axes.y_max - axes.y_min;
        let mut rng = Rng(seed);
        let mut out = Vec::new();
        for k in 0..count {
            let palm = k % 2 == 0;
            let mut tracker = Tracker::default();
            let over = rng.next() < 0.5;
            let x = axes.x_min + ((0.2 + rng.next() * 0.65) * (axes.x_max - axes.x_min) as f32) as i32;
            let y = if !palm {
                axes.y_max - ((0.05 + rng.next() * 0.13) * h as f32) as i32
            } else if over {
                axes.y_max - (rng.next() * 0.05 * h as f32) as i32
            } else {
                axes.y_max - ((0.08 + rng.next() * 0.15) * h as f32) as i32
            };
            let full = if palm { 700 + (rng.next() * 800.0) as i32 } else { 500 + (rng.next() * 500.0) as i32 };
            let shape = if palm { 0.35 + rng.next() * 0.2 } else { 0.55 + rng.next() * 0.2 };
            let rest = if palm { 2 + (rng.next() * 38.0) as i32 } else { 20 + (rng.next() * 20.0) as i32 };
            let click = if rng.next() < 0.5 { 150 + (rng.next() * 250.0) as i32 } else { rest };
            let hover = if palm { (rng.next() * 25.0) as u64 } else { (rng.next() * 3.0) as u64 };
            let with_finger = rng.next() < 0.5;
            let mut frames = Vec::new();
            for f in 0..(hover + 40) {
                let touching = f >= hover;
                let on = f.saturating_sub(hover);
                // A settling heel grows as it lands; a thumb presses on
                // to a click over its first 150 ms.
                let size = if palm { (full as f32 * (0.6 + 0.4 * (on as f32 / 10.0).min(1.0))) as i32 } else { full };
                let pressure = match (touching, palm) {
                    (false, _) => 0,
                    (true, true) => rest,
                    (true, false) => rest + ((click - rest) as f32 * (on as f32 / 18.0).min(1.0)) as i32,
                };
                let mut contacts = vec![TouchContact { id: 1, x, y, major: size, minor: (size as f32 * shape) as i32, pressure, ..Default::default() }];
                if with_finger {
                    let fy = y - 3000 - (f as i32 * 40);
                    contacts.push(TouchContact { id: 2, x: x + 600 + f as i32 * 60, y: fy, major: 550, minor: 450, pressure: 30, ..Default::default() });
                }
                for (id, feats, age) in tracker.frame(&axes, 1_000_000 + f * 8000, &contacts) {
                    if id == 1 && (MIN_AGE_MS..=DECIDE_MS).contains(&age) {
                        frames.push(feats);
                    }
                }
            }
            out.push(Labelled { frames, palm, size: full });
        }
        out
    }

    #[test]
    fn the_baseline_catches_a_hand_resting_low() {
        let m = baseline();
        let contacts = made_low(21, 400);
        let r = judge(&m, &contacts.iter().collect::<Vec<_>>(), 1600);
        eprintln!("the baseline, a hand resting low: {r:?}");
        assert_eq!(r.size_rule_palms_caught, 0.0, "{r:?}");
        // (Untrained, a start: most, and no thumb.)
        assert!(r.palms_caught > 0.8, "{r:?}");
        assert!(r.fingers_lost < 0.02, "{r:?}");
        let d = judge(&default_model(), &contacts.iter().collect::<Vec<_>>(), 1600);
        eprintln!("Olo's own, a hand resting low: {d:?}");
        // And trained on them (with the other kinds), on ones it never saw.
        let mut all = made_low(5, 200);
        all.extend(made(6, 200));
        let (trained, _) = train(Kind::MacBook, &all, 1600, |_| {}).expect("trained");
        let fresh = made_low(77, 200);
        let t = judge(&trained, &fresh.iter().collect::<Vec<_>>(), 1600);
        eprintln!("trained, a hand resting low: {t:?}");
        assert!(t.palms_caught > 0.9 && t.fingers_lost < 0.05, "{t:?}");
    }

    #[test]
    fn too_few_to_train() {
        let contacts = made(1, 4);
        assert!(train(Kind::MacBook, &contacts, 1600, |_| {}).is_none());
    }

    #[test]
    fn features_follow_a_contact() {
        let axes = PadAxes::macbook();
        let mut t = Tracker::default();
        let c = |x: i32, p: i32| TouchContact { id: 3, x, y: 4500, major: 500, minor: 400, pressure: p, ..Default::default() };
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
        // Hovering (no pressure) it isn't followed; its age counts from
        // when it touches, and it's followed after, light or not.
        t.frame(&axes, 400_000, &[]);
        assert!(t.frame(&axes, 410_000, &[c(0, 0)]).is_empty());
        assert!(t.frame(&axes, 500_000, &[c(0, 0)]).is_empty());
        assert_eq!(t.frame(&axes, 520_000, &[c(0, 5)])[0].2, 0);
        assert_eq!(t.frame(&axes, 580_000, &[c(0, 0)])[0].2, 60);
        // A pad that reports no pressure: touching from the first report.
        let bare = PadAxes { pressure_max: 0, ..PadAxes::macbook() };
        assert_eq!(Tracker::default().frame(&bare, 0, &[c(0, 0)]).len(), 1);
    }
}

/// How the models do on real hands: every run the palm trainer kept
/// (`keep_samples`), in `OLO_PALM_CAPTURES` (default
/// `~/.local/state/olo/palm`): `cargo test --release -p olo-palm
/// on_real_hands -- --ignored --nocapture`. Each run judged by the size
/// rule, Olo's own model (alone, and with the size rule as the filter
/// has them), and a model trained on each other run (another sitting's
/// hands: what training buys). `OLO_PALM_WHY=1` says which features and
/// units take the fingers it loses; `OLO_PALM_SWEEP=1` tries thresholds.
#[cfg(test)]
mod captures {
    use super::*;

    /// Every run kept, as the tracker follows contacts now
    /// ([`Run::on_the_glass`]), by name, oldest first: from
    /// `OLO_PALM_CAPTURES`, or where the trainer keeps them (and where it
    /// kept them until 2026-11-03, `olo/olo/palm`).
    pub(super) fn runs() -> Vec<(String, Vec<Labelled>)> {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dirs = match std::env::var_os("OLO_PALM_CAPTURES") {
            Some(d) => vec![std::path::PathBuf::from(d)],
            None => vec![home.join(".local/state/olo/palm"), home.join(".local/state/olo/olo/palm")],
        };
        let mut out: Vec<(String, Vec<Labelled>)> = dirs
            .iter()
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), Run::parse(&std::fs::read(e.path()).ok()?)?.on_the_glass())))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// What the runs may not show, made from their own touches: each
    /// finger pressing on to a click (every click is a finger pressing
    /// past the driver's 120: its pressure ramps there over 150 ms from
    /// where it was, its size grows a quarter as it flattens), and each
    /// contact again low on the pad (a fingertip is one anywhere, a thumb
    /// waits at the bottom to click; a palm rests there). On a MacBook's
    /// pad (pressure to 6000), as the runs were.
    pub(super) fn pressed(c: &Labelled, seed: u64) -> Labelled {
        let mut rng = Rng(seed | 1);
        let to = (150.0 + rng.next() * 250.0) / 6000.0;
        let (mut peak_size, mut peak_pressure) = (0.0f32, 0.0f32);
        let frames = c
            .frames
            .iter()
            .map(|f| {
                let mut f = *f;
                let r = (f[7] * 1000.0 / 150.0).min(1.0);
                let grow = 1.0 + 0.25 * r;
                f[2] += (to - f[2]) * r;
                (f[0], f[1]) = (f[0] * grow, f[1] * grow);
                f[3] = f[2] / (f[0] + 0.01);
                peak_size = peak_size.max(f[0]);
                peak_pressure = peak_pressure.max(f[2]);
                (f[9], f[10]) = (f[9].max(peak_size), peak_pressure);
                f[13] = (f[13] * grow).min(1.0);
                f
            })
            .collect();
        Labelled { frames, size: (c.size as f32 * 1.25) as i32, ..c.clone() }
    }

    pub(super) fn low(c: &Labelled, seed: u64) -> Labelled {
        let mut rng = Rng(seed | 1);
        let y = 0.8 + rng.next() * 0.15;
        let frames = c
            .frames
            .iter()
            .map(|f| {
                let mut f = *f;
                f[5] = y;
                f[6] = f[4].min(1.0 - f[4]).min(y).min(1.0 - y);
                f
            })
            .collect();
        Labelled { frames, ..c.clone() }
    }

    /// `contacts` and what they don't show made from them: each finger
    /// pressed on to a click, each contact low on the pad, each finger
    /// clicking there.
    pub(super) fn with_what_they_dont_show(contacts: &[Labelled]) -> Vec<Labelled> {
        let mut out = contacts.to_vec();
        for (i, c) in contacts.iter().enumerate() {
            let k = i as u64 * 4;
            out.push(low(c, k));
            if !c.palm {
                let click = pressed(c, k + 1);
                out.push(low(&click, k + 2));
                out.push(click);
            }
        }
        out
    }

    /// How models do on the runs' touches made into what they don't show
    /// (`pressed`, `low`): `cargo test --release -p olo-palm
    /// what_the_runs_dont_show -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn what_the_runs_dont_show() {
        let runs = runs();
        let all: Vec<Labelled> = runs.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        let fingers: Vec<&Labelled> = all.iter().filter(|c| !c.palm).collect();
        let palms: Vec<&Labelled> = all.iter().filter(|c| c.palm).collect();
        let clicks: Vec<Labelled> = fingers.iter().enumerate().map(|(i, c)| pressed(c, i as u64)).collect();
        let low_fingers: Vec<Labelled> = fingers.iter().enumerate().map(|(i, c)| low(c, i as u64)).collect();
        let low_clicks: Vec<Labelled> = clicks.iter().enumerate().map(|(i, c)| low(c, i as u64 + 7)).collect();
        let low_palms: Vec<Labelled> = palms.iter().enumerate().map(|(i, c)| low(c, i as u64)).collect();
        let share = |m: &PalmModel, v: &[Labelled]| v.iter().filter(|c| is_palm(m, c)).count() as f32 / v.len().max(1) as f32 * 100.0;
        for (name, m) in [("hand-written", baseline()), ("Olo's own", default_model())] {
            println!(
                "{name:<14} fingers clicking taken {:5.1}%, fingers low {:5.1}%, thumbs clicking low {:5.1}%; palms low caught {:5.1}%",
                share(&m, &clicks),
                share(&m, &low_fingers),
                share(&m, &low_clicks),
                share(&m, &low_palms)
            );
        }
    }

    /// Olo's own model made anew from every run kept: [`baseline`] trained
    /// on them all and what they don't show made from them
    /// ([`with_what_they_dont_show`]; every fifth contact kept back to
    /// set its threshold), written into `default_model.rs` with how it
    /// did: `cargo test --release -p olo-palm make_default_model --
    /// --ignored --nocapture`. Each run is judged too by a model made the
    /// same way from the others only (another sitting's hands): its own
    /// touches, and those made from them.
    #[test]
    #[ignore]
    fn make_default_model() {
        let runs = runs();
        assert!(!runs.is_empty(), "no captures: OLO_PALM_CAPTURES=dir");
        let real: Vec<Labelled> = runs.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        let all = with_what_they_dont_show(&real);
        let (m, r) = train_from(&baseline(), &all, 1600, |_| {}).expect("enough to train on");
        println!("{} runs, {} contacts; kept back: {r:?}", runs.len(), all.len());
        println!("  {}", line("trained on them all", r.palms_caught, r.fingers_lost));
        println!("  {}", line("the hand-written baseline", r.baseline_palms_caught, r.baseline_fingers_lost));
        println!("  {}", line("size rule (1600)", r.size_rule_palms_caught, r.size_rule_fingers_lost));
        let mut across = Vec::new();
        for (name, run) in &runs {
            let others: Vec<Labelled> = runs.iter().filter(|(n, _)| n != name).flat_map(|(_, r)| r.iter().cloned()).collect();
            if let Some((o, _)) = train_from(&baseline(), &with_what_they_dont_show(&others), 1600, |_| {}) {
                let t = judge(&o, &run.iter().collect::<Vec<_>>(), 1600);
                let made = with_what_they_dont_show(run);
                let u = judge(&o, &made[run.len()..].iter().collect::<Vec<_>>(), 1600);
                println!("  {name}, by a model made from the others: {}", line("", t.palms_caught, t.fingers_lost));
                if std::env::var_os("OLO_PALM_WHY").is_some() {
                    for c in run.iter().filter(|c| !c.palm && is_palm(&o, c)) {
                        let f = c.frames[c.frames.len() / 2];
                        println!(
                            "    a finger lost: size {} x {:.2} y {:.2} pressure {:.0} shape {:.2} speed {:.3} others {:.2} age-frames {}",
                            c.size,
                            f[4],
                            f[5],
                            f[2] * 6000.0,
                            f[12],
                            f[8],
                            f[11],
                            c.frames.len()
                        );
                    }
                }
                println!("  {name}, made into clicks and low touches:  {}", line("", u.palms_caught, u.fingers_lost));
                across.push(format!(
                    "{name}: {:.1}% and {:.1}%, its clicks and low touches {:.1}% and {:.1}%",
                    t.palms_caught * 100.0,
                    t.fingers_lost * 100.0,
                    u.palms_caught * 100.0,
                    u.fingers_lost * 100.0
                ));
            }
        }
        let list = |v: &[f32]| v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", ");
        let names: Vec<&str> = runs.iter().map(|(n, _)| n.as_str()).collect();
        let text = format!(
            "//! Olo's own palm model (`default_model`): `baseline` trained on the\n\
             //! trainer's runs kept on disk and the clicks and low touches made\n\
             //! from them, by `make_default_model` (don't edit: run it again).\n\
             //! From {} runs ({}), {} contacts and {} made from them; on the {} kept\n\
             //! back ({} palms, {} fingers) it caught {:.1}% of palms and took {:.1}%\n\
             //! of fingers for palms (the hand-written baseline: {:.1}% and {:.1}%;\n\
             //! the size rule: {:.1}% and {:.1}%). Each run by one made the same\n\
             //! way from the others only: {}.\n\n\
             pub(crate) const CONTACTS: u32 = {};\n\
             pub(crate) const PALMS_CAUGHT: f32 = {:?};\n\
             pub(crate) const FINGERS_LOST: f32 = {:?};\n\
             pub(crate) const THRESHOLD: f32 = {:?};\n\
             pub(crate) const B2: f32 = {:?};\n\
             pub(crate) const MEAN: &[f32] = &[{}];\n\
             pub(crate) const STD: &[f32] = &[{}];\n\
             pub(crate) const LO: &[f32] = &[{}];\n\
             pub(crate) const HI: &[f32] = &[{}];\n\
             pub(crate) const B1: &[f32] = &[{}];\n\
             pub(crate) const W2: &[f32] = &[{}];\n\
             pub(crate) const W1: &[f32] = &[{}];\n",
            runs.len(),
            names.join(", "),
            real.len(),
            all.len() - real.len(),
            r.contacts,
            r.palms,
            r.fingers,
            r.palms_caught * 100.0,
            r.fingers_lost * 100.0,
            r.baseline_palms_caught * 100.0,
            r.baseline_fingers_lost * 100.0,
            r.size_rule_palms_caught * 100.0,
            r.size_rule_fingers_lost * 100.0,
            across.join("; "),
            real.len(),
            r.palms_caught,
            r.fingers_lost,
            m.threshold,
            m.b2,
            list(&m.mean),
            list(&m.std),
            list(&m.lo),
            list(&m.hi),
            list(&m.b1),
            list(&m.w2),
            list(&m.w1),
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/default_model.rs");
        std::fs::write(&path, text).unwrap();
        println!("written to {} (cargo fmt it)", path.display());
    }

    fn line(what: &str, palms_caught: f32, fingers_lost: f32) -> String {
        format!("{what:<28} {:>5.1}% of palms caught, {:>5.1}% of fingers lost, cost {:.3}", palms_caught * 100.0, fingers_lost * 100.0, (1.0 - palms_caught) + FINGER_COST * fingers_lost)
    }

    #[test]
    #[ignore]
    fn on_real_hands() {
        let runs = runs();
        assert!(!runs.is_empty(), "no captures: OLO_PALM_CAPTURES=dir");
        let base = std::env::var_os("OLO_PALM_HAND_WRITTEN").map_or_else(default_model, |_| baseline());
        for (name, run) in &runs {
            let all: Vec<&Labelled> = run.iter().collect();
            let r = judge(&base, &all, 1600);
            println!("{name}: {} contacts ({} palms, {} fingers)", r.contacts, r.palms, r.fingers);
            println!("  {}", line("size rule (1600)", r.size_rule_palms_caught, r.size_rule_fingers_lost));
            println!("  {}", line("Olo's own model", r.palms_caught, r.fingers_lost));
            let floor = |c: &Labelled| is_palm(&base, c) || c.size >= 1600;
            let share = |palm: bool| run.iter().filter(|c| c.palm == palm && floor(c)).count() as f32 / run.iter().filter(|c| c.palm == palm).count().max(1) as f32;
            println!("  {}", line("Olo's own, or the size", share(true), share(false)));
            if std::env::var_os("OLO_PALM_WHY").is_some() {
                // The fingers taken for palms: their features against the
                // fingers kept and the palms (at each one's highest score),
                // and the units that take them.
                let top = |c: &Labelled| *c.frames.iter().max_by(|a, b| score(&base, a).total_cmp(&score(&base, b))).unwrap();
                let lost: Vec<[f32; FEATURES]> = run.iter().filter(|c| !c.palm && is_palm(&base, c)).map(top).collect();
                let kept: Vec<[f32; FEATURES]> = run.iter().filter(|c| !c.palm && !is_palm(&base, c)).map(top).collect();
                let palms: Vec<[f32; FEATURES]> = run.iter().filter(|c| c.palm).map(top).collect();
                let mean = |v: &[[f32; FEATURES]], i: usize| v.iter().map(|f| f[i]).sum::<f32>() / v.len().max(1) as f32;
                for i in 0..FEATURES {
                    println!("    feature {i:2}: fingers lost {:.3}, fingers kept {:.3}, palms {:.3}", mean(&lost, i), mean(&kept, i), mean(&palms, i));
                }
                let unit = |f: &[f32; FEATURES], j: usize| {
                    let a = base.b1[j] + (0..FEATURES).map(|i| base.w1[j * FEATURES + i] * (f[i] - base.mean[i]) / base.std[i]).sum::<f32>();
                    base.w2[j] * a.max(0.0)
                };
                let mut units: Vec<(usize, f32)> = (0..HIDDEN).map(|j| (j, lost.iter().map(|f| unit(f, j)).sum::<f32>() / lost.len().max(1) as f32)).collect();
                units.sort_by(|a, b| b.1.total_cmp(&a.1));
                println!("    units for the lost fingers: {:?}", units.iter().take(8).map(|(j, v)| format!("{j}: {v:+.2}")).collect::<Vec<_>>());
            }
            if std::env::var_os("OLO_PALM_SWEEP").is_some() {
                let verdict = |c: &Labelled| c.frames.iter().map(|f| score(&base, f)).fold(f32::MIN, f32::max);
                for t in [0.0f32, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0] {
                    // (As the filter has it: the size rule, or the model.)
                    let says = |c: &Labelled| verdict(c) > t || c.size >= 1600;
                    let caught = run.iter().filter(|c| c.palm && says(c)).count() as f32 / run.iter().filter(|c| c.palm).count().max(1) as f32;
                    let lost = run.iter().filter(|c| !c.palm && says(c)).count() as f32 / run.iter().filter(|c| !c.palm).count().max(1) as f32;
                    println!("    threshold {t:.1}, or the size: {}", line("", caught, lost));
                }
            }
            for (other, train_on) in runs.iter().filter(|(n, _)| n != name) {
                if let Some((m, _)) = train(Kind::MacBook, train_on, 1600, |_| {}) {
                    let t = judge(&m, &all, 1600);
                    println!("  {}", line(&format!("trained on {}", &other[4..14]), t.palms_caught, t.fingers_lost));
                }
            }
        }
    }
}

#[cfg(test)]
mod fingertips {
    use super::*;

    /// A fingertip resting on the pad, still or barely, light, anywhere in
    /// its middle, stays a finger to Olo's own model (fine tuning on its
    /// own once made this a palm: no run had one quite this still).
    #[test]
    fn a_resting_fingertip_is_a_finger() {
        let axes = PadAxes::macbook();
        let m = default_model();
        for (x, y) in [(0, 4500), (-4000, 2500), (4000, 6500)] {
            for jitter in [0, 6] {
                let mut t = Tracker::default();
                let mut c = Labelled::default();
                for k in 0..30i32 {
                    let wobble = if k % 2 == 0 { jitter } else { -jitter };
                    let contact = TouchContact { id: 1, x: x + wobble, y: y - wobble, major: 400 + wobble, minor: 300, pressure: 20 + wobble, ..Default::default() };
                    for (_, f, age) in t.frame(&axes, k as u64 * 8000, &[contact]) {
                        if (MIN_AGE_MS..=DECIDE_MS).contains(&age) {
                            c.frames.push(f);
                        }
                    }
                }
                let top = c.frames.iter().map(|f| score(&m, f)).fold(f32::MIN, f32::max);
                assert!(!is_palm(&m, &c), "at ({x}, {y}), jitter {jitter}: {top} against {}", m.threshold);
            }
        }
    }
}
