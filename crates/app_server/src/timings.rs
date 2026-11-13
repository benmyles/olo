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
/// A part of the loop this long is noted always (`held`), to say what
/// made input late.
const HELD: Duration = Duration::from_millis(20);
/// Input this late is said in the log, with what held the loop.
const LATE: Duration = Duration::from_millis(100);

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
    /// The parts of what's being done now (`within`), by kind: how long
    /// each kind took in all and how many there were (which requests of
    /// a dispatch of clients' requests held the loop).
    inner: BTreeMap<String, (Duration, u32)>,
    /// The longest part of the loop since the last input event, of
    /// `HELD` or more, counted whether watched or not: what held the
    /// loop when input comes late (`input`).
    held: Option<(String, Duration)>,
    /// When late input was last said (a stall's queued events are all
    /// late: the first says it).
    late_said: Option<Instant>,
}

impl Timings {
    fn watching(&self) -> bool {
        self.watched_until.is_some_and(|t| Instant::now() < t)
    }

    /// One frame of `output` drawn, in `took`.
    pub fn frame(&mut self, output: &str, took: Duration) {
        self.note_held(took, || format!("drawing {output}"));
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
        self.note_held(took, || name.to_owned());
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
        // (One reading of the clock, as before: it says both.)
        let now = Instant::now();
        let took = now.saturating_duration_since(start);
        let held = took >= HELD && self.held.as_ref().is_none_or(|(_, t)| took > *t);
        let slowest = self.watched_until.is_some_and(|t| now < t) && self.slowest.as_ref().is_none_or(|(_, t)| took > *t);
        if held || slowest {
            let what = what();
            if held {
                self.held = Some((what.clone(), took));
            }
            if slowest {
                self.slowest = Some((what, took));
            }
        }
    }

    /// `took` of the loop, `what` it was: kept if it's the longest since
    /// the last input event.
    fn note_held(&mut self, took: Duration, what: impl FnOnce() -> String) {
        if took >= HELD && self.held.as_ref().is_none_or(|(_, t)| took > *t) {
            self.held = Some((what(), took));
        }
    }

    /// An input event (`kind`) handled, `late` after the kernel stamped
    /// it: said in the log when it's late enough to be felt (the pointer
    /// stood still meanwhile), with the longest thing the loop did since
    /// the last event, or that it did nothing long (then app_server
    /// wasn't given a processor, or the event came late to it).
    pub fn input(&mut self, kind: &str, late: Duration) {
        let held = self.held.take();
        if late < LATE || late > Duration::from_secs(60) {
            return;
        }
        let now = Instant::now();
        if self.late_said.is_some_and(|t| now.duration_since(t) < Duration::from_secs(2)) {
            return;
        }
        self.late_said = Some(now);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        match held {
            Some((what, took)) if took * 2 >= late => tracing::warn!("input ({kind}) came {:.0} ms late: the loop was held by {what} ({:.0} ms)", ms(late), ms(took)),
            Some((what, took)) => {
                tracing::warn!("input ({kind}) came {:.0} ms late; the loop's longest part meanwhile was {what} ({:.0} ms): app_server wasn't run, or the event came late to it", ms(late), ms(took))
            }
            None => tracing::warn!("input ({kind}) came {:.0} ms late, with nothing in the loop as long as {} ms: app_server wasn't run, or the event came late to it", ms(late), HELD.as_millis()),
        }
    }

    /// A part of something being done (a commit within clients'
    /// requests), the kind `what` it was, since `start`: the kind that
    /// took longest in all names it ([`Timings::handled_naming_parts`]).
    pub fn within(&mut self, what: impl FnOnce() -> String, start: Instant) {
        if !self.watching() {
            return;
        }
        let took = start.elapsed();
        let e = self.inner.entry(what()).or_default();
        e.0 += took;
        e.1 += 1;
    }

    /// [`Timings::handled`], naming the kind of part noted `within` it
    /// that took longest in all, when that was most of it ("clients'
    /// requests: a new window ×7 (12.0 ms)").
    pub fn handled_naming_parts(&mut self, what: &str, start: Instant) {
        let inner = std::mem::take(&mut self.inner);
        if !self.watching() {
            return;
        }
        let took = start.elapsed();
        let most = inner.into_iter().max_by_key(|(_, (t, _))| *t);
        self.handled(
            || match most {
                Some((part, (t, n))) if t * 2 > took => {
                    let times = if n > 1 { format!(" ×{n}") } else { String::new() };
                    format!("{what}: {part}{times} ({:.1} ms)", t.as_secs_f64() * 1000.0)
                }
                _ => what.to_owned(),
            },
            start,
        );
    }

    /// Keep counting for a minute, without the heartbeat (capturing
    /// drops: what held the loop, `take_slowest`).
    pub fn watch_quietly(&mut self) {
        if !self.watching() {
            self.slowest = None;
        }
        self.watched_until = Some(Instant::now() + WATCH);
    }

    /// The slowest thing done since the last take (`handled`).
    pub fn take_slowest(&mut self) -> Option<(String, Duration)> {
        self.slowest.take()
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
