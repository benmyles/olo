//! A window put somewhere by Olo at the person's word glides there (Ben,
//! 2026-11-18: "windows should smoothly animate when they move not
//! jarringly just redraw somewhere else"): a place taken while it's
//! dragged, a drop into one, the window it swaps with (`snap`); a tiling
//! shortcut, Zoom (the key and the tab's box), Restore, Centre
//! (`glided`). Not what arranges many at once (auto-tiling, Tile All:
//! those show every window's new size in one frame, `rounds`), nor a
//! request over the socket that says where a window is to be (`Place`):
//! what asked reads the frame back.
//!
//! It's the window itself at each size on the way, drawn by its app as
//! when its edge is dragged (`tiling::follow`: asked a size when it has
//! answered the last, put where that size belongs when it draws it):
//! nothing is stretched, as Olo never resamples a picture. Each step is
//! where the glide has got to by the clock, so an app that draws slowly
//! takes fewer steps and arrives when a fast one does; a window that
//! only moves is moved each frame.
//!
//! The glide closes on its target as a critically damped spring does
//! (most of the way at once, then easing in: half way in 30 ms, 95% in
//! an eighth of a second, there in a little over a quarter), so a target
//! that moves (the pointer, as a window goes back under it out of a
//! place) is followed without a jump. Nothing ticks while nothing
//! glides: `glide_tick` is each frame's, and frames are asked for only
//! while a glide is on.
//!
//! A gliding window that something else places meanwhile is that
//! thing's: a layout or a drag that asks it a size (`ask_to_follow`) and
//! a zoom end the glide where it is; what needs it where it belongs
//! first (an edge taken in hand, a workspace put away) has it there at
//! once (`glide_finish`). Another frame given it (`set_frame`) is where
//! it glides to instead.

use std::time::Instant;

use smithay::utils::{IsAlive as _, Logical, Rectangle};

use super::{WindowElement, wm::ZoomedFrom};
use crate::state::{AnvilState, Backend};

type Rect = Rectangle<i32, Logical>;

/// How fast a glide closes in (per second): what's left shrinks by
/// e^(-RATE·t).
const RATE: f64 = 24.0;
/// The longest a step is taken to be (a frame late, the glide doesn't
/// leap).
const STEP_MOST: f64 = 0.05;
/// Nearer than this (points, every edge), it's there.
const THERE: f64 = 1.0;

/// A window on its way to a frame.
#[derive(Debug)]
pub struct Glide {
    window: WindowElement,
    /// Where it is on the way: x, y, width, height.
    at: [f64; 4],
    to: Rect,
    /// When `at` was worked out.
    last: Instant,
    /// The frame last asked of it.
    asked: Rect,
    /// Zoomed once it's there (`zoom_window`: its frame is the zoom's).
    zoom: bool,
}

fn edges(r: Rect) -> [f64; 4] {
    [r.loc.x as f64, r.loc.y as f64, r.size.w as f64, r.size.h as f64]
}

fn rect(e: [f64; 4]) -> Rect {
    Rect::new((e[0].round() as i32, e[1].round() as i32).into(), (e[2].round().max(1.0) as i32, e[3].round().max(1.0) as i32).into())
}

/// `at` after `dt` seconds on its way to `to`, and whether it's there.
fn step(at: [f64; 4], to: [f64; 4], dt: f64) -> ([f64; 4], bool) {
    let k = 1.0 - (-RATE * dt.clamp(0.0, STEP_MOST)).exp();
    let mut out = at;
    for i in 0..4 {
        out[i] += (to[i] - at[i]) * k;
    }
    if (0..4).all(|i| (to[i] - out[i]).abs() < THERE) { (to, true) } else { (out, false) }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Whether `window` is on its way somewhere.
    pub fn gliding(&self, window: &WindowElement) -> bool {
        self.glides.iter().any(|g| g.window == *window)
    }

    /// What `f` puts somewhere (`set_frame`) glides there.
    pub fn glided<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = std::mem::replace(&mut self.glide_frames, true);
        let r = f(self);
        self.glide_frames = was;
        r
    }

    /// `window`'s frame is to be `to`: it glides there from where it is
    /// (from where it has got to, on its way somewhere else). One as good
    /// as there already has nothing to glide.
    pub fn glide_to(&mut self, window: &WindowElement, to: Rect) {
        if let Some(g) = self.glides.iter_mut().find(|g| g.window == *window) {
            (g.to, g.zoom) = (to, false);
            return;
        }
        let Some(from) = self.space.element_geometry(window) else { return };
        let (at, target) = (edges(from), edges(to));
        if (0..4).all(|i| (target[i] - at[i]).abs() < THERE) {
            return;
        }
        self.glides.push(Glide { window: window.clone(), at, to, last: Instant::now(), asked: from, zoom: false });
        self.backend_data.wake(None);
    }

    /// `window`, gliding back under the pointer, is headed for `at` now
    /// (its top left), at the size it was headed for.
    pub fn glide_under(&mut self, window: &WindowElement, at: smithay::utils::Point<i32, Logical>) {
        if let Some(g) = self.glides.iter_mut().find(|g| g.window == *window) {
            g.to.loc = at;
        }
    }

    /// `window`, gliding to a display's whole area, is zoomed once it's
    /// there (now, if it isn't gliding).
    pub fn glide_then_zoom(&mut self, window: &WindowElement) {
        match self.glides.iter_mut().find(|g| g.window == *window) {
            Some(g) => g.zoom = true,
            None => self.zoom_there(window),
        }
    }

    /// Where `window` is gliding to, and whether it's zoomed once there.
    pub fn glide_goal(&self, window: &WindowElement) -> Option<(Rect, bool)> {
        self.glides.iter().find(|g| g.window == *window).map(|g| (g.to, g.zoom))
    }

    /// `window` zoomed where it is (it has glided into the zoom's frame),
    /// what it unzooms to left as it was: `zoom_window` takes that to be
    /// where the window is now, which is the zoom's own frame here.
    pub(super) fn zoom_there(&mut self, window: &WindowElement) {
        let from = window.user_data().get::<ZoomedFrom>().and_then(|z| *z.0.borrow());
        self.zoom_window(window);
        if let Some(z) = window.user_data().get::<ZoomedFrom>() {
            *z.0.borrow_mut() = from;
        }
    }

    /// `window` is something else's to place now: on no way of its own
    /// any more.
    pub fn glide_stop(&mut self, window: &WindowElement) {
        if !self.glides.is_empty() {
            self.glides.retain(|g| g.window != *window);
        }
    }

    /// `window` is where it was gliding to, at once: what's about to
    /// take hold of it or put it away (an edge dragged, its workspace
    /// left) finds it where it belongs.
    pub fn glide_finish(&mut self, window: &WindowElement) {
        let Some(i) = self.glides.iter().position(|g| g.window == *window) else { return };
        let g = self.glides.remove(i);
        self.set_frame(window, g.to, false);
        if g.zoom {
            self.zoom_there(window);
        }
    }

    /// Before each frame: every gliding window a step on (asked the size
    /// it has there, or moved, if its size is that already), and those
    /// that have arrived done with.
    pub fn glide_tick(&mut self) {
        if self.glides.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut glides = std::mem::take(&mut self.glides);
        let mut zoom = Vec::new();
        // (What a step asks of a window is the glide's own asking.)
        self.glide_stepping = true;
        glides.retain_mut(|g| {
            if !g.window.alive() {
                return false;
            }
            let Some(drawn) = self.space.element_geometry(&g.window) else { return false };
            let (at, there) = step(g.at, edges(g.to), now.duration_since(g.last).as_secs_f64());
            (g.at, g.last) = (at, now);
            let want = if there { g.to } else { rect(at) };
            // (Told it's being resized at each size but the last, as a
            // window following a drag is: it keeps room to grow into,
            // not new buffers at every step.)
            self.glide_resizing = !there;
            if want.size != g.asked.size || (there && drawn.size != want.size) {
                // Another size: asked of it, and put where that size
                // belongs when it draws it.
                self.follow(&g.window, g.asked, want);
            } else if there {
                // There, at the size last asked: put there, and told it's
                // resized no longer.
                self.follow(&g.window, want, want);
                if drawn.loc != want.loc {
                    self.space.relocate_element(&g.window, want.loc);
                }
            } else if drawn.size == want.size {
                if drawn.loc != want.loc {
                    self.unfollow(&g.window);
                    self.space.relocate_element(&g.window, want.loc);
                }
            } else if want.loc != g.asked.loc {
                // (Its size asked and not yet drawn, its place moved on:
                // what it draws goes where the glide is by now.)
                self.follow(&g.window, g.asked, want);
            }
            self.glide_resizing = false;
            g.asked = want;
            if there && g.zoom {
                zoom.push(g.window.clone());
            }
            !there
        });
        self.glide_stepping = false;
        // (Any begun meanwhile, by what a step set off, after them.)
        glides.append(&mut self.glides);
        self.glides = glides;
        for w in zoom {
            self.zoom_there(&w);
        }
        self.backend_data.wake(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glide closes most of the way at once and eases in, never past
    /// its target; the same in few long steps as in many short ones; and
    /// is there in under a third of a second.
    #[test]
    fn a_glide_eases_in_and_arrives() {
        let (from, to) = ([100.0, 100.0, 800.0, 600.0], [8.0, 8.0, 628.0, 784.0]);
        let run = |dt: f64| {
            let (mut at, mut t, mut steps) = (from, 0.0, Vec::new());
            loop {
                let (next, there) = step(at, to, dt);
                for i in 0..4 {
                    let (lo, hi) = (from[i].min(to[i]), from[i].max(to[i]));
                    assert!(next[i] >= lo - 1e-9 && next[i] <= hi + 1e-9, "past its target: {next:?}");
                    assert!((to[i] - next[i]).abs() <= (to[i] - at[i]).abs() + 1e-9, "away from its target");
                }
                (at, t) = (next, t + dt);
                steps.push((t, at));
                if there {
                    return steps;
                }
                assert!(t < 1.0, "never there");
            }
        };
        let (fast, slow) = (run(1.0 / 120.0), run(1.0 / 30.0));
        let (fast_t, slow_t) = (fast.last().unwrap().0, slow.last().unwrap().0);
        assert!(fast_t < 0.33 && slow_t < 0.36, "{fast_t} {slow_t}");
        assert!((fast_t - slow_t).abs() < 0.04, "a slow app arrives when a fast one does: {fast_t} {slow_t}");
        // Half way within 40 ms, 95% within 150.
        let left = |steps: &[(f64, [f64; 4])], at: f64| steps.iter().find(|(t, _)| *t >= at).map(|(_, e)| (to[0] - e[0]).abs() / (to[0] - from[0]).abs()).unwrap_or(0.0);
        assert!(left(&fast, 0.04) < 0.5 && left(&fast, 0.15) < 0.05, "{} {}", left(&fast, 0.04), left(&fast, 0.15));
        // A frame long late doesn't leap: no more than a step's most.
        let (late, _) = step(from, to, 0.5);
        let (most, _) = step(from, to, STEP_MOST);
        assert_eq!(late, most);
    }
}
