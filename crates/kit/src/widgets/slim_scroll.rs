//! A slim scroll bar, there when wanted: a thin rounded thumb over the
//! content's right edge, no track or arrows, hidden until the pointer
//! comes near that edge or the content scrolls, and fading out a moment
//! after (as macOS's overlay scroll bars do, where a BeOS bar's room
//! would cost a column of text: Terminal's scrollback). Under the pointer
//! it widens and shows its track. The thumb drags the view with the
//! pointer; a click in the track pages towards it. While hidden it takes
//! no clicks, so what's under it (a terminal's last column) still does.
//!
//! It knows nothing of the content: the app says where the view is
//! (`top` of `total`, `shown` showing, in any unit: lines, points) and
//! hears where to go ([`SlimScrollBar::on_scroll`]). Agents read it as a
//! `ScrollBar` whose value is how far down it is (0 to 1).
//!
//! Idle it costs nothing: it's drawn again, alone, when the time it
//! stays is up and for each frame of its fade
//! (`Painter::paint_again_within`), and not otherwise.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use olo_interface::{
    Bounds, CursorStyle, Element, ElementId, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, point, px,
    size,
};

use crate::{style, theme};

/// How wide the bar's room is at the content's right edge (the thumb is
/// thinner, wider under the pointer).
pub const SLIM_BAR: f32 = 12.0;
/// How near the edge the pointer comes before the bar shows.
pub const SLIM_NEAR: f32 = 28.0;
/// The thumb's breadth, and under the pointer or held.
const THUMB: f32 = 6.0;
const THUMB_WIDE: f32 = 8.0;
/// The thumb's least length.
const THUMB_MIN: f32 = 24.0;
/// The room round the thumb, at the edge and at the ends.
const EDGE: f32 = 2.0;
/// How long it stays after the pointer leaves or the scrolling stops,
/// and how long it takes to fade.
const LINGER: Duration = Duration::from_millis(900);
const FADE: Duration = Duration::from_millis(220);

type ScrollFn = Rc<dyn Fn(&f32, &mut WindowCx)>;

/// What it keeps from one drawing to the next: where the view was, until
/// when it shows, and what the pointer does.
#[derive(Default)]
struct SlimState {
    /// Where the view was when last drawn: its top, and how far its
    /// bottom was from the content's end.
    was: Option<(f32, f32)>,
    until: Option<Instant>,
    near: bool,
    over: bool,
    /// The thumb held: where along it the pointer took it.
    grab: Option<f32>,
    /// The view as last drawn (top, shown, total), and whether the bar
    /// showed at all: what its handlers go by.
    view: (f32, f32, f32),
    showing: bool,
}

/// A slim scroll bar over the right edge of the box it's given.
pub struct SlimScrollBar {
    id: ElementId,
    top: f32,
    shown: f32,
    total: f32,
    on_scroll: Option<ScrollFn>,
}

impl SlimScrollBar {
    /// The view shows `shown` of `total` from `top` (any unit).
    pub fn new(id: impl Into<ElementId>, top: f32, shown: f32, total: f32) -> SlimScrollBar {
        SlimScrollBar { id: id.into(), top, shown, total, on_scroll: None }
    }

    /// Where to go: the top it was dragged or paged to (in the units it
    /// was given, between 0 and `total - shown`).
    pub fn on_scroll(mut self, f: impl Fn(&f32, &mut WindowCx) + 'static) -> Self {
        self.on_scroll = Some(Rc::new(f));
        self
    }
}

/// Where its parts are in its box.
#[derive(Clone, Copy)]
struct Parts {
    /// The bar's room, and the pointer's "near" round it.
    bar: Bounds<Pixels>,
    near: Bounds<Pixels>,
    /// Where the thumb runs, and the thumb's length and place.
    track: Bounds<Pixels>,
    thumb_len: f32,
    travel: f32,
}

impl Parts {
    fn of(b: Bounds<Pixels>, shown: f32, total: f32) -> Parts {
        let bar = Bounds::from_corners(point(b.right() - px(SLIM_BAR), b.top()), b.bottom_right());
        let near = Bounds::from_corners(point(b.right() - px(SLIM_NEAR), b.top()), b.bottom_right());
        let track = Bounds::from_corners(point(bar.left(), bar.top() + px(EDGE)), point(bar.right(), bar.bottom() - px(EDGE)));
        let len = f32::from(track.size.height);
        let thumb_len = (len * shown / total.max(1e-6)).clamp(THUMB_MIN.min(len), len);
        Parts { bar, near, track, thumb_len, travel: (len - thumb_len).max(0.0) }
    }

    /// The thumb for a view at `top` of `room` (total - shown), `wide`
    /// or not.
    fn thumb(&self, top: f32, room: f32, wide: bool) -> Bounds<Pixels> {
        let w = if wide { THUMB_WIDE } else { THUMB };
        let at = if room > 0.0 { (top / room).clamp(0.0, 1.0) * self.travel } else { 0.0 };
        Bounds { origin: point(self.bar.right() - px(EDGE + w), self.track.top() + px(at)), size: size(px(w), px(self.thumb_len)) }
    }
}

impl IntoElement for SlimScrollBar {
    fn into_element(self) -> Element {
        let SlimScrollBar { id, top, shown, total, on_scroll } = self;
        let room = total - shown;
        let mut element = canvas(move |g, b| {
            if room <= 0.5 {
                // Everything shows: nothing to scroll (it's gone at once).
                *g.node_state::<SlimState>() = SlimState::default();
                return;
            }
            let parts = Parts::of(b, shown, total);
            let now = Instant::now();
            let (alpha, held, wide, until) = {
                let s = g.node_state::<SlimState>();
                // Scrolled since it was last drawn: moved from both ends.
                // (Not the first time: a view opened scrolled back
                // doesn't flash its bar. Not content added at an end the
                // view keeps to, a terminal's output, nor the view
                // resized: one end stays.)
                let at = (top, room - top);
                if s.was.is_some_and(|(t, b)| (t - at.0).abs() > 1e-3 && (b - at.1).abs() > 1e-3) {
                    s.until = Some(now + LINGER);
                }
                s.was = Some(at);
                s.view = (top, shown, total);
                // (Near, or held: it shows, and waits for the pointer to
                // move.)
                let held = s.near || s.grab.is_some();
                let alpha = match s.until {
                    _ if held => 1.0,
                    Some(until) if now < until => 1.0,
                    Some(until) => 1.0 - (now - until).as_secs_f32() / FADE.as_secs_f32(),
                    None => 0.0,
                }
                .clamp(0.0, 1.0);
                s.showing = alpha > 0.0;
                (alpha, held, s.over || s.grab.is_some(), s.until)
            };
            // Drawn again when the time it stays is up, then for each
            // frame of its fade (nothing while the pointer keeps it, or
            // once it's gone).
            if alpha > 0.0 && !held {
                g.paint_again_within(until.map_or(0.0, |until| until.saturating_duration_since(now).as_secs_f64()));
            }
            let thumb = style::snap(g, parts.thumb(top, room, wide));
            if alpha > 0.0 {
                let p = theme::palette();
                let ink = style::c(p.panel_text);
                g.with_element_opacity(Some(alpha), |g| {
                    if wide {
                        // Under the pointer: its track, faintly, as wide
                        // as the thumb is.
                        let track = style::snap(g, Bounds::from_corners(point(thumb.left(), parts.track.top()), point(thumb.right(), parts.track.bottom())));
                        g.paint_quad(olo_interface::fill(track, ink.opacity(0.08)).corner_radii(track.size.width / 2.0));
                    }
                    g.paint_quad(olo_interface::fill(thumb, ink.opacity(if wide { 0.55 } else { 0.4 })).corner_radii(thumb.size.width / 2.0));
                });
                g.set_cursor_style(CursorStyle::Arrow, parts.bar);
            }
            let at = (top / room).clamp(0.0, 1.0);
            g.agent(agent::Node { role: "ScrollBar", name: Some("Scroll Bar".into()), value: Some(SharedString::from(format!("{at:.2}"))), bounds: parts.track, enabled: true, ..Default::default() });
        })
        .id(id)
        .paint_key(style::key((top.to_bits(), shown.to_bits(), total.to_bits())))
        .absolute()
        .inset_0();

        // The pointer near the edge (or over the bar), whether it shows
        // or not: shown, it stays. And the thumb, held, follows it
        // wherever in the window it goes.
        let go = on_scroll.clone();
        element = element.on_window_mouse_move(move |e: &MouseMoveEvent, cx| {
            let b = cx.listener_bounds();
            let s = cx.node_state::<SlimState>();
            let (_, shown, total) = s.view;
            if total - shown <= 0.5 {
                return;
            }
            let parts = Parts::of(b, shown, total);
            let (near, over) = (parts.near.contains(&e.position), parts.bar.contains(&e.position));
            let changed = s.near != near || s.over != over;
            if changed {
                // (Gone from near the edge: it stays a moment, then
                // fades.)
                if s.near && !near {
                    s.until = Some(Instant::now() + LINGER);
                }
                (s.near, s.over) = (near, over);
            }
            let grab = s.grab.filter(|_| e.pressed_button == Some(MouseButton::Left) && parts.travel > 0.0);
            if changed {
                cx.repaint();
            }
            if let (Some(grab), Some(go)) = (grab, &go) {
                let along = (f32::from(e.position.y - parts.track.top()) - grab).clamp(0.0, parts.travel);
                go(&(along / parts.travel * (total - shown)), cx);
            }
        });
        let Some(go) = on_scroll else { return element };
        element
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
                let b = cx.listener_bounds();
                let s = cx.node_state::<SlimState>();
                let (top, shown, total) = s.view;
                // (Hidden, it takes nothing: what's under it does.)
                if !s.showing || total - shown <= 0.5 {
                    return;
                }
                let parts = Parts::of(b, shown, total);
                if !parts.bar.contains(&e.position) {
                    return;
                }
                let scale = olo_interface::scale_factor();
                let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
                let thumb = parts.thumb(top, total - shown, s.over);
                let (thumb_top, thumb_bottom) = (snap(thumb.top()), snap(thumb.bottom()));
                s.until = Some(Instant::now() + LINGER);
                let page = if (thumb_top..thumb_bottom).contains(&e.position.y) {
                    s.grab = Some(f32::from(e.position.y - thumb_top));
                    None
                } else {
                    // A page towards the click.
                    let up = e.position.y < thumb_top;
                    Some((top + if up { -shown } else { shown } * 0.9).clamp(0.0, total - shown))
                };
                cx.stop_propagation();
                cx.repaint();
                if let Some(to) = page {
                    go(&to, cx);
                }
            })
            .on_window_mouse_up(|e, cx| {
                let s = cx.node_state::<SlimState>();
                if e.button == MouseButton::Left && s.grab.take().is_some() {
                    s.until = Some(Instant::now() + LINGER);
                    cx.repaint();
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_is_as_long_as_what_shows_and_runs_the_track() {
        let b = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(400.0), px(204.0)) };
        // A quarter shows: a quarter of the 200-point track.
        let p = Parts::of(b, 25.0, 100.0);
        assert_eq!(p.thumb_len, 50.0);
        assert_eq!(p.travel, 150.0);
        assert_eq!(p.thumb(0.0, 75.0, false).top(), px(2.0));
        assert_eq!(p.thumb(75.0, 75.0, false).bottom(), px(202.0));
        // At the right edge, inset; wider under the pointer.
        assert_eq!(p.thumb(0.0, 75.0, false).right(), px(398.0));
        assert_eq!(p.thumb(0.0, 75.0, true).size.width, px(THUMB_WIDE));
        // Much scrollback: never shorter than its least.
        assert_eq!(Parts::of(b, 1.0, 100_000.0).thumb_len, THUMB_MIN);
    }
}
