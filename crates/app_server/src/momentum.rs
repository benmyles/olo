//! Momentum scrolling, as on a Mac: a two-finger scroll on the touchpad
//! that ends while the fingers are still moving keeps going after they
//! lift, slowing down like a sheet sliding to rest. Fingers back on the
//! touchpad (a hold, a move, a click) or another scroll stop it at once.
//!
//! libinput reports the lift as a scroll of 0 ("axis stop"). That stop is
//! held back until the glide ends, so clients see one continuous finger
//! scroll and toolkits with kinetic scrolling of their own (GTK) don't
//! add theirs on top. `pace.rs` sends it, a frame's worth a frame.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back the fingers' speed is measured when they lift.
const WINDOW: Duration = Duration::from_millis(80);
/// Fingers that stopped this long before lifting were resting: no glide.
const RESTED: Duration = Duration::from_millis(50);
/// The slowest flick that glides, in pixels per millisecond.
const MIN_START: f64 = 0.3;
/// The glide ends below this speed.
const MIN_SPEED: f64 = 0.015;
/// Speed kept per millisecond (macOS's "normal" deceleration).
const DECAY: f64 = 0.997;
/// A touchpad's usual report interval: the shortest time a flick's speed
/// is measured over.
pub const STEP: Duration = Duration::from_millis(8);

#[derive(Debug)]
pub struct Momentum {
    samples: VecDeque<(Instant, f64, f64)>,
    glide: Option<Glide>,
    /// Bumped by every glide, so a stale timer knows to stop.
    pub generation: u64,
    /// How far a glide goes, 1.0 as macOS's normal (Preferences ▸
    /// Touchpad); 0 turns momentum off.
    pub length: f64,
}

impl Default for Momentum {
    fn default() -> Self {
        Momentum { samples: VecDeque::new(), glide: None, generation: 0, length: 1.0 }
    }
}

#[derive(Debug)]
struct Glide {
    vx: f64,
    vy: f64,
    last: Instant,
}

impl Momentum {
    /// The fingers scrolled by (dx, dy). Ends any glide.
    pub fn scrolled(&mut self, now: Instant, dx: f64, dy: f64) {
        self.glide = None;
        self.samples.push_back((now, dx, dy));
        while self.samples.front().is_some_and(|s| now.duration_since(s.0) > WINDOW) {
            self.samples.pop_front();
        }
    }

    /// The fingers lifted: true if a glide begins (then hold back the
    /// stop and call [`Momentum::step`] every [`STEP`]).
    pub fn lifted(&mut self, now: Instant) -> bool {
        let samples = std::mem::take(&mut self.samples);
        if self.length <= 0.0 {
            return false;
        }
        let Some(&(first, ..)) = samples.iter().find(|s| now.duration_since(s.0) <= WINDOW) else { return false };
        let Some(&(last, ..)) = samples.back() else { return false };
        if now.duration_since(last) > RESTED {
            return false;
        }
        // Each move is how far the fingers went since the report before
        // it: the first in the window went before it began, so the speed
        // is the rest over the time from it to the last. One move alone
        // is taken to have taken a usual report's time.
        let recent: Vec<_> = samples.iter().filter(|s| s.0 >= first).collect();
        let (sx, sy, ms) = if recent.len() > 1 {
            let (x, y) = recent[1..].iter().fold((0.0, 0.0), |(x, y), s| (x + s.1, y + s.2));
            (x, y, (last.duration_since(first).as_secs_f64() * 1000.0).max(1.0))
        } else {
            (recent[0].1, recent[0].2, STEP.as_secs_f64() * 1000.0)
        };
        let (vx, vy) = (sx / ms, sy / ms);
        if vx.hypot(vy) < MIN_START {
            return false;
        }
        self.generation += 1;
        self.glide = Some(Glide { vx, vy, last: now });
        true
    }

    /// Glide from `t`, where the fingers left off, rather than from the
    /// lift (`pace.rs` sends the fingers' scroll a little late, and the
    /// glide goes on from the end of it).
    pub fn glide_from(&mut self, t: Instant) {
        if let Some(g) = &mut self.glide {
            g.last = t;
        }
    }

    /// The glide's move since the last step, or None when it has ended
    /// (then send the stop).
    pub fn step(&mut self, now: Instant) -> Option<(f64, f64)> {
        let g = self.glide.as_mut()?;
        let ms = now.duration_since(g.last).as_secs_f64() * 1000.0;
        g.last = now;
        // Distance is the integral of v·decay^t over the step; a glide
        // goes v0 / -ln(decay), so `length` scales the log.
        let decay = DECAY.powf(1.0 / self.length.max(0.1));
        let keep = decay.powf(ms);
        let travel = (1.0 - keep) / -decay.ln();
        let (dx, dy) = (g.vx * travel, g.vy * travel);
        g.vx *= keep;
        g.vy *= keep;
        if g.vx.hypot(g.vy) < MIN_SPEED {
            self.glide = None;
            return None;
        }
        Some((dx, dy))
    }

    /// Stop now; true if it was gliding (send the stop).
    pub fn cancel(&mut self) -> bool {
        self.samples.clear();
        self.glide.take().is_some()
    }

    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scroll `per_step` pixels every 8 ms for `steps`, from `start`.
    fn flick(m: &mut Momentum, start: Instant, per_step: f64, steps: u32) -> Instant {
        let mut t = start;
        for _ in 0..steps {
            t += STEP;
            m.scrolled(t, 0.0, per_step);
        }
        t
    }

    #[test]
    fn a_flick_glides_and_slows_to_rest() {
        let mut m = Momentum::default();
        let t0 = Instant::now();
        let t = flick(&mut m, t0, 16.0, 10); // 2 px/ms
        assert!(m.lifted(t + STEP));
        let mut t = t + STEP;
        let (mut moved, mut last, mut steps) = (0.0, f64::MAX, 0);
        while let Some((dx, dy)) = m.step({
            t += STEP;
            t
        }) {
            assert_eq!(dx, 0.0);
            assert!(dy > 0.0 && dy <= last, "slows down");
            last = dy;
            moved += dy;
            steps += 1;
        }
        // About v0 / -ln(DECAY) ≈ 666 px, over a couple of seconds.
        assert!((550.0..700.0).contains(&moved), "{moved}");
        assert!((150..400).contains(&steps), "{steps}");
        assert!(!m.gliding());
    }

    #[test]
    fn length_scales_the_glide() {
        let glide = |length: f64| {
            let mut m = Momentum { length, ..Default::default() };
            let t = flick(&mut m, Instant::now(), 12.0, 10);
            if !m.lifted(t) {
                return 0.0;
            }
            let (mut total, mut t) = (0.0, t);
            loop {
                t += STEP;
                match m.step(t) {
                    Some((_, dy)) => total += dy,
                    None => return total,
                }
            }
        };
        let (normal, long) = (glide(1.0), glide(2.0));
        assert!(long > 1.8 * normal && long < 2.2 * normal, "{normal} {long}");
        assert_eq!(glide(0.0), 0.0);
    }

    #[test]
    fn slow_or_resting_fingers_dont_glide() {
        let mut m = Momentum::default();
        let t0 = Instant::now();
        let t = flick(&mut m, t0, 1.0, 10); // 0.125 px/ms
        assert!(!m.lifted(t + STEP));
        // Fast, but held still before lifting.
        let t = flick(&mut m, t + Duration::from_secs(1), 16.0, 10);
        assert!(!m.lifted(t + Duration::from_millis(120)));
    }

    #[test]
    fn touching_again_stops_it() {
        let mut m = Momentum::default();
        let t = flick(&mut m, Instant::now(), -20.0, 10);
        assert!(m.lifted(t + STEP));
        assert!(m.step(t + STEP * 2).is_some_and(|(_, dy)| dy < 0.0), "keeps the direction");
        assert!(m.cancel());
        assert!(m.step(t + STEP * 3).is_none());
        assert!(!m.cancel());
    }
}
