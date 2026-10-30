//! The pool's activity over time: each minute's requests by client (with
//! their tokens and how many failed), kept for a day, for the chart in
//! the Clients window and `olo-pool --status --json`. Only counts, so it
//! is kept apart from the sealed accounts, in a plain file of its own
//! (`state_dir()/pool-activity.json`), written at most once a minute.

use std::{collections::VecDeque, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Outcome, Request};

/// How long it is kept.
pub const KEEP_MINUTES: i64 = 24 * 60;

/// A client's requests in one minute.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Minute {
    /// Minutes since the epoch.
    pub minute: i64,
    pub client: String,
    pub requests: u32,
    /// Of them, those that didn't complete (rate limited, failed).
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Activity {
    /// Oldest first.
    pub minutes: VecDeque<Minute>,
    /// Changed since it was last written.
    #[serde(skip)]
    pub dirty: bool,
}

/// What a client's requests came to over a stretch.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub requests: u64,
    pub failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn path() -> PathBuf {
    olo_xdg::state_dir().join("pool-activity.json")
}

impl Activity {
    /// Count request `r` in its minute.
    pub fn record(&mut self, r: &Request) {
        let minute = r.at.div_euclid(60);
        let client = if r.client.is_empty() { "Other" } else { r.client.as_str() };
        // Kept in order: the place after every minute up to its own.
        let at = self.minutes.partition_point(|m| m.minute <= minute);
        let same = self.minutes.range(..at).rev().take_while(|m| m.minute == minute).position(|m| m.client == client).map(|k| at - 1 - k);
        let i = same.unwrap_or_else(|| {
            self.minutes.insert(at, Minute { minute, client: client.to_owned(), ..Default::default() });
            at
        });
        let m = &mut self.minutes[i];
        m.requests += 1;
        if matches!(r.outcome, Outcome::RateLimited | Outcome::Failed) {
            m.failed += 1;
        }
        m.input_tokens += r.usage.input_tokens;
        m.output_tokens += r.usage.output_tokens;
        self.prune(r.at);
        self.dirty = true;
    }

    /// Forget what is older than a day before `now` (seconds).
    pub fn prune(&mut self, now: i64) {
        let oldest = now.div_euclid(60) - KEEP_MINUTES;
        while self.minutes.front().is_some_and(|m| m.minute <= oldest) {
            self.minutes.pop_front();
        }
    }

    /// The clients seen, Claude Code and Codex first.
    pub fn clients(&self) -> Vec<String> {
        let mut out: Vec<String> = ["Claude Code", "Codex"].map(String::from).to_vec();
        for m in &self.minutes {
            if !out.contains(&m.client) {
                out.push(m.client.clone());
            }
        }
        out
    }

    /// `client`'s requests in each `bucket` minutes of the `span` minutes
    /// up to `now` (seconds), as a stepped line for a graph: each stretch
    /// from its start to its end at its count (per minute, so a span's
    /// stretches of any length read the same), oldest first.
    pub fn series(&self, client: &str, now: i64, span: i64, bucket: i64) -> VecDeque<(f64, f64)> {
        let bucket = bucket.max(1);
        let end = now.div_euclid(60) + 1;
        let n = (span / bucket).max(1);
        let start = end - n * bucket;
        let mut counts = vec![0u32; n as usize];
        for m in self.minutes.iter().filter(|m| m.client == client && m.minute >= start && m.minute < end) {
            counts[((m.minute - start) / bucket) as usize] += m.requests;
        }
        let mut out = VecDeque::with_capacity(counts.len() * 2);
        for (k, count) in counts.into_iter().enumerate() {
            let from = (start + k as i64 * bucket) * 60;
            let rate = count as f64 / bucket as f64;
            out.push_back((from as f64, rate));
            out.push_back(((from + bucket * 60) as f64 - 0.001, rate));
        }
        out
    }

    /// `client`'s totals since `since` (seconds).
    pub fn totals(&self, client: &str, since: i64) -> Totals {
        let first = since.div_euclid(60);
        self.minutes.iter().filter(|m| m.client == client && m.minute >= first).fold(Totals::default(), |mut t, m| {
            t.requests += m.requests as u64;
            t.failed += m.failed as u64;
            t.input_tokens += m.input_tokens;
            t.output_tokens += m.output_tokens;
            t
        })
    }

    /// What was kept, if there is any.
    pub fn load() -> Activity {
        let mut a: Activity = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        a.minutes.make_contiguous().sort_by_key(|m| m.minute);
        a
    }

    /// Keep it (if changed): a new file put in place of the old.
    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Usage;

    fn request(at: i64, client: &str, outcome: Outcome) -> Request {
        Request { at, model: "m".into(), client: client.into(), outcome, status: 200, ms: 1, usage: Usage { requests: 1, input_tokens: 10, output_tokens: 2 } }
    }

    #[test]
    fn counts_by_minute_and_client() {
        let mut a = Activity::default();
        let t = 1_800_000_000; // a minute's start
        a.record(&request(t + 5, "Codex", Outcome::Completed));
        a.record(&request(t + 50, "Claude Code", Outcome::Completed));
        a.record(&request(t + 59, "Codex", Outcome::Failed));
        a.record(&request(t + 61, "Codex", Outcome::Completed));
        // One late, for a minute before.
        a.record(&request(t - 30, "Codex", Outcome::Completed));
        let minutes: Vec<(i64, &str, u32, u32)> = a.minutes.iter().map(|m| (m.minute - t / 60, m.client.as_str(), m.requests, m.failed)).collect();
        assert_eq!(minutes, vec![(-1, "Codex", 1, 0), (0, "Codex", 2, 1), (0, "Claude Code", 1, 0), (1, "Codex", 1, 0)]);
        let codex = a.totals("Codex", t);
        assert_eq!((codex.requests, codex.failed, codex.input_tokens), (3, 1, 30));
        // Two-minute stretches over the last four minutes: a step each.
        let s = a.series("Codex", t + 61, 4, 2);
        assert_eq!(s.len(), 4);
        assert_eq!(s.iter().map(|(_, v)| *v).collect::<Vec<_>>(), vec![0.5, 0.5, 1.5, 1.5]);
        assert_eq!(s[0].0 as i64, (t / 60 - 2) * 60);
        // A day on, it's gone.
        a.record(&request(t + KEEP_MINUTES * 60 + 120, "Codex", Outcome::Completed));
        assert_eq!(a.minutes.len(), 1);
        assert_eq!(a.clients()[..2], ["Claude Code".to_owned(), "Codex".to_owned()]);
    }
}
