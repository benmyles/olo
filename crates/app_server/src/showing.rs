//! Clicks and keys shown on screen, for screen recordings and demos
//! (CleanShot X's "show clicks and keystrokes"): a ring spreading from
//! where a button is pressed, and the chord just pressed (⌘⇧4, written as
//! the keyboard profile writes it) on a key-cap panel low on the screen.
//! Only chords (with ⌃, ⌥ or ⌘) and a few keys alone (Return, Escape,
//! Tab, Delete, the arrows) are shown, never typing: a password typed
//! while recording shows nothing. Off unless asked (`Request::SetShowing`:
//! the recorder turns it on for a recording that asks, and off after).
//! Drawn over everything but the pointer, so captures have it (see-
//! through: no opaque region); the displays stay awake only while
//! something is fading.

use olo_sync::Mutex;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::{Buffer, Logical, Point, Size, Transform},
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke};

use crate::{render::Exact, text, theme};

static CLICKS: AtomicBool = AtomicBool::new(false);
static KEYS: AtomicBool = AtomicBool::new(false);

/// How long a ring spreads and fades.
const RING: Duration = Duration::from_millis(450);
/// How long a chord stays up (its last fifth fading).
const CHORD: Duration = Duration::from_millis(1600);
/// The ring's size (points) at its start and end.
const RING_FROM: f32 = 6.0;
const RING_TO: f32 = 26.0;

struct State {
    /// Rings: the display, where (in its points), since when.
    rings: Vec<(String, Point<f64, Logical>, Instant)>,
    /// The chord up: the display, its label, since when, and its picture
    /// (made once, for the display's scale).
    chord: Option<(String, String, Instant, Option<(f64, MemoryRenderBuffer, (i32, i32))>)>,
}

static STATE: Mutex<State> = Mutex::new(State { rings: Vec::new(), chord: None });

/// A recording's countdown: the display, the number, and its picture
/// (made once, for the display's scale).
type Count = (String, u32, Option<(f64, MemoryRenderBuffer, (i32, i32))>);

static COUNTDOWN: Mutex<Option<Count>> = Mutex::new(None);

/// A recording's countdown at `n` on `output` (0: done, taken down). The
/// recorder says each number as its second begins.
pub fn countdown(output: &Output, n: u32) {
    *COUNTDOWN.lock().unwrap() = (n > 0).then(|| (output.name(), n, None));
}

/// Turn showing clicks and keys on or off.
pub fn set(clicks: bool, keys: bool) {
    CLICKS.store(clicks, Ordering::Relaxed);
    KEYS.store(keys, Ordering::Relaxed);
    if !clicks || !keys {
        let mut s = STATE.lock().unwrap();
        if !clicks {
            s.rings.clear();
        }
        if !keys {
            s.chord = None;
        }
    }
}

pub fn clicks() -> bool {
    CLICKS.load(Ordering::Relaxed)
}

pub fn keys() -> bool {
    KEYS.load(Ordering::Relaxed)
}

/// A button pressed at `at` (points from `output`'s top left).
pub fn click(output: &Output, at: Point<f64, Logical>) {
    if clicks() {
        STATE.lock().unwrap().rings.push((output.name(), at, Instant::now()));
    }
}

/// Whether physical `key` is one shown alone (without ⌃, ⌥ or ⌘).
pub fn shown_alone(key: u32) -> bool {
    // Escape, Backspace, Tab, Return, Delete, the arrows, keypad Enter.
    matches!(key, 1 | 14 | 15 | 28 | 111 | 103 | 105 | 106 | 108 | 96)
}

/// Chord `label` pressed, on `output`.
pub fn chord(output: &Output, label: String) {
    if keys() {
        STATE.lock().unwrap().chord = Some((output.name(), label, Instant::now(), None));
    }
}

/// Something is up (a ring spreading, a chord fading): the displays
/// draw on.
pub fn active() -> bool {
    let mut s = STATE.lock().unwrap();
    let now = Instant::now();
    s.rings.retain(|(_, _, at)| now.duration_since(*at) < RING);
    if s.chord.as_ref().is_some_and(|(_, _, at, _)| now.duration_since(*at) >= CHORD) {
        s.chord = None;
    }
    !s.rings.is_empty() || s.chord.is_some()
}

/// What's up on `output` now, as elements (its rings, then its chord).
pub fn elements<R>(renderer: &mut R, output: &Output, screen: Size<i32, Logical>) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut out = Vec::new();
    // A recording's countdown: a big number in the middle of the screen.
    if let Some((o, n, made)) = COUNTDOWN.lock().unwrap().as_mut().filter(|(o, ..)| *o == output.name()) {
        let _ = o;
        let scale = output.current_scale().fractional_scale();
        if made.as_ref().is_none_or(|(sc, ..)| *sc != scale) {
            let p = theme::palette();
            let pm = number(*n, scale, p.panel_text, p.panel_background, p.frame_focused[0]);
            let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
            let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, None);
            *made = Some((scale, buffer, (size.w, size.h)));
        }
        if let Some((_, buffer, (w, h))) = made {
            let phys = screen.to_f64().to_physical(scale);
            let at = (((phys.w - f64::from(*w)) / 2.0).round(), ((phys.h - f64::from(*h)) / 2.0).round());
            out.extend(crate::render::picture(renderer, at.into(), buffer, (*w, *h), None, Kind::Unspecified));
        }
    }
    if !clicks() && !keys() {
        return out;
    }
    let mut s = STATE.lock().unwrap();
    let now = Instant::now();
    let name = output.name();
    let scale = output.current_scale().fractional_scale();
    let p = theme::palette();
    for (o, at, since) in &s.rings {
        if *o != name {
            continue;
        }
        let t = (now.duration_since(*since).as_secs_f32() / RING.as_secs_f32()).min(1.0);
        let (pm, side) = ring(t, scale, p.navigation_base, p.panel_background);
        let size: Size<i32, Buffer> = (side, side).into();
        let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, None);
        let centre = at.to_physical(scale);
        let half = f64::from(side) / 2.0;
        out.extend(crate::render::picture(renderer, (centre.x - half, centre.y - half).into(), &buffer, (side, side), None, Kind::Unspecified));
    }
    if let Some((o, label, since, made)) = s.chord.as_mut().filter(|(o, ..)| *o == name) {
        let _ = o;
        if made.as_ref().is_none_or(|(sc, ..)| *sc != scale) {
            let pm = keycap(label, scale, p.panel_text, p.panel_background, p.frame_focused[0]);
            let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
            let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, None);
            *made = Some((scale, buffer, (size.w, size.h)));
        }
        if let Some((_, buffer, (w, h))) = made {
            let age = now.duration_since(*since).as_secs_f32() / CHORD.as_secs_f32();
            let alpha = if age < 0.8 { 1.0 } else { ((1.0 - age) / 0.2).clamp(0.0, 1.0) };
            let phys = screen.to_f64().to_physical(scale);
            let x = ((phys.w - f64::from(*w)) / 2.0).round();
            // Low, above the level indicator's place.
            let y = (phys.h * 0.80 - f64::from(*h)).round();
            out.extend(crate::render::picture(renderer, (x, y).into(), buffer, (*w, *h), Some(alpha), Kind::Unspecified));
        }
    }
    out
}

/// A ring `t` of the way through (0 to 1), at `scale`: its picture and
/// side in pixels. The navigation colour, a thin edge of the panel's
/// around it so it shows on anything, fading as it spreads.
fn ring(t: f32, scale: f64, ink: [u8; 4], edge: [u8; 4]) -> (Pixmap, i32) {
    let s = scale as f32;
    let side = ((RING_TO * 2.0 + 8.0) * s).ceil() as i32;
    let mut pm = Pixmap::new(side as u32, side as u32).expect("ring pixmap");
    let ease = 1.0 - (1.0 - t) * (1.0 - t);
    let r = (RING_FROM + (RING_TO - RING_FROM) * ease) * s;
    // Whole for the first part of its spread, then fading.
    let alpha = if t < 0.4 { 1.0 } else { ((1.0 - t) / 0.6).powf(1.2) };
    let c = side as f32 / 2.0;
    if let Some(circle) = PathBuilder::from_circle(c, c, r) {
        let id = tiny_skia::Transform::identity();
        let mut paint = Paint::default();
        paint.anti_alias = true;
        // A faint fill, an edge, then the ring.
        paint.set_color_rgba8(ink[0], ink[1], ink[2], (alpha * 50.0) as u8);
        pm.fill_path(&circle, &paint, FillRule::Winding, id, None);
        paint.set_color_rgba8(edge[0], edge[1], edge[2], (alpha * 200.0) as u8);
        pm.stroke_path(&circle, &paint, &Stroke { width: 5.0 * s, ..Default::default() }, id, None);
        paint.set_color_rgba8(ink[0], ink[1], ink[2], (alpha * 255.0) as u8);
        pm.stroke_path(&circle, &paint, &Stroke { width: 3.0 * s, ..Default::default() }, id, None);
    }
    (pm, side)
}

/// A countdown's number: a key cap, square, its figure large.
fn number(n: u32, scale: f64, ink: [u8; 4], face: [u8; 4], frame: [u8; 4]) -> Pixmap {
    keycap_sized(&n.to_string(), scale, 96.0, Some(1.0), ink, face, frame)
}

/// `label` on a key-cap panel at `scale`: big, bold, in the panel's
/// colours with its frame.
fn keycap(label: &str, scale: f64, ink: [u8; 4], face: [u8; 4], frame: [u8; 4]) -> Pixmap {
    keycap_sized(label, scale, 28.0, None, ink, face, frame)
}

/// `label` on a key cap, its text `points` tall; `square`: as wide as
/// tall at least.
fn keycap_sized(label: &str, scale: f64, points: f32, square: Option<f32>, ink: [u8; 4], face: [u8; 4], frame: [u8; 4]) -> Pixmap {
    let s = scale as f32;
    let size = points * s;
    let tw = text::measure(label, size, true).ceil();
    let (pad_x, pad_y) = (20.0 * s * points / 28.0, 12.0 * s * points / 28.0);
    let h = (size * 1.25 + 2.0 * pad_y).ceil() as u32;
    let w = ((tw + 2.0 * pad_x).ceil() as u32).max(if square.is_some() { h } else { 0 });
    // (The figure in the middle of a square cap.)
    let pad_x = (w as f32 - tw) / 2.0;
    let mut pm = Pixmap::new(w, h).expect("keycap pixmap");
    let id = tiny_skia::Transform::identity();
    let mut paint = Paint::default();
    paint.anti_alias = true;
    let rect = |inset: f32| {
        let r = 10.0 * s;
        let (x0, y0, x1, y1) = (inset, inset, w as f32 - inset, h as f32 - inset);
        let mut pb = PathBuilder::new();
        pb.move_to(x0 + r, y0);
        pb.line_to(x1 - r, y0);
        pb.quad_to(x1, y0, x1, y0 + r);
        pb.line_to(x1, y1 - r);
        pb.quad_to(x1, y1, x1 - r, y1);
        pb.line_to(x0 + r, y1);
        pb.quad_to(x0, y1, x0, y1 - r);
        pb.line_to(x0, y0 + r);
        pb.quad_to(x0, y0, x0 + r, y0);
        pb.close();
        pb.finish()
    };
    if let (Some(outer), Some(inner)) = (rect(0.0), rect(s.max(1.0))) {
        paint.set_color_rgba8(frame[0], frame[1], frame[2], 235);
        pm.fill_path(&outer, &paint, FillRule::Winding, id, None);
        paint.set_color_rgba8(face[0], face[1], face[2], 235);
        pm.fill_path(&inner, &paint, FillRule::Winding, id, None);
    }
    text::draw(&mut pm, label, pad_x, pad_y, size, true, ink);
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_is_never_shown_alone() {
        // Letters, digits and the space bar: never alone.
        for key in [30, 31, 2, 11, 57, 16] {
            assert!(!shown_alone(key));
        }
        // Return, Escape, Tab, the arrows: so.
        for key in [28, 1, 15, 103, 108, 105, 106] {
            assert!(shown_alone(key));
        }
    }

    #[test]
    fn rings_fade_and_go() {
        set(true, true);
        let (pm, side) = ring(0.0, 2.0, [0, 100, 255, 255], [220, 220, 220, 255]);
        assert_eq!(side, 120);
        assert!(pm.pixels().iter().any(|p| p.alpha() > 200));
        let (end, _) = ring(1.0, 2.0, [0, 100, 255, 255], [220, 220, 220, 255]);
        assert!(end.pixels().iter().all(|p| p.alpha() < 5));
        set(false, false);
        assert!(!active());
    }
}
