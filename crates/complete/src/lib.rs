//! Command suggestions from a tiny model of the person's own shell
//! history (Ben, 2026-11-03: "a tiny ai model for brush shell command
//! autocompletion that is accelerated either via simd or gpu ... based
//! on the existing bash history and take into account what dir the user
//! is in, what commands have come before in the session, time of day and
//! day of week").
//!
//! - [`history`]: the commands run, from Olo's record and the history
//!   files.
//! - [`index`]: what's known of each command, the candidates for what's
//!   typed, and the [`index::F`] features the model sees of each.
//! - [`kernels`]: the model (16 → 16 → 1) and its arithmetic, plain and
//!   in SIMD written for the processor.
//! - [`train`]: examples from the history as it was typed, fine-tuning,
//!   and how well it does.
//! - [`protocol`]: what brush and Olo's apps say to olo-complete (the
//!   daemon: apps/complete, which also computes on the GPU).

pub mod history;
pub mod index;
pub mod kernels;
pub mod protocol;
pub mod train;

use index::{Context, Index};
use kernels::Weights;
use train::Backend;

/// What `line` likely becomes: the likeliest candidates whole, best
/// first (at most `n`), by `w` computed with `b`.
pub fn suggest(ix: &Index, b: &mut dyn Backend, w: &Weights, cx: &Context, n: usize) -> Vec<String> {
    let cands = ix.candidates(cx.line);
    if cands.is_empty() {
        return Vec::new();
    }
    let scores = b.scores(w, &ix.rows(cx, &cands));
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    order.into_iter().take(n).map(|i| ix.command(cands[i]).to_owned()).collect()
}
