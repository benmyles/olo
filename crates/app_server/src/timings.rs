//! How app_server keeps up (`Request::Timings`), for tools/dev/perf-fuzz:
//! each display's frames, the slowest thing it did (a request, an input
//! event, clients' requests, the tick), and how late the event loop runs. Asking starts a heartbeat every few milliseconds:
//! a beat comes late by however long something held the loop (a slow
//! handler, a blocking call, a long frame), so nothing has to be timed
//! one by one to be caught. It stops a minute after the last ask, so
//! nothing wakes app_server for it otherwise.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use olo_protocol::{FrameTimings, PhaseTimings, Timings as Answer};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::state::{AnvilState, Backend};

/// How often the heartbeat beats, while watched.
const BEAT: Duration = Duration::from_millis(4);
/// How long it keeps beating after an ask.
const WATCH: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct Timings {
    /// Until when someone is watching (the heartbeat runs till then).
    watched_until: Option<Instant>,
    /// The heartbeat's timer is in the loop.
    beating: bool,
    frames: BTreeMap<String, FrameTimings>,
    beats: u32,
    latest_beat: Duration,
    slowest: Option<(String, Duration)>,
    phases: BTreeMap<&'static str, PhaseTimings>,
}

impl Timings {
    fn watching(&self) -> bool {
        self.watched_until.is_some_and(|t| Instant::now() < t)
    }

    /// One frame of `output` drawn, in `took`.
    pub fn frame(&mut self, output: &str, took: Duration) {
        if !self.watching() {
            return;
        }
        let f = self.frames.entry(output.to_owned()).or_insert_with(|| FrameTimings { output: output.to_owned(), ..Default::default() });
        let us = took.as_micros() as u64;
        f.frames += 1;
        f.slowest_us = f.slowest_us.max(us);
        f.total_us += us;
    }

    /// Part `name` of the loop done, since `start` (`Answer::phases`).
    pub fn phase(&mut self, name: &'static str, start: Instant) {
        self.add(name, start.elapsed());
    }

    /// Part `name` of the loop done, in `took`.
    pub fn add(&mut self, name: &'static str, took: Duration) {
        if !self.watching() {
            return;
        }
        let us = took.as_micros() as u64;
        let p = self.phases.entry(name).or_insert_with(|| PhaseTimings { name: name.to_owned(), ..Default::default() });
        p.count += 1;
        p.total_us += us;
        p.slowest_us = p.slowest_us.max(us);
    }

    /// Something done (a request, an input event, clients' requests, the
    /// tick), `what` it was, since `start`.
    pub fn handled(&mut self, what: impl FnOnce() -> String, start: Instant) {
        if !self.watching() {
            return;
        }
        let took = start.elapsed();
        if self.slowest.as_ref().is_none_or(|(_, t)| took > *t) {
            self.slowest = Some((what(), took));
        }
    }

    fn take(&mut self) -> Answer {
        let answer = Answer {
            outputs: std::mem::take(&mut self.frames).into_values().collect(),
            beats: self.beats,
            latest_beat_us: self.latest_beat.as_micros() as u64,
            slowest: self.slowest.take().map(|(what, t)| (what, t.as_micros() as u64)),
            phases: std::mem::take(&mut self.phases).into_values().collect(),
        };
        self.beats = 0;
        self.latest_beat = Duration::ZERO;
        answer
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// `Request::Timings`: what was counted since the last ask; and keep
    /// counting (the first ask starts the heartbeat).
    pub fn take_timings(&mut self, quiet: bool) -> Answer {
        if !self.timings.watching() {
            // Nothing counted while nobody watched.
            self.timings = Timings { beating: self.timings.beating, ..Default::default() };
        }
        self.timings.watched_until = Some(Instant::now() + WATCH);
        if !self.timings.beating && !quiet {
            self.timings.beating = true;
            let _ = self.handle.insert_source(Timer::from_duration(BEAT), |due, _, state| {
                let t = &mut state.timings;
                if !t.watching() {
                    t.beating = false;
                    return TimeoutAction::Drop;
                }
                t.beats += 1;
                t.latest_beat = t.latest_beat.max(Instant::now().saturating_duration_since(due));
                TimeoutAction::ToDuration(BEAT)
            });
        }
        self.timings.take()
    }
}
