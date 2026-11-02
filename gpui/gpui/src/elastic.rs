//! Olo: a scroll view's ends are elastic, as on a Mac. Fingers on a
//! touchpad pull the content past an end against a spring (the further,
//! the harder: Apple's rubber band, a third of the view at most), and it
//! springs back when they lift; the glide after them, reaching an end,
//! bounces off it as fast as it came. A wheel stops at the ends. GPUI's
//! scrolling (`div`'s `overflow_scroll`, `uniform_list`) keeps one for
//! each scroller, and a view that scrolls itself (the GPUI kit's editor)
//! keeps its own.

use std::time::Instant;

use crate::{Pixels, Point, ScrollKind, Size, TouchPhase, point, px};

/// How hard the rubber band pulls back (Apple's 0.55).
const STIFFNESS: f32 = 0.55;
/// How fast it springs back (a critically damped spring, per second): at
/// rest in about a third of a second.
const SPRING: f32 = 14.0;
/// A bounce goes at most this far into the view.
const MOST_BOUNCE: f32 = 0.25;

/// A scroller's stretch past its ends, on each axis (one with nothing
/// to scroll that way doesn't stretch) ([`Elastic::scroll`],
/// [`Elastic::offset`]).
#[derive(Clone, Debug, Default)]
pub struct Elastic {
    axes: [Axis; 2],
    /// Where its ends are (the offset goes from minus this to zero) and
    /// how big its view is, as last laid out.
    max: Size<Pixels>,
    view: Size<Pixels>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Axis {
    /// Fingers pulling past an end: how far they've gone (unstretched;
    /// positive past the start).
    pull: f32,
    /// Springing back or bouncing: from where (stretched), how fast
    /// (points a second), since when.
    spring: Option<(f32, f32, Instant)>,
    /// A glide bounced off an end: the rest of it is spent.
    bounced: bool,
    /// When the glide's last move came (for its speed).
    last: Option<Instant>,
}

impl Elastic {
    /// Its ends and view, as laid out this frame.
    pub fn set_extent(&mut self, max: Size<Pixels>, view: Size<Pixels>) {
        self.max = max;
        self.view = view;
    }

    /// Whether it's past an end, or on its way back.
    pub fn stretched(&self) -> bool {
        self.axes.iter().any(|a| a.pull != 0.0 || a.spring.is_some())
    }

    /// Scrolls `offset` (from minus `max` to zero, as GPUI's) by `delta`
    /// from a scroll of `kind` in `phase`; whether anything moved.
    pub fn scroll(&mut self, offset: &mut Point<Pixels>, delta: Point<Pixels>, kind: ScrollKind, phase: TouchPhase) -> bool {
        let now = Instant::now();
        let (max, view) = ([f32::from(self.max.width), f32::from(self.max.height)], [f32::from(self.view.width), f32::from(self.view.height)]);
        let mut at = [f32::from(offset.x), f32::from(offset.y)];
        let d = [f32::from(delta.x), f32::from(delta.y)];
        let mut moved = false;
        for i in 0..2 {
            let a = &mut self.axes[i];
            let (lo, hi) = (-max[i].max(0.0), 0.0);
            let was = (at[i], a.pull);
            // Nothing to scroll that way: nothing to stretch either (as a
            // Mac's; a pane around a list, or one that fits).
            let kind = if max[i] <= 0.0 && a.pull == 0.0 && a.spring.is_none() { ScrollKind::Wheel } else { kind };
            match kind {
                ScrollKind::Wheel => {
                    at[i] = (at[i] + d[i]).clamp(lo, hi);
                }
                ScrollKind::Fingers => {
                    if matches!(phase, TouchPhase::Ended) {
                        if a.pull != 0.0 {
                            a.spring = Some((rubber(a.pull, view[i]), 0.0, now));
                            a.pull = 0.0;
                            moved = true;
                        }
                        continue;
                    }
                    // Taking hold of it on its way back.
                    if let Some(s) = a.spring.take() {
                        a.pull = unrubber(spring_at(s, now).0, view[i]);
                    }
                    a.bounced = false;
                    let (o, p) = pull(at[i], a.pull, d[i], lo, hi);
                    at[i] = o;
                    a.pull = p;
                }
                ScrollKind::Glide => {
                    if matches!(phase, TouchPhase::Ended) {
                        a.bounced = false;
                        a.last = None;
                        if a.pull != 0.0 {
                            a.spring = Some((rubber(a.pull, view[i]), 0.0, now));
                            a.pull = 0.0;
                            moved = true;
                        }
                        continue;
                    }
                    let since = a.last.replace(now).map(|t| now.duration_since(t).as_secs_f32());
                    if a.bounced || d[i] == 0.0 {
                        continue;
                    }
                    let t = at[i] + d[i];
                    if (lo..=hi).contains(&t) {
                        at[i] = t;
                        continue;
                    }
                    // Off an end, as fast as it came (at most a quarter
                    // of the view deep).
                    at[i] = t.clamp(lo, hi);
                    let speed = d[i] / since.unwrap_or(1.0 / 120.0).clamp(0.004, 0.034);
                    let most = MOST_BOUNCE * view[i] * SPRING * std::f32::consts::E;
                    a.spring = Some((0.0, speed.clamp(-most, most), now));
                    a.bounced = true;
                    moved = true;
                }
            }
            moved |= (at[i], a.pull) != was;
        }
        *offset = point(px(at[0]), px(at[1]));
        moved
    }

    /// Where the content shows, from where it's scrolled to (`offset`,
    /// within its ends): moved past an end as far as it's stretched; and
    /// whether it's still moving (springing back: draw again next frame).
    pub fn offset(&mut self, offset: Point<Pixels>) -> (Point<Pixels>, bool) {
        let now = Instant::now();
        let view = [f32::from(self.view.width), f32::from(self.view.height)];
        let mut out = [f32::from(offset.x), f32::from(offset.y)];
        let mut moving = false;
        for (i, a) in self.axes.iter_mut().enumerate() {
            if a.pull != 0.0 {
                out[i] += rubber(a.pull, view[i]);
            } else if let Some(s) = a.spring {
                let (x, v) = spring_at(s, now);
                // At rest: under a quarter of a point, barely moving.
                if now.duration_since(s.2).as_secs_f32() > 1.0 / SPRING && x.abs() < 0.25 && v.abs() < 20.0 {
                    a.spring = None;
                } else {
                    out[i] += x;
                    moving = true;
                }
            }
        }
        (point(px(out[0]), px(out[1])), moving)
    }
}

/// Fingers moving `d` from `at` (within `lo..=hi`) with `pull` past an
/// end already: where it's scrolled to, and how far past an end.
fn pull(at: f32, pull: f32, d: f32, lo: f32, hi: f32) -> (f32, f32) {
    if pull != 0.0 {
        let p = pull + d;
        // Back within its ends: the rest scrolls.
        if p.signum() != pull.signum() || p == 0.0 {
            return ((at + p).clamp(lo, hi), 0.0);
        }
        return (at, p);
    }
    let t = at + d;
    if t > hi {
        (hi, t - hi)
    } else if t < lo {
        (lo, t - lo)
    } else {
        (t, 0.0)
    }
}

/// How far past an end fingers `x` past it show, in a view `d` long:
/// Apple's rubber band, never more than a third of the view.
fn rubber(x: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    let d = d / 3.0;
    x.signum() * (1.0 - 1.0 / (x.abs() * STIFFNESS / d + 1.0)) * d
}

/// [`rubber`] undone: how far fingers would have pulled to show `y`.
fn unrubber(y: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    let d = d / 3.0;
    let f = (y.abs() / d).min(0.999);
    y.signum() * d / STIFFNESS * (1.0 / (1.0 - f) - 1.0)
}

/// A critically damped spring from `x0` at `v0` points a second, at
/// `now`: where it is and how fast it's going.
fn spring_at((x0, v0, t0): (f32, f32, Instant), now: Instant) -> (f32, f32) {
    let t = now.duration_since(t0).as_secs_f32();
    let w = SPRING;
    let e = (-w * t).exp();
    let b = v0 + w * x0;
    (((x0 + b * t) * e), (b - w * (x0 + b * t)) * e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(max: f32, view: f32) -> Elastic {
        let mut e = Elastic::default();
        e.set_extent(Size { width: px(0.0), height: px(max) }, Size { width: px(400.0), height: px(view) });
        e
    }

    #[test]
    fn fingers_pull_past_an_end_and_it_springs_back() {
        let mut e = el(1000.0, 600.0);
        let mut o = point(px(0.0), px(0.0));
        // Down past the top (content pulled down): the offset stays at
        // the end, the content shows past it, less than pulled.
        e.scroll(&mut o, point(px(0.0), px(100.0)), ScrollKind::Fingers, TouchPhase::Started);
        assert_eq!(o.y, px(0.0));
        let (shown, _) = e.offset(o);
        assert!(shown.y > px(0.0) && shown.y < px(100.0), "{shown:?}");
        // Back the other way: undone first, then it scrolls.
        e.scroll(&mut o, point(px(0.0), px(-150.0)), ScrollKind::Fingers, TouchPhase::Moved);
        assert_eq!(o.y, px(-50.0));
        assert_eq!(e.offset(o).0.y, px(-50.0));
        // Past the bottom, lifted: it springs back.
        e.scroll(&mut o, point(px(0.0), px(-2000.0)), ScrollKind::Fingers, TouchPhase::Moved);
        assert_eq!(o.y, px(-1000.0));
        e.scroll(&mut o, point(px(0.0), px(0.0)), ScrollKind::Fingers, TouchPhase::Ended);
        let (shown, moving) = e.offset(o);
        assert!(moving && shown.y < px(-1000.0) && shown.y > px(-1200.0), "{shown:?}");
        // A third of the view at most.
        assert!(shown.y >= px(-1000.0 - 200.0));
    }

    #[test]
    fn a_glide_bounces_off_an_end_and_a_wheel_stops_there() {
        let mut e = el(500.0, 600.0);
        let mut o = point(px(0.0), px(-480.0));
        e.scroll(&mut o, point(px(0.0), px(-40.0)), ScrollKind::Glide, TouchPhase::Moved);
        assert_eq!(o.y, px(-500.0));
        assert!(e.stretched());
        // The rest of the glide is spent.
        e.scroll(&mut o, point(px(0.0), px(-30.0)), ScrollKind::Glide, TouchPhase::Moved);
        assert_eq!(o.y, px(-500.0));
        let mut w = el(500.0, 600.0);
        let mut o = point(px(0.0), px(-480.0));
        w.scroll(&mut o, point(px(0.0), px(-100.0)), ScrollKind::Wheel, TouchPhase::Moved);
        assert_eq!(o.y, px(-500.0));
        assert!(!w.stretched());
    }

    #[test]
    fn the_rubber_band_comes_undone() {
        for x in [1.0, 20.0, 300.0, 2000.0] {
            let y = rubber(x, 600.0);
            assert!((unrubber(y, 600.0) - x).abs() < x * 0.01 + 0.01, "{x}");
        }
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn nothing_to_scroll_nothing_to_stretch() {
        let mut e = Elastic::default();
        e.set_extent(Size { width: px(0.0), height: px(0.0) }, Size { width: px(400.0), height: px(300.0) });
        let mut o = point(px(0.0), px(0.0));
        e.scroll(&mut o, point(px(0.0), px(80.0)), ScrollKind::Fingers, TouchPhase::Moved);
        assert!(!e.stretched());
        assert_eq!(e.offset(o).0, o);
    }
}
