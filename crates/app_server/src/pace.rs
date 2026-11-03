//! Scrolling on the frame clock. A touchpad reports at its own rate (90,
//! 125 Hz…), which beats against the display's: passed on as they come,
//! one frame gets one report and the next two, so the content judders
//! though the fingers moved evenly. So finger scrolls are held back and,
//! once a frame of the screen under the pointer, just before clients are
//! told to draw, app_server sends where the fingers were a report's time
//! ago, interpolated between the reports either side of then (as
//! Android's input resampling does). The glide after a flick
//! (`momentum.rs`) goes the same way, along the same delayed clock from
//! where the fingers left off, so each frame gets exactly one scroll, as
//! far as the fingers (or the glide) went in that frame's time.
//!
//! Olo's own apps get the glide as a scroll of its own (`Continuous`,
//! focus.rs), so they bounce off an edge as on a Mac rather than pull at
//! it; everything else sees one finger scroll, its stop held back until
//! the glide ends.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::momentum::Momentum;

/// Reports this far apart are assumed until a gesture shows its own.
const USUAL: Duration = Duration::from_millis(8);
/// The touchpad's interval is taken to be within these.
const SHORTEST: Duration = Duration::from_millis(4);
const LONGEST: Duration = Duration::from_millis(16);
/// Behind the fingers by their report interval and this, so a report
/// that comes a little late is still in before its time is drawn.
const SLACK: Duration = Duration::from_micros(1500);

/// When a report says the fingers were where it says (`at_us`, the
/// kernel's time on a pad's report, on the monotonic clock), as an
/// Instant: the pacer follows the fingers by when they moved, not by
/// when app_server got to the report (its loop busy drawing, a report a
/// few ms late was a short step then a long one). A time from another
/// clock, ahead or long ago, is taken as now.
pub fn reported(at_us: u64) -> Instant {
    let now = Instant::now();
    match crate::contacts::now_us().checked_sub(at_us) {
        Some(age) if age <= 50_000 => now.checked_sub(Duration::from_micros(age)).unwrap_or(now),
        _ => now,
    }
}

/// What to send a client, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Out {
    /// A scroll by (dx, dy): the fingers', or the glide's.
    Move { dx: f64, dy: f64, glide: bool },
    /// It is over: the fingers lifted (with no glide), the glide ended,
    /// or it was stopped.
    Stop { glide: bool },
}

#[derive(Debug)]
pub struct Pacer {
    pub momentum: Momentum,
    /// This gesture's reports: when, and how far the fingers had gone
    /// in all; the first is where they started, a report's time before
    /// the first.
    reports: VecDeque<(Instant, f64, f64)>,
    /// How far of it has been sent.
    sent: (f64, f64),
    /// The fingers have lifted: once the rest is sent, the stop or the
    /// glide.
    lifted: bool,
    /// Gliding, from the time the fingers left off.
    gliding: bool,
    /// The touchpad's report interval, as last measured.
    interval: Duration,
    /// A glide is what is being sent (for focus.rs).
    pub sending_glide: bool,
}

impl Pacer {
    /// A scroll is being sent, a frame's worth a frame, or gliding on.
    pub fn busy(&self) -> bool {
        !self.reports.is_empty() || self.gliding
    }
}

impl Default for Pacer {
    fn default() -> Self {
        Pacer { momentum: Momentum::default(), reports: VecDeque::new(), sent: (0.0, 0.0), lifted: false, gliding: false, interval: USUAL, sending_glide: false }
    }
}

impl Pacer {
    /// How far behind the fingers the scroll sent runs.
    pub fn delay(&self) -> Duration {
        self.interval.clamp(SHORTEST, LONGEST) + SLACK
    }

    /// The fingers moved by (dx, dy). Returns what to send now: the end
    /// of whatever this cuts short.
    pub fn fingers(&mut self, now: Instant, dx: f64, dy: f64) -> Vec<Out> {
        let mut out = Vec::new();
        if self.lifted || self.gliding {
            out = self.stop();
        }
        self.momentum.scrolled(now, dx, dy);
        let (_, x, y) = *self.reports.back().unwrap_or(&(now, 0.0, 0.0));
        if self.reports.is_empty() {
            self.reports.push_back((now.checked_sub(self.interval).unwrap_or(now), 0.0, 0.0));
        } else if let Some(&(last, ..)) = self.reports.back() {
            // The interval, smoothed: reports come with some jitter.
            let gap = now.saturating_duration_since(last);
            if gap >= SHORTEST / 2 && gap <= LONGEST * 2 {
                self.interval = (self.interval * 3 + gap) / 4;
            }
        }
        self.reports.push_back((now, x + dx, y + dy));
        out
    }

    /// The fingers lifted (libinput's scroll of 0).
    pub fn lifted(&mut self, now: Instant) {
        if self.reports.is_empty() {
            // Nothing to finish (a lift on its own).
            self.momentum.cancel();
            return;
        }
        self.lifted = true;
        if self.momentum.lifted(now) {
            // From where the fingers left off.
            let from = self.reports.back().map_or(now, |r| r.0);
            self.momentum.glide_from(from);
        }
    }

    /// A frame: what to send for it, `now` its time.
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        if self.reports.is_empty() && !self.gliding {
            return out;
        }
        let t = now.checked_sub(self.delay()).unwrap_or(now);
        if !self.reports.is_empty() {
            let (x, y) = self.at(t);
            let (dx, dy) = (x - self.sent.0, y - self.sent.1);
            if dx != 0.0 || dy != 0.0 {
                out.push(Out::Move { dx, dy, glide: false });
                self.sent = (x, y);
            }
            let last = self.reports.back().map_or(t, |r| r.0);
            if !self.lifted || t < last {
                return out;
            }
            // All of the fingers' scroll is out.
            self.reports.clear();
            self.sent = (0.0, 0.0);
            self.lifted = false;
            if !self.momentum.gliding() {
                out.push(Out::Stop { glide: false });
                return out;
            }
            self.gliding = true;
        }
        match self.momentum.step(t) {
            Some((dx, dy)) => {
                if dx != 0.0 || dy != 0.0 {
                    out.push(Out::Move { dx, dy, glide: true });
                }
            }
            None => {
                self.gliding = false;
                out.push(Out::Stop { glide: true });
            }
        }
        out
    }

    /// Stop now (fingers back on the touchpad, a click, a wheel): the
    /// rest of the fingers' scroll and the stop, if it was going.
    pub fn stop(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(&(_, x, y)) = self.reports.back() {
            let (dx, dy) = (x - self.sent.0, y - self.sent.1);
            if dx != 0.0 || dy != 0.0 {
                out.push(Out::Move { dx, dy, glide: false });
            }
        }
        let fingers = !self.reports.is_empty();
        self.reports.clear();
        self.sent = (0.0, 0.0);
        self.lifted = false;
        let glided = self.momentum.cancel() && self.gliding;
        self.gliding = false;
        if fingers || glided {
            out.push(Out::Stop { glide: glided && !fingers });
        }
        out
    }

    /// Where the fingers were at `t`, between the reports either side.
    fn at(&mut self, t: Instant) -> (f64, f64) {
        // Reports before the one just before `t` are done with.
        while self.reports.len() > 2 && self.reports[1].0 <= t {
            self.reports.pop_front();
        }
        let r = &self.reports;
        let Some(i) = r.iter().position(|p| p.0 > t) else {
            let &(_, x, y) = r.back().expect("reports");
            return (x, y);
        };
        if i == 0 {
            return (r[0].1, r[0].2);
        }
        let ((t0, x0, y0), (t1, x1, y1)) = (r[i - 1], r[i]);
        let span = t1.duration_since(t0).as_secs_f64();
        let k = if span > 0.0 { t.duration_since(t0).as_secs_f64() / span } else { 1.0 };
        (x0 + (x1 - x0) * k, y0 + (y1 - y0) * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_when_the_pad_said() {
        let now_us = crate::contacts::now_us();
        let age = Instant::now() - reported(now_us - 3_000);
        assert!(age >= Duration::from_micros(2_900) && age < Duration::from_millis(20), "{age:?}");
        // (A time from another clock, ahead or long ago, is now.)
        assert!(Instant::now() - reported(now_us + 1_000_000) < Duration::from_millis(5));
        assert!(Instant::now() - reported(now_us.saturating_sub(10_000_000)) < Duration::from_millis(5));
    }

    fn moved(out: &[Out]) -> f64 {
        out.iter().map(|o| if let Out::Move { dy, .. } = o { *dy } else { 0.0 }).sum()
    }

    /// Fingers at a steady speed, reported every `report` µs, drawn
    /// every `frame` µs: each frame's scroll, once under way.
    fn steady(report: u64, frame: u64) -> Vec<f64> {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        let speed = 1.0; // px/ms
        let (mut next_report, mut next_frame, mut steps) = (report, frame, Vec::new());
        while next_frame < 600_000 {
            if next_report <= next_frame {
                p.fingers(t0 + Duration::from_micros(next_report), 0.0, speed * report as f64 / 1000.0);
                next_report += report;
            } else {
                steps.push(moved(&p.tick(t0 + Duration::from_micros(next_frame))));
                next_frame += frame;
            }
        }
        steps.drain(..10);
        steps
    }

    #[test]
    fn every_frame_moves_as_far() {
        // 90 Hz and 125 Hz touchpads on 60 Hz and 120 Hz screens.
        for (report, frame) in [(11_111, 16_667), (8_000, 16_667), (11_111, 8_333), (8_000, 8_333), (7_000, 8_333)] {
            let steps = steady(report, frame);
            let want = frame as f64 / 1000.0;
            for s in &steps {
                assert!((s - want).abs() < want * 0.02, "{report} µs reports, {frame} µs frames: {steps:?}");
            }
        }
    }

    #[test]
    fn a_lift_sends_the_rest_then_the_stop() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        let mut total = 0.0;
        for i in 1..=5 {
            p.fingers(t0 + USUAL * i, 0.0, 2.0);
        }
        p.lifted(t0 + USUAL * 5 + Duration::from_millis(100));
        let mut stopped = false;
        for f in 1..40 {
            let out = p.tick(t0 + Duration::from_micros(8_333) * f);
            total += moved(&out);
            stopped |= out.contains(&Out::Stop { glide: false });
        }
        assert_eq!(total, 10.0);
        assert!(stopped);
        assert!(p.tick(t0 + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn a_flick_glides_on_from_where_the_fingers_left_off() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        for i in 1..=10 {
            p.fingers(t0 + USUAL * i, 0.0, 16.0); // 2 px/ms
        }
        p.lifted(t0 + USUAL * 11);
        let frame = Duration::from_micros(8_333);
        let (mut steps, mut glide_stop) = (Vec::new(), false);
        for f in 1..800 {
            let out = p.tick(t0 + frame * f);
            steps.push(moved(&out));
            glide_stop |= out.contains(&Out::Stop { glide: true });
        }
        assert!(glide_stop);
        // From the fingers' 16.7 px a frame into the glide, no frame
        // jumps: each within a few percent of the one before.
        let moving: Vec<f64> = steps.iter().copied().skip_while(|s| *s < 16.0).take_while(|s| *s > 0.5).collect();
        for w in moving.windows(2) {
            assert!(w[1] <= w[0] * 1.02 && w[1] >= w[0] * 0.9, "{moving:?}");
        }
    }

    #[test]
    fn touching_again_sends_the_rest_and_stops() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        for i in 1..=3 {
            p.fingers(t0 + USUAL * i, 0.0, 5.0);
        }
        let out = p.stop();
        assert_eq!(moved(&out), 15.0);
        assert_eq!(out.last(), Some(&Out::Stop { glide: false }));
        assert!(p.stop().is_empty());
    }
}
