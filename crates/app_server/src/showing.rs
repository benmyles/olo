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

/// A recording's caption (`Request::Caption`, an agent's narration):
/// drawn into captures only, never on the screen, low in the middle,
/// fading in and out.
struct Caption {
    text: String,
    since: Instant,
    /// When it goes (fading out till then); None: when it's replaced.
    until: Option<Instant>,
    /// Its picture, made once for a scale and width.
    made: Option<CaptionPicture>,
}

/// A caption's picture: the scale and width it was made for, it, and its
/// size in pixels.
type CaptionPicture = ((u64, i32), MemoryRenderBuffer, (i32, i32));

static CAPTION: Mutex<Option<Caption>> = Mutex::new(None);

/// How long a caption takes to fade in or out.
const FADE: Duration = Duration::from_millis(180);
/// A caption's text size (points).
const CAPTION_SIZE: f32 = 22.0;

/// Caption `text` up in captures from now, for `seconds` (None: till
/// replaced); None or empty: the one up fades out. When the display should
/// look again (the end of the fade or the caption's end), if ever.
pub fn caption(text: Option<String>, seconds: Option<f64>) -> Option<Duration> {
    let mut c = CAPTION.lock().unwrap();
    let now = Instant::now();
    match text.filter(|t| !t.trim().is_empty()) {
        Some(text) => {
            let until = seconds.filter(|s| *s > 0.0).map(|s| now + Duration::from_secs_f64(s.min(3600.0)));
            *c = Some(Caption { text, since: now, until, made: None });
            Some(until.map_or(FADE, |u| u - now + Duration::from_millis(10)))
        }
        None => {
            let shown = c.as_mut()?;
            shown.until = Some(now + FADE);
            Some(FADE + Duration::from_millis(10))
        }
    }
}

/// A caption is fading in or out: captures draw on till it's done.
pub fn caption_fading() -> bool {
    let c = CAPTION.lock().unwrap();
    let now = Instant::now();
    c.as_ref().is_some_and(|c| now.duration_since(c.since) < FADE || c.until.is_some_and(|u| u > now && u - now < FADE))
}

/// The caption up now, as elements over a capture `size` device pixels
/// at `scale`: centred, low, at most four fifths of its width.
pub fn caption_elements<R>(renderer: &mut R, size: Size<i32, smithay::utils::Physical>, scale: f64) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut c = CAPTION.lock().unwrap();
    let now = Instant::now();
    if c.as_ref().is_some_and(|c| c.until.is_some_and(|u| now >= u)) {
        *c = None;
    }
    let Some(cap) = c.as_mut() else { return Vec::new() };
    let width = (size.w as f32 * 0.8) as i32;
    let key = (scale.to_bits(), width);
    if cap.made.as_ref().is_none_or(|(k, ..)| *k != key) {
        let pm = caption_panel(&cap.text, scale, width as f32);
        let px: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
        let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, px, 1, Transform::Normal, None);
        cap.made = Some((key, buffer, (px.w, px.h)));
    }
    let Some((_, buffer, (w, h))) = &cap.made else { return Vec::new() };
    let fade_in = now.duration_since(cap.since).as_secs_f32() / FADE.as_secs_f32();
    let fade_out = cap.until.map_or(1.0, |u| u.saturating_duration_since(now).as_secs_f32() / FADE.as_secs_f32());
    let alpha = fade_in.min(fade_out).clamp(0.0, 1.0);
    let x = ((size.w - w) / 2) as f64;
    let y = (f64::from(size.h) * 0.93 - f64::from(*h)).round();
    crate::render::picture(renderer, (x, y).into(), buffer, (*w, *h), Some(alpha), Kind::Unspecified).into_iter().collect()
}

/// `text` on a dark panel, white, wrapped to `max_w` pixels (at most three
/// lines, the last cut short with an ellipsis).
fn caption_panel(text: &str, scale: f64, max_w: f32) -> Pixmap {
    let s = scale as f32;
    let size = CAPTION_SIZE * s;
    let (pad_x, pad_y, gap) = (16.0 * s, 10.0 * s, 4.0 * s);
    let inner = (max_w - 2.0 * pad_x).max(size * 4.0);
    // Words onto lines as they fit.
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if text::measure(&format!("{l} {word}"), size, false) <= inner => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    if lines.len() > 3 {
        lines.truncate(3);
        lines[2].push('…');
    }
    for l in &mut lines {
        while text::measure(l, size, false) > inner && l.chars().count() > 1 {
            l.pop();
            l.pop();
            l.push('…');
        }
    }
    let line_h = (size * 1.3).ceil();
    let tw = lines.iter().map(|l| text::measure(l, size, false)).fold(0.0, f32::max);
    let (w, h) = ((tw + 2.0 * pad_x).ceil() as u32, (lines.len() as f32 * line_h + (lines.len().max(1) - 1) as f32 * gap + 2.0 * pad_y).ceil() as u32);
    let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("caption pixmap");
    let r = 10.0 * s;
    let (x1, y1) = (w as f32, h as f32);
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(x1 - r, 0.0);
    pb.quad_to(x1, 0.0, x1, r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(r, y1);
    pb.quad_to(0.0, y1, 0.0, y1 - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint { anti_alias: true, ..Default::default() };
        paint.set_color_rgba8(0, 0, 0, 190);
        pm.fill_path(&path, &paint, FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    for (i, l) in lines.iter().enumerate() {
        let lw = text::measure(l, size, false);
        text::draw(&mut pm, l, ((w as f32 - lw) / 2.0).round(), pad_y + i as f32 * (line_h + gap), size, false, [255, 255, 255, 255]);
    }
    pm
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

/// The pointer as captures draw it (screen recordings, screen sharing):
/// how many times its size, whether it glides after the real one, and
/// the glide: where it's drawn, how fast it goes (points a second), where
/// it's going and as of when.
struct Captured {
    size: f64,
    smooth: bool,
    glide: Option<Glide>,
}

#[derive(Clone, Copy)]
struct Glide {
    at: Point<f64, Logical>,
    speed: Point<f64, Logical>,
    to: Point<f64, Logical>,
    when: Instant,
}

static CAPTURED: Mutex<Captured> = Mutex::new(Captured { size: 1.0, smooth: false, glide: None });

/// How fast the glide closes in: a critically damped spring's rate (per
/// second). A jump is 95% done in 0.26 s, a hand's jitter (a few points
/// back and forth in 20 ms) smoothed away, no overshoot.
const GLIDE: f64 = 18.0;

/// Captures draw the pointer `size` times its size (1, or 0: as it is),
/// gliding after the real one if `smooth`.
pub fn set_pointer(size: f64, smooth: bool) {
    let mut c = CAPTURED.lock().unwrap();
    c.size = if size > 0.0 { size.clamp(1.0, 4.0) } else { 1.0 };
    c.smooth = smooth;
    c.glide = None;
}

/// Where captures draw the pointer, the real one being at `real` (the
/// glide moved on to now), and how many times its size.
pub fn captured_pointer(real: Point<f64, Logical>) -> (Point<f64, Logical>, f64) {
    let mut c = CAPTURED.lock().unwrap();
    if !c.smooth {
        return (real, c.size);
    }
    let now = Instant::now();
    let g = c.glide.unwrap_or(Glide { at: real, speed: Point::default(), to: real, when: now });
    let (at, speed) = glide(g.at - real, g.speed, now.duration_since(g.when).as_secs_f64());
    let at = real + at;
    c.glide = Some(Glide { at, speed, to: real, when: now });
    (at, c.size)
}

/// A critically damped spring `dt` seconds on: from `off` (where it is,
/// less where it's going) at `speed`, its new offset and speed. Exact for
/// any `dt`, so frames coming unevenly glide the same.
fn glide(off: Point<f64, Logical>, speed: Point<f64, Logical>, dt: f64) -> (Point<f64, Logical>, Point<f64, Logical>) {
    let e = (-GLIDE * dt).exp();
    let axis = |y: f64, v: f64| {
        let k = v + GLIDE * y;
        ((y + k * dt) * e, (v - GLIDE * k * dt) * e)
    };
    let ((x, vx), (y, vy)) = (axis(off.x, speed.x), axis(off.y, speed.y));
    // (Within a twentieth of a point and nearly still: arrived.)
    if x.hypot(y) < 0.05 && vx.hypot(vy) < 1.0 {
        return (Point::default(), Point::default());
    }
    ((x, y).into(), (vx, vy).into())
}

/// The captured pointer is still gliding: the displays draw on till it
/// arrives.
pub fn gliding() -> bool {
    let c = CAPTURED.lock().unwrap();
    c.smooth
        && c.glide.is_some_and(|g| {
            let off = g.at - g.to;
            off.x.hypot(off.y) >= 0.05 || g.speed.x.hypot(g.speed.y) >= 1.0
        })
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
    if !CHORD_APART.get() {
        out.extend(chord_in(&mut s, renderer, output, screen));
    }
    out
}

thread_local! {
    /// The chord left out of `elements`, for a zoomed capture to draw it
    /// unzoomed (`apart`).
    static CHORD_APART: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `f` with the chord left out of `elements`, then the chord's elements
/// on their own: a zoomed capture draws them over the zoomed picture, in
/// their place on the screen, so a shortcut shows wherever the view is.
pub fn apart<R, T>(renderer: &mut R, output: &Output, screen: Size<i32, Logical>, f: impl FnOnce(&mut R) -> T) -> (T, Vec<Exact<MemoryRenderBufferRenderElement<R>>>)
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    CHORD_APART.set(true);
    let made = f(renderer);
    CHORD_APART.set(false);
    let chord = if keys() { chord_in(&mut STATE.lock().unwrap(), renderer, output, screen) } else { Vec::new() };
    (made, chord)
}

/// The chord up on `output`, if any, as elements.
fn chord_in<R>(s: &mut State, renderer: &mut R, output: &Output, screen: Size<i32, Logical>) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut out = Vec::new();
    let (name, scale, p, now) = (output.name(), output.current_scale().fractional_scale(), theme::palette(), Instant::now());
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
    fn the_pointer_glides_without_overshooting() {
        // A jump of 400 points: 95% there in 0.26 s, never past it.
        let (mut off, mut speed) = (Point::<f64, Logical>::from((-400.0, 0.0)), Point::default());
        let mut t = 0.0;
        while t < 0.26 {
            (off, speed) = glide(off, speed, 1.0 / 120.0);
            assert!(off.x <= 0.0, "overshot at {t}: {off:?}");
            t += 1.0 / 120.0;
        }
        assert!(off.x > -25.0 && off.x < -5.0, "{off:?}");
        // The same time in uneven steps: the same place.
        let (mut o2, mut s2) = (Point::<f64, Logical>::from((-400.0, 0.0)), Point::default());
        for dt in [0.1, 0.01, 0.05, 0.1] {
            (o2, s2) = glide(o2, s2, dt);
        }
        let (o1, _) = glide((-400.0, 0.0).into(), Point::default(), 0.26);
        assert!((o2.x - o1.x).abs() < 0.5, "{o1:?} {o2:?}");
        let _ = s2;
        // Long enough: arrived, still.
        assert_eq!(glide((-400.0, 3.0).into(), (50.0, 0.0).into(), 2.0), (Point::default(), Point::default()));
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
