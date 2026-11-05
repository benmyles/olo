//! The palette's results ranked by a tiny model, as brush's suggestions
//! are (`olo_complete`'s kernels and trainer: [`F`] features of a
//! candidate through 16 hidden units to a score, in the processor's SIMD,
//! never a GPU), fine-tuned on what the person chooses after each choice.
//!
//! What it knows of a candidate ([`FEATURE_NAMES`]): how well what's
//! typed matches it (`rank::score_in`, and whether its title starts with
//! or is what's typed), its kind, whether it can be done now, and from
//! the person's choices: how often and how lately it was chosen, at this
//! hour and on this day, after typing what's typed now, with the app in
//! front now, whether it was the last thing chosen, and how often its
//! kind is. Its weights start written by hand, ranking as the palette did
//! before it ([`baseline`]); every choice is an example (the candidates
//! shown, their features, which was chosen) and the model is tuned again
//! on a worker after it (a few milliseconds), the new weights kept only
//! if they rank the newest choices at least as well.
//!
//! Kept in ~/.local/state/olo/palette/: `choices.jsonl` (each choice:
//! when, what was typed, its key, its kind, the app in front),
//! `examples.jsonl` (the newest [`EXAMPLES_KEPT`]) and `weights.json`.

use std::{collections::HashMap, io::Write as _, path::PathBuf};

use olo_complete::{
    kernels::{self, Cpu, H, Weights},
    train::{self, CpuBackend, Example, Plan},
};
use serde::{Deserialize, Serialize};

use crate::sources::{Action, Item};

/// Features of a candidate (the kernels' width).
pub const F: usize = olo_complete::index::F;

pub const FEATURE_NAMES: [&str; F] =
    ["match", "prefix", "exact", "kind", "enabled", "times", "recency", "hour", "weekday", "after_typed", "in_app", "last", "of_kind", "open", "title_len", "typed_len"];

/// Examples kept to learn from.
pub const EXAMPLES_KEPT: usize = 1000;
/// Candidates kept of each choice's (the best ranked, the chosen one
/// among them).
const ROWS_KEPT: usize = 24;
/// Choices kept (what the history features are worked out from).
const CHOICES_KEPT: usize = 5000;
/// How a tune goes: on every example kept, after each choice, so enough
/// passes and a step big enough that what the person keeps choosing wins
/// within a few dozen choices (brush's quick plan adds a minute's
/// commands to a long history, and moves little), the pull toward the
/// weights it starts from keeping one odd choice from undoing the rest.
const PLAN: Plan = Plan { epochs: 8, batch: 16, rate: 0.01, hold: 1e-3 };

/// The hand-written weights: each hidden unit passes one feature on
/// (ReLU of a feature at least 0 is itself), weighed as the palette's
/// ranking was before the model (`rank::score_in` + the kind's weight +
/// how often and lately chosen − 20 when it can't be done, in hundredths
/// of a word's best match), with what was chosen after the same typing
/// and in the same app counting most of the rest.
pub fn baseline() -> Weights {
    let mut w1 = vec![0.0; F * H];
    for i in 0..F.min(H) {
        w1[i * H + i] = 1.0;
    }
    // In FEATURE_NAMES' order.
    let w2 = vec![1.0, 0.1, 0.6, 1.0, 0.2, 1.2, 0.4, 0.2, 0.1, 0.8, 0.3, 0.2, 0.2, 0.05, -0.05, 0.0];
    Weights { w1, b1: vec![0.0; H], w2, b2: 0.0 }
}

/// One choice, as kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Seconds since the epoch.
    pub at: u64,
    /// What was typed, lowercased and trimmed.
    pub typed: String,
    pub key: String,
    pub kind: String,
    /// The app in front then (its app id).
    pub app: String,
}

/// A candidate's history, from the choices.
#[derive(Debug, Default, Clone)]
struct Stats {
    times: u32,
    last: u64,
    hours: [u16; 24],
    weekdays: [u16; 7],
    /// What was typed when it was chosen, the newest few.
    typed: Vec<String>,
    apps: HashMap<String, u16>,
}

/// The local hour and weekday (0 Sunday) of `at`.
fn local(at: u64) -> (usize, usize) {
    let t = at as libc::time_t;
    // SAFETY: localtime_r fills `tm` from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_hour.clamp(0, 23) as usize, tm.tm_wday.clamp(0, 6) as usize)
}

/// What an item is, for its history: its action's key, else (menu
/// commands and windows come and go, their keys none) the command in the
/// front app, or the window's app.
pub fn key_of(item: &Item, front_app: &str) -> Option<String> {
    if let Some(k) = item.action.key() {
        return Some(k);
    }
    match &item.action {
        Action::Menu { command, .. } => Some(format!("menu:{front_app}:{command}")),
        Action::Activate(_) => Some(format!("window:{}", item.subtitle)),
        _ => None,
    }
}

/// What the ranking is asked about now (`typed` lowercased).
pub struct Now<'a> {
    pub at: u64,
    pub typed: &'a str,
    pub front_app: &'a str,
    /// The local hour and weekday of `at`, worked out once for all the
    /// candidates.
    hour: usize,
    day: usize,
}

impl<'a> Now<'a> {
    pub fn new(at: u64, typed: &'a str, front_app: &'a str) -> Now<'a> {
        let (hour, day) = local(at);
        Now { at, typed, front_app, hour, day }
    }
}

/// `s` starts with `prefix` (lowercase), its case aside, nothing made.
fn starts_with_lower(s: &str, prefix: &str) -> bool {
    let mut s = s.chars().flat_map(char::to_lowercase);
    prefix.chars().all(|p| s.next() == Some(p))
}

/// `s` is `other` (lowercase), its case aside.
fn is_lower(s: &str, other: &str) -> bool {
    starts_with_lower(s, other) && s.chars().flat_map(char::to_lowercase).count() == other.chars().count()
}

/// The model and what it knows of the person's choices.
pub struct Ranker {
    pub weights: Weights,
    stats: HashMap<String, Stats>,
    kinds: HashMap<String, u32>,
    choices: u32,
    last: Option<String>,
}

fn dir() -> PathBuf {
    olo_xdg::state_dir().join("palette")
}

fn read_lines<T: for<'a> Deserialize<'a>>(name: &str) -> Vec<T> {
    std::fs::read_to_string(dir().join(name)).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// An example as kept: rows of [`F`] features (to three places) and the
/// chosen one's row.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Kept {
    rows: Vec<[f32; F]>,
    target: usize,
}

impl Ranker {
    /// A model of `weights` that knows no choices (for tests).
    #[cfg(test)]
    pub fn load_from(weights: Weights) -> Ranker {
        Ranker { weights, stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None }
    }

    /// The model as kept (else the hand-written one) and the history.
    pub fn load() -> Ranker {
        let weights = std::fs::read(dir().join("weights.json")).ok().and_then(|b| serde_json::from_slice::<Weights>(&b).ok()).filter(Weights::sound).unwrap_or_else(baseline);
        let mut r = Ranker { weights, stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None };
        for c in read_lines::<Choice>("choices.jsonl") {
            r.learn(&c);
        }
        r
    }

    fn learn(&mut self, c: &Choice) {
        let s = self.stats.entry(c.key.clone()).or_default();
        s.times += 1;
        s.last = s.last.max(c.at);
        let (hour, day) = local(c.at);
        s.hours[hour] = s.hours[hour].saturating_add(1);
        s.weekdays[day] = s.weekdays[day].saturating_add(1);
        if !c.typed.is_empty() {
            s.typed.push(c.typed.clone());
            if s.typed.len() > 20 {
                s.typed.remove(0);
            }
        }
        *s.apps.entry(c.app.clone()).or_default() += 1;
        *self.kinds.entry(c.kind.clone()).or_default() += 1;
        self.choices += 1;
        self.last = Some(c.key.clone());
    }

    /// `item`'s features, `matched` its match (`rank::score_in`) for
    /// `words` words, `open` whether its app has a window open.
    pub fn features(&self, item: &Item, matched: i32, words: usize, open: bool, now: &Now) -> [f32; F] {
        let typed = now.typed;
        let title = item.title.as_str();
        // (Its key only when there's a history to look it up in.)
        let key = if self.choices == 0 { None } else { key_of(item, now.front_app) };
        let s = key.as_ref().and_then(|k| self.stats.get(k));
        let (hour, day) = (now.hour, now.day);
        let share = |n: u16, of: u32| if of == 0 { 0.0 } else { f32::from(n) / of as f32 };
        let mut x = [0.0f32; F];
        x[0] = (matched as f32 / (100.0 * words.max(1) as f32)).clamp(0.0, 2.0);
        x[1] = f32::from(u8::from(!typed.is_empty() && starts_with_lower(title, typed)));
        x[2] = f32::from(u8::from(!typed.is_empty() && is_lower(title, typed)));
        x[3] = ((item.kind.weight() + 60) as f32 / 100.0).clamp(0.0, 2.0);
        x[4] = f32::from(u8::from(item.enabled));
        if let Some(s) = s {
            x[5] = ((1.0 + s.times.min(20) as f32).ln() / 21f32.ln()).min(1.0);
            let days = now.at.saturating_sub(s.last) as f32 / 86_400.0;
            x[6] = (-days / 3.0).exp();
            let near: u16 = (0..3).map(|d| s.hours[(hour + 23 + d) % 24]).sum();
            x[7] = share(near, s.times);
            x[8] = share(s.weekdays[day], s.times);
            let after = s.typed.iter().filter(|t| !typed.is_empty() && (t.starts_with(typed) || typed.starts_with(t.as_str()))).count();
            x[9] = ((1.0 + after as f32).ln() / 21f32.ln()).min(1.0);
            x[10] = share(s.apps.get(now.front_app).copied().unwrap_or(0), s.times);
        }
        x[11] = f32::from(u8::from(key.is_some() && key == self.last));
        x[12] = if self.choices == 0 { 0.0 } else { self.kinds.get(item.kind.label()).copied().unwrap_or(0) as f32 / self.choices as f32 };
        x[13] = f32::from(u8::from(open));
        x[14] = (title.chars().count() as f32 / 40.0).min(2.0);
        x[15] = (typed.chars().count() as f32 / 10.0).min(2.0);
        x
    }

    /// What it is and knows (`olo-palette --model`): its features with
    /// the weight each has where it was written by hand (passed straight
    /// on, as in [`baseline`]), how many choices and examples it has, and
    /// whether it has been tuned.
    pub fn describe(&self) -> serde_json::Value {
        let examples = std::fs::read_to_string(dir().join("examples.jsonl")).map_or(0, |t| t.lines().count());
        let direct: Vec<serde_json::Value> =
            FEATURE_NAMES.iter().enumerate().map(|(i, name)| serde_json::json!({ "feature": name, "weight": self.weights.w2[i] * self.weights.w1[i * H + i] })).collect();
        serde_json::json!({
            "features": direct,
            "tuned": self.weights != baseline(),
            "choices": self.choices,
            "examples": examples,
            "simd": kernels::simd_name(),
            "kept_in": dir(),
        })
    }

    /// Scores of candidates whose features are `rows` (higher better).
    pub fn scores(&self, rows: &[[f32; F]]) -> Vec<f32> {
        let x: Vec<f32> = rows.iter().flatten().copied().collect();
        let mut hidden = vec![0.0f32; rows.len() * H];
        let mut scores = vec![0.0f32; rows.len()];
        kernels::forward(Cpu::Simd, &self.weights, &x, &mut hidden, &mut scores);
        scores
    }

    /// `chosen` chosen among `rows` (the candidates as ranked, their
    /// features), noted: the choice and the example kept, the history
    /// knowing it at once. The examples, for a tune.
    pub fn chose(&mut self, choice: Choice, rows: &[[f32; F]], chosen: usize) {
        let _ = std::fs::create_dir_all(dir());
        append(&dir().join("choices.jsonl"), &choice, CHOICES_KEPT);
        if chosen < rows.len() && rows.len() > 1 {
            // The best ranked, the chosen one among them.
            let scores = self.scores(rows);
            let mut order: Vec<usize> = (0..rows.len()).collect();
            order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
            order.truncate(ROWS_KEPT);
            if !order.contains(&chosen) {
                order.pop();
                order.push(chosen);
            }
            let kept: Vec<[f32; F]> = order.iter().map(|&i| rows[i].map(|v| (v * 1000.0).round() / 1000.0)).collect();
            let target = order.iter().position(|&i| i == chosen).unwrap_or(0);
            append(&dir().join("examples.jsonl"), &Kept { rows: kept, target }, EXAMPLES_KEPT);
        }
        self.learn(&choice);
    }
}

/// `value` as a line at the end of `path`, the file cut to its newest
/// `most` lines now and then (when it has half as many again).
fn append<T: Serialize>(path: &std::path::Path, value: &T, most: usize) {
    let Ok(line) = serde_json::to_string(value) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > most + most / 2 {
        let keep = lines[lines.len() - most..].join("\n") + "\n";
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, keep).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// A tune's weights for the palette (None: kept as they were).
pub struct Retuned(pub Option<Weights>);

/// What a tune did, for the log.
#[derive(Debug, Clone)]
pub struct Tuned {
    pub weights: Weights,
    pub report: train::Tuned,
}

/// Tune `start` on the examples kept (a worker's: a few milliseconds),
/// the result saved if it ranks the newest choices at least as well.
pub fn tune(start: &Weights) -> Option<Tuned> {
    let examples: Vec<Example> = read_lines::<Kept>("examples.jsonl")
        .into_iter()
        .filter(|k| k.target < k.rows.len() && k.rows.len() > 1)
        .map(|k| Example { x: k.rows.iter().flatten().copied().collect(), target: k.target })
        .collect();
    let tuned = tune_on(start, examples)?;
    if tuned.report.kept
        && let Ok(json) = serde_json::to_vec(&tuned.weights)
    {
        let path = dir().join("weights.json");
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
    Some(tuned)
}

/// [`tune`] on `examples`, oldest first (the newest tenth held back to
/// judge by); none with fewer than ten.
pub fn tune_on(start: &Weights, examples: Vec<Example>) -> Option<Tuned> {
    if examples.len() < 10 {
        return None;
    }
    let (learn, held) = train::split(examples);
    let mut b = CpuBackend(Cpu::Simd);
    let (weights, report) = train::fine_tune_with(&mut b, start, &baseline(), &learn, &held, PLAN);
    Some(Tuned { weights, report })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Kind;

    fn item(kind: Kind, title: &str, key: &str) -> Item {
        Item::new(kind, title, Action::Shortcut(key.into()))
    }

    #[test]
    fn untrained_it_ranks_as_the_palette_did() {
        let r = Ranker { weights: baseline(), stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None };
        let now = Now::new(1_800_000_000, "term", "olo.tracker");
        let words = 1;
        // A title starting with what's typed over one with it inside;
        // an app over a preference that match alike.
        let a = r.features(&item(Kind::Application, "Terminal", "a"), crate::rank::score_in(&["term".into()], "Terminal", &[]).unwrap(), words, false, &now);
        let b = r.features(&item(Kind::Setting, "Determine Spacing", "b"), crate::rank::score_in(&["term".into()], "Determine Spacing", &[]).unwrap(), words, false, &now);
        let s = r.scores(&[a, b]);
        assert!(s[0] > s[1], "{s:?}");
        let app = r.features(&item(Kind::Application, "Tracker", "c"), 100, words, false, &now);
        let pref = r.features(&item(Kind::Preference, "Tracker", "d"), 100, words, false, &now);
        let s = r.scores(&[app, pref]);
        assert!(s[0] > s[1], "{s:?}");
    }

    #[test]
    fn what_was_chosen_after_this_typing_comes_first() {
        let mut r = Ranker { weights: baseline(), stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None };
        let at = 1_800_000_000;
        for i in 0..4 {
            r.learn(&Choice { at: at - 3600 * i, typed: "s".into(), key: "shortcut:screenshot".into(), kind: "Shortcut".into(), app: "olo.tracker".into() });
        }
        let now = Now::new(at, "s", "olo.tracker");
        // Two that match "s" alike: the one chosen after typing "s" wins.
        let settings = r.features(&item(Kind::Shortcut, "Settings", "settings"), 100, 1, false, &now);
        let shot = r.features(&item(Kind::Shortcut, "Screenshot", "screenshot"), 100, 1, false, &now);
        let s = r.scores(&[settings, shot]);
        assert!(s[1] > s[0], "{s:?}");
        assert!(shot[9] > 0.0 && shot[10] == 1.0 && shot[11] == 1.0, "{shot:?}");
    }

    #[test]
    fn features_stay_in_range() {
        let r = Ranker { weights: baseline(), stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None };
        let now = Now::new(0, "", "");
        let x = r.features(&item(Kind::File, &"x".repeat(500), "f"), -50, 0, true, &now);
        assert!(x.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 2.0), "{x:?}");
        assert_eq!(FEATURE_NAMES.len(), F);
    }

    /// A person who always takes the second of two things that match
    /// alike (Screenshot over Settings, typing "s"), with nothing else to
    /// tell them apart: tuned on that, the model ranks it first.
    #[test]
    fn it_learns_what_the_person_takes() {
        let r = Ranker { weights: baseline(), stats: HashMap::new(), kinds: HashMap::new(), choices: 0, last: None };
        let now = Now::new(1_800_000_000, "s", "olo.tracker");
        // The one taken is a Preference (weighed less by hand), the other a
        // Setting: by hand, the other comes first.
        let other = r.features(&item(Kind::Setting, "Sound Settings", "a"), 100, 1, false, &now);
        let taken = r.features(&item(Kind::Preference, "Screenshot", "b"), 100, 1, false, &now);
        let s = r.scores(&[other, taken]);
        assert!(s[0] > s[1], "by hand, the other first: {s:?}");
        let examples: Vec<Example> = (0..200).map(|_| Example { x: [other, taken].iter().flatten().copied().collect(), target: 1 }).collect();
        let tuned = tune_on(&r.weights, examples).unwrap();
        assert!(tuned.report.kept, "{:?}", tuned.report);
        let r = Ranker { weights: tuned.weights, ..r };
        let s = r.scores(&[other, taken]);
        assert!(s[1] > s[0], "tuned, the one taken first: {s:?}");
        eprintln!("tuned on 200 choices in {} µs ({})", tuned.report.micros, tuned.report.backend);
    }

    /// What a tune costs on a full history: the examples kept, each with
    /// the most candidates kept (`--ignored`, on release builds).
    #[test]
    #[ignore]
    fn tune_cost() {
        let mut rng = 0x1234_5678u32;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng % 1000) as f32 / 1000.0
        };
        let examples: Vec<Example> = (0..EXAMPLES_KEPT)
            .map(|_| {
                let x: Vec<f32> = (0..ROWS_KEPT * F).map(|_| next()).collect();
                Example { x, target: (next() * ROWS_KEPT as f32) as usize % ROWS_KEPT }
            })
            .collect();
        let mut times: Vec<u64> = (0..5).map(|_| tune_on(&baseline(), examples.clone()).unwrap().report.micros).collect();
        times.sort();
        eprintln!("a tune on {} examples of {} candidates: median {} µs, max {} µs ({})", EXAMPLES_KEPT, ROWS_KEPT, times[2], times[4], kernels::simd_name());
    }
}
