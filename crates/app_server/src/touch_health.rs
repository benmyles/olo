//! How the touchpad's reports reach app_server (Activity ▸ Displays ▸
//! Touchpad, `Request::Frames`): each report's latency, from the kernel's
//! time on it to app_server handling it, so libinput's work, Olo's touch
//! filter and its palm model (touch_filter.rs), and any wait for the
//! main thread, which reads the pad and draws frames too. Kept for a
//! minute; noting one is a clock read and a push.

use std::collections::VecDeque;

use olo_protocol::TouchHealth;

/// Reports are kept this long (the percentiles' window).
const WINDOW_US: u64 = 60_000_000;
/// At most this many: a minute at 250 reports a second (a pad sends
/// 125, with gestures and clicks between).
const KEPT: usize = 60 * 250;

/// The touchpad's last minute of reports: when each came and its latency.
#[derive(Default)]
pub struct Track {
    reports: VecDeque<(u64, u32)>,
}

impl Track {
    /// A report stamped `event_us` by the kernel, handled at `now_us`
    /// (both on the monotonic clock, as libinput's).
    pub fn note(&mut self, now_us: u64, event_us: u64) {
        let latency = now_us.saturating_sub(event_us).min(u32::MAX as u64) as u32;
        if self.reports.len() == KEPT {
            self.reports.pop_front();
        }
        self.reports.push_back((now_us, latency));
        while self.reports.front().is_some_and(|&(t, _)| now_us.saturating_sub(t) > WINDOW_US) {
            self.reports.pop_front();
        }
    }

    /// How many came in the last second, and the last minute's latencies.
    pub fn health(&self, now_us: u64) -> TouchHealth {
        let mut minute: Vec<u32> = self.reports.iter().filter(|(t, _)| now_us.saturating_sub(*t) <= WINDOW_US).map(|r| r.1).collect();
        minute.sort_unstable();
        let at = |q: f64| minute.get(((minute.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0) as u64;
        TouchHealth {
            reports_second: self.reports.iter().rev().take_while(|(t, _)| now_us.saturating_sub(*t) < 1_000_000).count() as u32,
            reports_minute: minute.len() as u32,
            latency_p50_us: at(0.5),
            latency_p99_us: at(0.99),
            latency_max_us: minute.last().copied().unwrap_or(0) as u64,
            ..Default::default()
        }
    }
}

/// The calling thread's scheduling into `h`: app_server's main thread,
/// which reads the pad and runs the palm model (IPC is answered on it).
pub fn scheduling(h: &mut TouchHealth) {
    // SAFETY: gettid has no preconditions.
    let tid = unsafe { libc::gettid() } as u32;
    h.nice = olo_system::priority::nice_of(tid).unwrap_or(0);
    h.realtime = olo_system::priority::is_realtime(tid);
    h.util_min = olo_system::priority::util_min_of(tid).unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latencies_over_the_last_minute() {
        let mut t = Track::default();
        // A pad at 125 Hz for two minutes, each report 300 µs late but
        // every fiftieth 4 ms: the first minute's gone.
        let mut now = 1_000_000;
        for i in 0..15_000u64 {
            now += 8_000;
            let late = if i % 50 == 49 { 4_000 } else { 300 };
            t.note(now, now - late);
        }
        let h = t.health(now);
        assert_eq!(h.reports_second, 125);
        assert!((7_400..=7_600).contains(&h.reports_minute), "{}", h.reports_minute);
        assert_eq!((h.latency_p50_us, h.latency_max_us), (300, 4_000));
        assert_eq!(h.latency_p99_us, 4_000);
        // (An event stamped after it's handled, by another clock, is 0.)
        t.note(now + 8_000, now + 9_000);
        assert_eq!(t.health(now + 8_000).latency_p50_us, 300);
    }

    /// What noting a report costs: `cargo test --release -p app_server
    /// note_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn note_cost() {
        let mut t = Track::default();
        let n = 1_000_000u64;
        let start = std::time::Instant::now();
        for i in 0..n {
            t.note(i * 8_000 + crate::contacts::now_us() % 2, i * 8_000);
        }
        println!("{:.1} ns a report (with a clock read)", start.elapsed().as_nanos() as f64 / n as f64);
        let start = std::time::Instant::now();
        let h = std::hint::black_box(t.health(n * 8_000));
        println!("{:.0} µs for the minute's health ({} reports)", start.elapsed().as_secs_f64() * 1e6, h.reports_minute);
    }
}
