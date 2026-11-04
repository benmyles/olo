//! What the person does with each kind of input, the last minute back:
//! clicks, scrolls, gestures and keys pressed (how many, never which),
//! for Activity Monitor ▸ Inputs (`InputActivity` in `Reply::Frames`).
//! Each is a time noted as it's handled, the older than a minute let go
//! as the next comes: 12–27 ns an event (`an_event_costs_nanoseconds`),
//! nothing while idle.

use std::{collections::VecDeque, time::Instant};

use olo_protocol::InputActivity;

/// What's counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Click,
    Scroll,
    Gesture,
    Key,
}

const MINUTE: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct InputCounts {
    clicks: VecDeque<Instant>,
    scrolls: VecDeque<Instant>,
    gestures: VecDeque<Instant>,
    keys: VecDeque<Instant>,
}

impl InputCounts {
    pub fn note(&mut self, what: Input, at: Instant) {
        let q = match what {
            Input::Click => &mut self.clicks,
            Input::Scroll => &mut self.scrolls,
            Input::Gesture => &mut self.gestures,
            Input::Key => &mut self.keys,
        };
        while q.front().is_some_and(|t| at.duration_since(*t) > MINUTE) {
            q.pop_front();
        }
        q.push_back(at);
    }

    /// Each kind in the minute to `now`.
    pub fn activity(&self, now: Instant) -> InputActivity {
        let n = |q: &VecDeque<Instant>| q.iter().filter(|t| now.duration_since(**t) <= MINUTE).count() as u32;
        InputActivity { clicks_minute: n(&self.clicks), scrolls_minute: n(&self.scrolls), gestures_minute: n(&self.gestures), keys_minute: n(&self.keys) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Its cost an event: `cargo test --release -p app_server
    /// input_counts -- --nocapture`.
    #[test]
    fn an_event_costs_nanoseconds() {
        let mut c = InputCounts::default();
        let t0 = Instant::now();
        const N: u32 = 1_000_000;
        let start = Instant::now();
        // (A key every 5 ms: a minute holds 12 000, let go as they age.)
        for i in 0..N {
            c.note(Input::Key, t0 + std::time::Duration::from_millis(i as u64 * 5));
        }
        let ns = start.elapsed().as_nanos() as f64 / N as f64;
        eprintln!("input_counts: {ns:.1} ns an event");
        assert!(ns < 200.0, "{ns}");
    }

    #[test]
    fn counts_the_last_minute() {
        let mut c = InputCounts::default();
        let t0 = Instant::now();
        c.note(Input::Click, t0);
        c.note(Input::Key, t0);
        c.note(Input::Key, t0 + std::time::Duration::from_secs(30));
        let a = c.activity(t0 + std::time::Duration::from_secs(45));
        assert_eq!((a.clicks_minute, a.keys_minute, a.scrolls_minute), (1, 2, 0));
        let b = c.activity(t0 + std::time::Duration::from_secs(75));
        assert_eq!((b.clicks_minute, b.keys_minute), (0, 1));
    }
}
