//! Fine-tuning on the person's history. Each command run, as it was
//! being typed (its first letter, two, three, and its program's name and
//! a space), is an example: the candidates the index had then (only
//! what came before it), and which was typed. The loss is how unlikely
//! the model found it among them (softmax cross-entropy); Adam moves the
//! weights, held near where they started (an L2 pull, as the palm
//! model's), so a few examples don't undo what's known. The last tenth
//! of the examples, kept back, says how well it does: how often its
//! first suggestion was the command (`top1`), against the hand-written
//! weights'.
//!
//! The arithmetic is a [`Backend`]'s: the processor's (plain or SIMD,
//! here) or the GPU's (olo-complete's).

use serde::{Deserialize, Serialize};

use crate::{
    history::Entry,
    index::{Context, F, Index},
    kernels::{self, Cpu, H, PARAMS, Weights},
};

/// One command as it was typed: its candidates' features (`n × F`) and
/// which candidate it was.
#[derive(Debug, Clone)]
pub struct Example {
    pub x: Vec<f32>,
    pub target: usize,
}

impl Example {
    pub fn rows(&self) -> usize {
        self.x.len() / F
    }
}

/// Where a command is taken as being typed: its first one, two and three
/// characters, and its program's name and the space after.
pub fn typing_points(cmd: &str) -> Vec<usize> {
    let ends: Vec<usize> = cmd.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    let mut at: Vec<usize> = ends.iter().take(3).copied().collect();
    if let Some(sp) = cmd.find(' ') {
        at.push(sp + 1);
    }
    at.retain(|&k| k < cmd.len());
    at.sort_unstable();
    at.dedup();
    at
}

/// The example for `e` typed as far as `k` bytes, with the index as it
/// was before `e` ran (None: the command wasn't among its candidates).
pub fn example_at(ix: &Index, e: &Entry, k: usize) -> Option<Example> {
    let line = &e.command[..k];
    let cands = ix.candidates(line);
    let target = cands.iter().position(|&id| ix.command(id) == e.command)?;
    let cx = Context { line, cwd: e.cwd.as_deref(), session: e.session, at: e.at };
    Some(Example { x: ix.rows(&cx, &cands), target })
}

/// Every example in `entries`, oldest first, each made from what came
/// before it; the index after them all.
pub fn replay(entries: &[Entry]) -> (Vec<Example>, Index) {
    let mut ix = Index::new();
    let mut out = Vec::new();
    for e in entries {
        if ix.id(&e.command).is_some() {
            for k in typing_points(&e.command) {
                out.extend(example_at(&ix, e, k));
            }
        }
        ix.add(e);
    }
    (out, ix)
}

/// What does the arithmetic: scores for one line's candidates, and the
/// loss and gradient over examples.
pub trait Backend {
    fn name(&self) -> String;
    /// Why it isn't computing as asked (a GPU that stopped), if so.
    fn trouble(&self) -> Option<String> {
        None
    }
    fn scores(&mut self, w: &Weights, x: &[f32]) -> Vec<f32>;
    /// The mean loss over `examples` and its gradient.
    fn loss_and_grad(&mut self, w: &Weights, examples: &[&Example]) -> (f32, Weights);
}

/// The processor's arithmetic.
pub struct CpuBackend(pub Cpu);

impl Backend for CpuBackend {
    fn name(&self) -> String {
        match self.0 {
            Cpu::Scalar => "processor (plain)".into(),
            Cpu::Simd => format!("processor ({})", kernels::simd_name()),
        }
    }

    fn scores(&mut self, w: &Weights, x: &[f32]) -> Vec<f32> {
        let rows = x.len() / F;
        let (mut h, mut s) = (vec![0.0; rows * H], vec![0.0; rows]);
        kernels::forward(self.0, w, x, &mut h, &mut s);
        s
    }

    fn loss_and_grad(&mut self, w: &Weights, examples: &[&Example]) -> (f32, Weights) {
        let mut g = Weights::zeros();
        let mut loss = 0.0;
        let (mut h, mut s, mut ds) = (Vec::new(), Vec::new(), Vec::new());
        for ex in examples {
            let rows = ex.rows();
            h.resize(rows * H, 0.0);
            s.resize(rows, 0.0);
            kernels::forward(self.0, w, &ex.x, &mut h, &mut s);
            loss += softmax_grad(&s, ex.target, &mut ds);
            kernels::backward(self.0, w, &ex.x, &h, &ds, &mut g);
        }
        let n = examples.len().max(1) as f32;
        scale(&mut g, 1.0 / n);
        (loss / n, g)
    }
}

/// Cross-entropy of `target` under the softmax of `scores`; `ds` its
/// derivative by each score (the probabilities, less one at the target).
pub fn softmax_grad(scores: &[f32], target: usize, ds: &mut Vec<f32>) -> f32 {
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    ds.clear();
    ds.extend(scores.iter().map(|s| (s - max).exp()));
    let sum: f32 = ds.iter().sum();
    for d in ds.iter_mut() {
        *d /= sum;
    }
    let loss = -(ds[target].max(1e-12)).ln();
    ds[target] -= 1.0;
    loss
}

fn scale(g: &mut Weights, by: f32) {
    for v in g.w1.iter_mut().chain(g.b1.iter_mut()).chain(g.w2.iter_mut()) {
        *v *= by;
    }
    g.b2 *= by;
}

/// How well weights do on examples: how often their first suggestion is
/// the command, and the mean reciprocal of its place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub top1: f32,
    pub mrr: f32,
    pub examples: usize,
}

pub fn evaluate(b: &mut dyn Backend, w: &Weights, examples: &[Example]) -> Score {
    let (mut top1, mut mrr) = (0.0, 0.0);
    for ex in examples {
        let s = b.scores(w, &ex.x);
        let better = s.iter().filter(|&&v| v > s[ex.target]).count();
        top1 += f32::from(u8::from(better == 0));
        mrr += 1.0 / (better + 1) as f32;
    }
    let n = examples.len().max(1) as f32;
    Score { top1: top1 / n, mrr: mrr / n, examples: examples.len() }
}

/// How a fine-tune goes.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub epochs: usize,
    pub batch: usize,
    pub rate: f32,
    /// The L2 pull toward the weights it started from.
    pub hold: f32,
}

impl Plan {
    /// The first, on the whole history.
    pub const FIRST: Plan = Plan { epochs: 12, batch: 32, rate: 0.01, hold: 1e-3 };
    /// A minute's commands, with some of what came before.
    pub const QUICK: Plan = Plan { epochs: 3, batch: 32, rate: 0.005, hold: 3e-3 };
}

/// What a fine-tune did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tuned {
    pub before: Score,
    pub after: Score,
    /// The hand-written weights' on the same held-back examples.
    pub baseline: Score,
    pub examples: usize,
    pub steps: usize,
    pub loss: f32,
    pub micros: u64,
    pub backend: String,
    /// Kept: it did at least as well on the held-back examples as the
    /// weights it started from (else they stay).
    pub kept: bool,
}

/// Fine-tune `start` on `train`, judged on `held`: the new weights if
/// they do at least as well there, else `start`.
pub fn fine_tune(b: &mut dyn Backend, start: &Weights, train: &[Example], held: &[Example], plan: Plan) -> (Weights, Tuned) {
    let t0 = std::time::Instant::now();
    let before = evaluate(b, start, held);
    let baseline = evaluate(b, &Weights::baseline(), held);
    let anchor = start.flat();
    let mut p = anchor.clone();
    let (mut m, mut v) = (vec![0.0f32; PARAMS], vec![0.0f32; PARAMS]);
    let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
    let mut order: Vec<usize> = (0..train.len()).collect();
    let mut rng = 0x9e37_79b9u32;
    let (mut steps, mut loss) = (0usize, 0.0f32);
    for _ in 0..plan.epochs {
        // Shuffled each time (a little xorshift: the same each run).
        for i in (1..order.len()).rev() {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            order.swap(i, rng as usize % (i + 1));
        }
        for chunk in order.chunks(plan.batch.max(1)) {
            let w = Weights::from_flat(&p);
            let batch: Vec<&Example> = chunk.iter().map(|&i| &train[i]).collect();
            let (l, g) = b.loss_and_grad(&w, &batch);
            loss = l;
            steps += 1;
            let g = g.flat();
            let (c1, c2) = (1.0 - b1.powi(steps as i32), 1.0 - b2.powi(steps as i32));
            for k in 0..PARAMS {
                let gk = g[k] + plan.hold * (p[k] - anchor[k]);
                m[k] = b1 * m[k] + (1.0 - b1) * gk;
                v[k] = b2 * v[k] + (1.0 - b2) * gk * gk;
                p[k] -= plan.rate * (m[k] / c1) / ((v[k] / c2).sqrt() + eps);
            }
        }
    }
    let tuned = Weights::from_flat(&p);
    let after = evaluate(b, &tuned, held);
    let kept = tuned.sound() && (held.is_empty() || after.mrr >= before.mrr);
    let report = Tuned { before, after, baseline, examples: train.len(), steps, loss, micros: t0.elapsed().as_micros() as u64, backend: b.name(), kept };
    (if kept { tuned } else { start.clone() }, report)
}

/// Examples split in time: all but the last tenth to learn from, the last
/// tenth (at least one, when there are ten) held back to judge by.
pub fn split(examples: Vec<Example>) -> (Vec<Example>, Vec<Example>) {
    let held = if examples.len() >= 10 { examples.len() / 10 } else { 0 };
    let mut train = examples;
    let held = train.split_off(train.len() - held);
    (train, held)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up person: in ~/src they build, test and commit, in that
    /// order; in ~/notes they edit and sync; mornings they check mail.
    fn person(days: usize) -> Vec<Entry> {
        let mut out = Vec::new();
        let day0 = 1_767_225_600; // 2026-01-01 00:00 UTC.
        for d in 0..days {
            let at = |h: i64, m: i64| day0 + d as i64 * 86_400 + h * 3600 + m * 60;
            let s = d as u64 + 1;
            let e = |cmd: &str, cwd: &str, at: i64| Entry { cwd: Some(cwd.into()), at, session: s, status: Some(0), ..Entry::new(cmd) };
            out.push(e("mutt", "/home/me", at(8, 0)));
            for k in 0..3 {
                out.push(e("cargo build --release", "/home/me/src", at(10 + k, 0)));
                out.push(e("cargo test --release", "/home/me/src", at(10 + k, 5)));
                out.push(e("git commit -am wip", "/home/me/src", at(10 + k, 10)));
            }
            out.push(e("vim notes.md", "/home/me/notes", at(15, 0)));
            out.push(e("git pull --rebase", "/home/me/notes", at(15, 30)));
            out.push(e("git push", "/home/me/notes", at(15, 31)));
        }
        out
    }

    #[test]
    fn replay_makes_examples_from_what_came_before() {
        let (ex, ix) = replay(&person(3));
        assert!(ex.len() > 30, "{}", ex.len());
        assert!(ex.iter().all(|e| e.target < e.rows() && e.x.len() == e.rows() * F));
        assert_eq!(ix.len(), 7);
    }

    #[test]
    fn fine_tuning_learns_this_persons_order() {
        let (ex, _) = replay(&person(30));
        let (train, held) = split(ex);
        let mut b = CpuBackend(Cpu::Simd);
        let (w, t) = fine_tune(&mut b, &Weights::baseline(), &train, &held, Plan::FIRST);
        assert!(t.kept && w.sound());
        assert!(t.after.top1 >= t.baseline.top1, "{t:?}");
        assert!(t.after.top1 > 0.9, "{t:?}");
        // The order learned: in ~/src after a build, "cargo " is the test.
        let (_, ix) = replay(&person(30));
        let mut ix = ix;
        let s = 99;
        ix.add(&Entry { cwd: Some("/home/me/src".into()), session: s, ..Entry::new("cargo build --release") });
        let cx = Context { line: "cargo ", cwd: Some("/home/me/src"), session: s, at: 0 };
        let cands = ix.candidates("cargo ");
        let scores = b.scores(&w, &ix.rows(&cx, &cands));
        let best = cands[scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0];
        assert_eq!(ix.command(best), "cargo test --release");
    }

    #[test]
    fn simd_and_plain_train_alike() {
        let (ex, _) = replay(&person(5));
        let refs: Vec<&Example> = ex.iter().collect();
        let w = Weights::baseline();
        let (l0, g0) = CpuBackend(Cpu::Scalar).loss_and_grad(&w, &refs);
        let (l1, g1) = CpuBackend(Cpu::Simd).loss_and_grad(&w, &refs);
        assert!((l0 - l1).abs() < 1e-4);
        assert!(g0.flat().iter().zip(g1.flat()).all(|(a, b)| (a - b).abs() < 1e-4));
    }
}
