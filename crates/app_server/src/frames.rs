//! Every frame on time, on every display, and the record of how each
//! keeps up (`Request::Frames`, Activity Monitor's Displays).
//!
//! After a vblank app_server waits before drawing the next frame, so the
//! clients told to draw at that vblank have time to hand in their buffers
//! and still make the next one (udev.rs `frame_finish_inner`). Smithay
//! splits the frame 60/40, the compositor drawing in the last 40%: 6.7 ms
//! at 60 Hz, 3.3 ms at 120 Hz. A 5K frame with a video playing costs
//! 5–9 ms of CPU here (asahi, 2026-11-01), so a third of the Studio
//! Display's vblanks were missed, each a frame held twice: the pointer
//! jerked and scrolls stuttered. [`Pacing`] sets each display's split from
//! what its frames cost: forward from the last frames' CPU time, and back
//! from the vblank's hardware timestamp, which says whether the frame
//! drawn for it made it (a frame or more late: dropped). A drop widens
//! the budget at once; a long run of frames on time narrows it again,
//! since a narrower budget is a later draw and less latency for everyone.
//! Nothing here runs while a display rests.
//!
//! With `Request::CaptureDrops` every drop is logged with what went into
//! the frame (the parts of the draw, what else held the loop) as a
//! `frames` record, for Logs.

use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use olo_protocol::DisplayFrames;
use smithay::utils::{Monotonic, Time};

/// Drops are captured (`Request::CaptureDrops`), on every display.
static CAPTURE: AtomicBool = AtomicBool::new(false);

pub fn capture() -> bool {
    CAPTURE.load(Ordering::Relaxed)
}

pub fn set_capture(on: bool) {
    CAPTURE.store(on, Ordering::Relaxed);
}

/// How many of the last frames' costs are kept (the percentiles' window).
const KEPT: usize = 120;
/// Frames on time in a row before the budget narrows a step.
const SETTLE: u32 = 600;
/// Past what the last frames cost, for the GPU's share and a bad one.
const HEADROOM: Duration = Duration::from_micros(500);

/// One display's pacing and record.
#[derive(Debug)]
pub struct Pacing {
    /// Kept for drawing, before the vblank.
    budget: Duration,
    /// What the last `KEPT` frames cost to draw (CPU), oldest first.
    renders: VecDeque<Duration>,
    /// Frames presented on time in a row.
    clean: u32,
    frames: u64,
    late: u64,
    /// When the late ones of the last minute were presented.
    late_when: VecDeque<Instant>,
    /// The last second's presentations.
    presented: VecDeque<Instant>,
    /// The vblank the frame being drawn now is for (set when the vblank's
    /// timer fires; a look of the display's own has none).
    expected: Option<Time<Monotonic>>,
    /// The frame on its way: its vblank, if drawn for one, and its cost.
    queued: Option<(Option<Time<Monotonic>>, Duration)>,
    /// The parts of the frame on its way, while capturing.
    parts: Vec<(&'static str, Duration)>,
    /// The last vblank's time, and how long before it the one before
    /// came: a panel idling (the MacBook's ProMotion drops to 80 or
    /// 60 Hz with nothing new) has long intervals, and a frame after one
    /// isn't late, the vblank is.
    last_vblank: Option<Time<Monotonic>>,
    interval: Option<Duration>,
    /// The pointer went on the display's cursor plane in the last frame.
    pub cursor_plane: bool,
    /// What the last frame scanned out and drew (`scanout_summary`).
    pub scanout: String,
}

/// What a frame's elements came to: how many went straight to a plane
/// (zero-copy) and how many had to be drawn, by Smithay's reason:
/// "1 on a plane; 6 drawn (1 overlaps, 2 format, 3 not tried)".
pub fn scanout_summary(states: &smithay::backend::renderer::element::RenderElementStates) -> String {
    use smithay::backend::renderer::element::{RenderElementPresentationState as P, RenderingReason as R};
    let (mut planes, mut drawn, mut skipped) = (0, 0, 0);
    let mut why: std::collections::BTreeMap<&'static str, u32> = Default::default();
    for s in states.states.values() {
        match s.presentation_state {
            P::ZeroCopy => planes += 1,
            P::Skipped => skipped += 1,
            P::Rendering { reason } => {
                drawn += 1;
                let r = match reason {
                    None => "not tried",
                    Some(R::FormatUnsupported) => "format",
                    Some(R::ScanoutFailed) => "the driver refused",
                    Some(R::Overlaps) => "overlaps",
                    Some(R::Underlay) => "underlay",
                    Some(R::PlaneTaken) => "planes taken",
                    Some(R::NoScanoutBuffer) => "no dmabuf",
                };
                *why.entry(r).or_default() += 1;
            }
        }
    }
    let why: Vec<String> = why.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let mut s = format!("{planes} on a plane; {drawn} drawn ({})", why.join(", "));
    if skipped > 0 {
        s.push_str(&format!("; {skipped} hidden"));
    }
    s
}

/// A frame presented late (`Pacing::presented`), for the log.
#[derive(Debug)]
pub struct Drop {
    /// How far past its vblank.
    pub late: Duration,
    /// What it cost to draw.
    pub took: Duration,
    pub budget: Duration,
    pub parts: Vec<(&'static str, Duration)>,
}

impl Drop {
    /// The parts, as "gather 1.2 ms, drm render_frame 6.8 ms".
    pub fn parts_text(&self) -> String {
        self.parts.iter().map(|(n, d)| format!("{n} {:.1} ms", d.as_secs_f64() * 1e3)).collect::<Vec<_>>().join(", ")
    }
}

impl Default for Pacing {
    fn default() -> Self {
        Pacing {
            budget: Duration::ZERO,
            renders: VecDeque::with_capacity(KEPT),
            clean: 0,
            frames: 0,
            late: 0,
            late_when: VecDeque::new(),
            presented: VecDeque::new(),
            expected: None,
            queued: None,
            parts: Vec::new(),
            last_vblank: None,
            interval: None,
            cursor_plane: false,
            scanout: String::new(),
        }
    }
}

impl Pacing {
    /// Smithay's split: the floor of the budget.
    fn least(frame: Duration) -> Duration {
        frame.mul_f64(0.4)
    }

    /// What the last frames say the next needs: past their 95th
    /// percentile, with headroom.
    fn need(&self) -> Duration {
        if self.renders.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted: Vec<Duration> = self.renders.iter().copied().collect();
        sorted.sort_unstable();
        let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        p95.mul_f64(1.25) + HEADROOM
    }

    /// How long after a vblank the next frame starts: the frame less the
    /// budget.
    pub fn delay(&mut self, frame: Duration) -> Duration {
        let least = Self::least(frame);
        self.budget = self.budget.max(least).max(self.need()).min(frame);
        frame.saturating_sub(self.budget)
    }

    /// The vblank's timer fired: the frame drawn now is for `target`.
    pub fn expect(&mut self, target: Time<Monotonic>) {
        self.expected = Some(target);
    }

    /// A frame was drawn, in `took`, and handed over (`queued`) or found
    /// nothing new.
    pub fn drawn(&mut self, took: Duration, parts: &[(&'static str, Duration)], queued: bool) {
        if self.renders.len() == KEPT {
            self.renders.pop_front();
        }
        self.renders.push_back(took);
        let target = self.expected.take();
        if queued {
            self.queued = Some((target, took));
            if capture() {
                self.parts = parts.to_vec();
            }
        }
    }

    /// A vblank came at `at` (the hardware's time), presenting the frame
    /// on its way: a [`Drop`] if that frame was for an earlier vblank.
    pub fn presented(&mut self, at: Time<Monotonic>, frame: Duration) -> Option<Drop> {
        let now = Instant::now();
        self.frames += 1;
        self.presented.push_back(now);
        self.tidy(now);
        // The display's cadence before this vblank: at its rate, or idling.
        let before = self.interval;
        self.interval = self.last_vblank.replace(at).map(|last| Duration::from(at).saturating_sub(Duration::from(last)));
        let (target, took) = self.queued.take()?;
        let target = target?;
        let late = Duration::from(at).saturating_sub(Duration::from(target));
        // A panel that was idling (its last interval well over a frame)
        // presents the frame at its next vblank, whenever its cadence
        // brings one: not a drop, however late against a full-rate frame.
        let idling = before.is_some_and(|i| i > frame + frame / 4);
        if idling {
            self.parts.clear();
            return None;
        }
        // Three quarters of a frame: a frame that made the next vblank is a
        // whole frame late, and a panel with a rate of its own (the
        // MacBook's ProMotion at 120 Hz) stamps a vblank up to half a
        // frame off now and then (seen 2026-11-01: 4.2–4.8 ms at 8.3 ms a
        // frame, with the draw well inside the frame).
        if late < frame * 3 / 4 {
            self.clean += 1;
            if self.clean >= SETTLE {
                self.clean = 0;
                self.budget = self.budget.saturating_sub(frame / 16).max(Self::least(frame));
            }
            self.parts.clear();
            return None;
        }
        self.late += 1;
        self.late_when.push_back(now);
        self.clean = 0;
        self.budget = (self.budget.max(Self::least(frame)) + frame / 4).min(frame);
        Some(Drop { late, took, budget: self.budget, parts: std::mem::take(&mut self.parts) })
    }

    fn tidy(&mut self, now: Instant) {
        while self.presented.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1)) {
            self.presented.pop_front();
        }
        while self.late_when.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            self.late_when.pop_front();
        }
    }

    /// The record, for `Request::Frames`.
    pub fn stats(&mut self, output: &str, refresh_mhz: u32, resting: bool) -> DisplayFrames {
        self.tidy(Instant::now());
        let mut sorted: Vec<Duration> = self.renders.iter().copied().collect();
        sorted.sort_unstable();
        let at = |p: usize| sorted.get((sorted.len() * p / 100).min(sorted.len().saturating_sub(1))).map_or(0, |d| d.as_micros() as u64);
        let frame = Duration::from_secs_f64(1000.0 / refresh_mhz.max(1) as f64);
        DisplayFrames {
            output: output.to_owned(),
            refresh_mhz,
            fps: self.presented.len() as f64,
            frames: self.frames,
            late: self.late,
            late_minute: self.late_when.len() as u32,
            render_min_us: sorted.first().map_or(0, |d| d.as_micros() as u64),
            render_p50_us: at(50),
            render_p95_us: at(95),
            render_max_us: sorted.last().map_or(0, |d| d.as_micros() as u64),
            budget_us: self.budget.max(Self::least(frame)).as_micros() as u64,
            delay_us: frame.saturating_sub(self.budget.max(Self::least(frame))).as_micros() as u64,
            cursor_plane: self.cursor_plane,
            scanout: self.scanout.clone(),
            resting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F60: Duration = Duration::from_micros(16_667);

    fn vblank(n: u64) -> Time<Monotonic> {
        Time::from(Duration::from_micros(1_000_000 + n * 16_667))
    }

    #[test]
    fn cheap_frames_keep_smithays_split() {
        let mut p = Pacing::default();
        for n in 0..50 {
            p.expect(vblank(n + 1));
            p.drawn(Duration::from_millis(1), &[], true);
            assert!(p.presented(vblank(n + 1), F60).is_none());
        }
        assert_eq!(p.delay(F60), F60 - F60.mul_f64(0.4));
    }

    #[test]
    fn costly_frames_start_sooner() {
        let mut p = Pacing::default();
        for n in 0..20 {
            p.expect(vblank(n + 1));
            p.drawn(Duration::from_millis(7), &[], true);
            p.presented(vblank(n + 1), F60);
        }
        // 7 ms × 1.25 + 0.5 ms kept: the draw starts 7.4 ms before the vblank.
        let delay = p.delay(F60);
        assert!(delay < Duration::from_micros(7_600), "{delay:?}");
        assert!(delay > Duration::from_micros(7_000), "{delay:?}");
    }

    #[test]
    fn a_drop_widens_the_budget_and_is_reported() {
        let mut p = Pacing::default();
        p.expect(vblank(1));
        p.drawn(Duration::from_millis(2), &[("draw", Duration::from_millis(2))], true);
        assert!(p.presented(vblank(1), F60).is_none());
        set_capture(true);
        p.expect(vblank(2));
        p.drawn(Duration::from_millis(2), &[("draw", Duration::from_millis(2))], true);
        // Presented at vblank 3: a frame late.
        let d = p.presented(vblank(3), F60).expect("a drop");
        set_capture(false);
        assert!(d.late >= F60 - Duration::from_micros(10), "{:?}", d.late);
        assert_eq!(d.parts_text(), "draw 2.0 ms");
        let s = p.stats("DP-1", 60_000, false);
        assert_eq!((s.frames, s.late, s.late_minute), (2, 1, 1));
        // The budget grew by a quarter frame over the floor.
        assert!(s.budget_us > 10_000, "{}", s.budget_us);
    }

    #[test]
    fn a_long_clean_run_narrows_it_again() {
        let mut p = Pacing::default();
        p.budget = F60;
        for n in 0..(SETTLE as u64 * 2) {
            p.expect(vblank(n + 1));
            p.drawn(Duration::from_millis(1), &[], true);
            p.presented(vblank(n + 1), F60);
        }
        assert!(p.budget < F60, "{:?}", p.budget);
        assert!(p.budget >= Pacing::least(F60));
    }

    #[test]
    fn a_frame_from_a_look_of_its_own_is_not_judged() {
        let mut p = Pacing::default();
        p.drawn(Duration::from_millis(1), &[], true);
        assert!(p.presented(vblank(9), F60).is_none());
        assert_eq!(p.stats("x", 60_000, true).late, 0);
    }
}
