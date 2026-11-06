//! Zoom in screen recordings (CleanShot X's auto zoom): a display's
//! captures drawn magnified around what's being done, gliding in and out,
//! the screen itself as it is. Following (`Request::SetShowing`'s `zoom`,
//! the recorder's `--zoom-follow`): a click zooms in around it, the view
//! pans to keep the pointer in it, and clicks and keys hold it; three
//! seconds without either and it zooms out. An agent zooms to a part of
//! the screen itself (`Request::ZoomTo`, `olo-screen zoom-video`), and
//! back out. Applied by the GPU as the capture is drawn (as a mirror is
//! fitted: `render::frame`), so it costs nothing to the recorder.

use olo_sync::Mutex;
use std::time::{Duration, Instant};

use smithay::utils::{Logical, Point, Size};

/// How long a following zoom holds after the last click or key.
pub const HOLD: Duration = Duration::from_secs(3);
/// How fast the view glides: a critically damped spring's rate (per
/// second); a zoom in or out 95% done in 0.68 s.
const RATE: f64 = 7.0;
/// The pointer is kept this far (a share of the view) from its edges.
const MARGIN: f64 = 0.15;
/// The most a view is magnified.
const MOST: f64 = 8.0;

/// A view: its centre (points of the display) and magnification.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    x: f64,
    y: f64,
    k: f64,
}

#[derive(Debug)]
struct Display {
    name: String,
    /// Where the view is going.
    to: View,
    /// Where it is and how fast it's going, as of `when`.
    at: View,
    speed: View,
    when: Instant,
    /// Held there by an agent (`ZoomTo`), not following.
    fixed: bool,
    /// Following: zoomed in till then.
    hold: Option<Instant>,
}

struct Zoom {
    /// Following clicks, magnifying this much.
    follow: Option<f64>,
    displays: Vec<Display>,
}

static ZOOM: Mutex<Zoom> = Mutex::new(Zoom { follow: None, displays: Vec::new() });

/// Follow clicks, magnifying `k` times (None: not; every display's
/// captures as they are at once, as a recording ends).
pub fn set_follow(k: Option<f64>) {
    let mut z = ZOOM.lock().unwrap();
    z.follow = k.map(|k| k.clamp(1.0, MOST)).filter(|&k| k > 1.0);
    if z.follow.is_none() {
        z.displays.clear();
    }
}

/// The display's entry, made at the whole screen if new.
fn display<'a>(z: &'a mut Zoom, name: &str, screen: Size<f64, Logical>) -> &'a mut Display {
    if let Some(i) = z.displays.iter().position(|d| d.name == name) {
        return &mut z.displays[i];
    }
    let whole = View { x: screen.w / 2.0, y: screen.h / 2.0, k: 1.0 };
    z.displays.push(Display { name: name.to_owned(), to: whole, at: whole, speed: View { x: 0.0, y: 0.0, k: 0.0 }, when: Instant::now(), fixed: false, hold: None });
    z.displays.last_mut().unwrap()
}

/// A click at `at` (points from display `name`'s top left): following,
/// the view zooms in around it (or, in already, holds). True: following
/// (the caller wakes the displays once the hold is up).
pub fn click(name: &str, at: Point<f64, Logical>, screen: Size<f64, Logical>) -> bool {
    let mut z = ZOOM.lock().unwrap();
    let Some(k) = z.follow else { return false };
    let d = display(&mut z, name, screen);
    if d.fixed {
        return true;
    }
    d.hold = Some(Instant::now() + HOLD);
    if d.to.k <= 1.0 {
        d.to = View { x: at.x, y: at.y, k };
    }
    true
}

/// A key pressed: following views zoomed in hold on.
pub fn key() {
    let mut z = ZOOM.lock().unwrap();
    if z.follow.is_none() {
        return;
    }
    let until = Instant::now() + HOLD;
    for d in z.displays.iter_mut().filter(|d| !d.fixed && d.to.k > 1.0) {
        d.hold = Some(until);
    }
}

/// How long till the last following hold is up (None: none is held).
pub fn hold_left() -> Option<Duration> {
    let z = ZOOM.lock().unwrap();
    let now = Instant::now();
    z.displays.iter().filter_map(|d| d.hold).filter(|&h| h > now).max().map(|h| h - now)
}

/// Display `name`'s view to `rect` (points: x, y, w, h), magnified as
/// much as shows all of it; None: back out (following again, if it was).
pub fn to(name: &str, rect: Option<[f64; 4]>, screen: Size<f64, Logical>) {
    let mut z = ZOOM.lock().unwrap();
    let d = display(&mut z, name, screen);
    match rect {
        Some([x, y, w, h]) => {
            let k = (screen.w / w.max(1.0)).min(screen.h / h.max(1.0)).clamp(1.0, MOST);
            d.to = View { x: x + w / 2.0, y: y + h / 2.0, k };
            d.fixed = true;
        }
        None => {
            d.to = View { x: screen.w / 2.0, y: screen.h / 2.0, k: 1.0 };
            d.fixed = false;
            d.hold = None;
        }
    }
}

/// How display `name`'s captures are drawn now: magnified `k` times
/// with the view's top left at `corner` (points), or None (as it is).
/// The glide moved on to now; following, the view pans to keep
/// `pointer` (points from the display's corner) in it, and zooms out
/// once the hold is up.
pub fn view(name: &str, screen: Size<f64, Logical>, pointer: Option<Point<f64, Logical>>) -> Option<(f64, Point<f64, Logical>)> {
    let mut z = ZOOM.lock().unwrap();
    let d = z.displays.iter_mut().find(|d| d.name == name)?;
    let now = Instant::now();
    if !d.fixed {
        if d.hold.is_some_and(|h| now >= h) {
            d.hold = None;
            d.to = View { x: screen.w / 2.0, y: screen.h / 2.0, k: 1.0 };
        }
        if let Some(p) = pointer.filter(|_| d.to.k > 1.0) {
            d.to = pan(d.to, p, screen);
        }
    }
    step(d, now);
    let v = clamped(d.at, screen);
    (v.k > 1.0005).then(|| (v.k, corner(v, screen)))
}

/// Some view is gliding (or about to zoom out): the displays draw on.
pub fn moving() -> bool {
    let z = ZOOM.lock().unwrap();
    let now = Instant::now();
    z.displays.iter().any(|d| d.at != d.to || d.hold.is_some_and(|h| now >= h))
}

/// `v` moved so `p` is inside it, `MARGIN` from its edges.
fn pan(v: View, p: Point<f64, Logical>, screen: Size<f64, Logical>) -> View {
    let (w, h) = (screen.w / v.k, screen.h / v.k);
    let v = clamped(v, screen);
    let follow = |c: f64, p: f64, side: f64| {
        let half = side / 2.0 - side * MARGIN;
        if p < c - half {
            p + half
        } else if p > c + half {
            p - half
        } else {
            c
        }
    };
    clamped(View { x: follow(v.x, p.x, w), y: follow(v.y, p.y, h), k: v.k }, screen)
}

/// `v` with its view inside the screen.
fn clamped(v: View, screen: Size<f64, Logical>) -> View {
    let k = v.k.clamp(1.0, MOST);
    let (hw, hh) = (screen.w / k / 2.0, screen.h / k / 2.0);
    View { x: v.x.clamp(hw, screen.w - hw), y: v.y.clamp(hh, screen.h - hh), k }
}

/// The view's top left.
fn corner(v: View, screen: Size<f64, Logical>) -> Point<f64, Logical> {
    (v.x - screen.w / v.k / 2.0, v.y - screen.h / v.k / 2.0).into()
}

/// The glide moved on to `now` (exact for any time step; arrived within
/// a hundredth of a point and of a magnification: there).
fn step(d: &mut Display, now: Instant) {
    let dt = now.duration_since(d.when).as_secs_f64();
    d.when = now;
    let e = (-RATE * dt).exp();
    let axis = |at: f64, to: f64, v: f64| {
        let y = at - to;
        let c = v + RATE * y;
        (to + (y + c * dt) * e, (v - RATE * c * dt) * e)
    };
    let (x, vx) = axis(d.at.x, d.to.x, d.speed.x);
    let (y, vy) = axis(d.at.y, d.to.y, d.speed.y);
    let (k, vk) = axis(d.at.k, d.to.k, d.speed.k);
    if (x - d.to.x).abs() < 0.01 && (y - d.to.y).abs() < 0.01 && (k - d.to.k).abs() < 0.0005 && vx.abs() + vy.abs() < 0.5 && vk.abs() < 0.01 {
        d.at = d.to;
        d.speed = View { x: 0.0, y: 0.0, k: 0.0 };
    } else {
        d.at = View { x, y, k };
        d.speed = View { x: vx, y: vy, k: vk };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Size<f64, Logical> {
        (1280.0, 800.0).into()
    }

    fn after(name: &str, secs: f64) -> Option<(f64, Point<f64, Logical>)> {
        {
            let mut z = ZOOM.lock().unwrap();
            for d in z.displays.iter_mut().filter(|d| d.name == name) {
                d.when -= Duration::from_secs_f64(secs);
                if let Some(h) = d.hold.as_mut() {
                    *h -= Duration::from_secs_f64(secs);
                }
            }
        }
        view(name, screen(), None)
    }

    // (One test: the state is shared.)
    #[test]
    fn follows_clicks_holds_and_zooms_out() {
        set_follow(Some(2.0));
        // Nothing until a click.
        assert_eq!(view("A", screen(), None), None);
        assert!(click("A", (100.0, 100.0).into(), screen()));
        // Gliding in, then in: twice, kept on the screen (the click near
        // its corner: the view's corner at the screen's).
        let (k, _) = after("A", 0.2).unwrap();
        assert!(k > 1.2 && k < 2.0, "{k}");
        let (k, at) = after("A", 1.0).unwrap();
        assert!((k - 2.0).abs() < 0.005, "{k}");
        assert!(at.x.abs() < 0.01 && at.y.abs() < 0.01, "{at:?}");
        // The pointer far off: the view pans to it.
        let (_, at) = {
            view("A", screen(), Some((900.0, 500.0).into()));
            after("A", 1.5).unwrap()
        };
        assert!(at.x > 200.0 && at.y > 100.0, "{at:?}");
        // A key holds it; the hold up, it zooms out, and stops moving.
        key();
        assert!(after("A", 2.0).is_some(), "held");
        assert!(hold_left().is_some_and(|l| l > Duration::from_millis(900)));
        ZOOM.lock().unwrap().displays[0].hold = Some(Instant::now());
        assert!(moving());
        assert!(view("A", screen(), None).is_some());
        assert!(after("A", 0.2).is_some_and(|(k, _)| k > 1.0 && k < 2.0));
        assert_eq!(after("A", 2.0), None);
        assert!(!moving());
        // An agent's zoom: to a rect, held; out again.
        to("A", Some([640.0, 400.0, 320.0, 200.0]), screen());
        let (k, at) = after("A", 2.0).unwrap();
        assert!((k - 4.0).abs() < 0.001 && (at.x - 640.0).abs() < 0.1 && (at.y - 400.0).abs() < 0.1, "{k} {at:?}");
        assert!(after("A", 10.0).is_some(), "held");
        to("A", None, screen());
        assert_eq!(after("A", 3.0), None);
        // Off: nothing zoomed.
        set_follow(None);
        assert!(!click("A", (1.0, 1.0).into(), screen()));
    }
}
