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
//! Idle it costs nothing: it shows and fades by one timer and the frames
//! of its fade only.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, CursorStyle, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, Style, Task, Window, point, px, relative, size,
};

use crate::{agent, style, theme};

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

type ScrollFn = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// Between frames: where the view was, until when it shows, what the
/// pointer does, and the timer that fades it.
#[derive(Default)]
struct SlimState {
    /// Where the view was last frame: its top, and how far its bottom was
    /// from the content's end.
    top: Option<(f32, f32)>,
    until: Option<Instant>,
    near: bool,
    over: bool,
    /// The thumb held: where along it the pointer took it.
    grab: Option<f32>,
    wake: Option<(Instant, Task<()>)>,
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
    pub fn on_scroll(mut self, f: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_scroll = Some(Rc::new(f));
        self
    }
}

impl IntoElement for SlimScrollBar {
    type Element = Self;

    fn into_element(self) -> Self {
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

/// What prepaint found: its parts, how strongly it shows, and the hitbox
/// it takes clicks with while it shows.
pub struct Prepainted {
    parts: Parts,
    alpha: f32,
    hitbox: Option<Hitbox>,
    state: Rc<RefCell<SlimState>>,
}

impl Element for SlimScrollBar {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepainted>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Option<Prepainted> {
        let state = window.with_optional_element_state::<Rc<RefCell<SlimState>>, _>(id, |s, _| {
            let s = s.flatten().unwrap_or_default();
            (s.clone(), Some(s))
        });
        let room = self.total - self.shown;
        if room <= 0.5 {
            // Everything shows: nothing to scroll (it's gone at once).
            *state.borrow_mut() = SlimState::default();
            return None;
        }
        let parts = Parts::of(bounds, self.shown, self.total);
        let now = Instant::now();
        let (alpha, held) = {
            let mut s = state.borrow_mut();
            // Scrolled since the last frame: moved from both ends. (Not
            // the first frame: a view opened scrolled back doesn't flash
            // its bar. Not content added at an end the view keeps to, a
            // terminal's output, nor the view resized: one end stays.)
            let at = (self.top, room - self.top);
            if s.top.is_some_and(|(t, b)| (t - at.0).abs() > 1e-3 && (b - at.1).abs() > 1e-3) {
                s.until = Some(now + LINGER);
            }
            s.top = Some(at);
            // (Near, or held: it shows, and waits for the pointer to move.)
            let held = s.near || s.grab.is_some();
            let alpha = match s.until {
                _ if held => 1.0,
                Some(until) if now < until => 1.0,
                Some(until) => 1.0 - (now - until).as_secs_f32() / FADE.as_secs_f32(),
                None => 0.0,
            };
            (alpha.clamp(0.0, 1.0), held)
        };
        // A frame each step of the fade; the fade's start woken by a timer
        // (nothing while it stays, or once it's gone).
        if alpha > 0.0 && !held {
            let until = state.borrow().until;
            match until {
                Some(until) if now < until => {
                    let pending = state.borrow().wake.as_ref().is_some_and(|(at, _)| *at == until);
                    if !pending {
                        let view = window.current_view();
                        let task = cx.spawn(async move |cx| {
                            cx.background_executor().timer(until.saturating_duration_since(Instant::now())).await;
                            let _ = cx.update(|cx| cx.notify(view));
                        });
                        state.borrow_mut().wake = Some((until, task));
                    }
                }
                _ => window.request_animation_frame(),
            }
        }
        let hitbox = (alpha > 0.0).then(|| window.insert_hitbox(parts.bar, HitboxBehavior::Normal));
        Some(Prepainted { parts, alpha, hitbox, state })
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), pre: &mut Option<Prepainted>, window: &mut Window, cx: &mut App) {
        let Some(Prepainted { parts, alpha, hitbox, state }) = pre.take() else { return };
        let room = self.total - self.shown;
        let (over, grab) = {
            let s = state.borrow();
            (s.over, s.grab)
        };
        let wide = over || grab.is_some();
        let thumb = style::snap(window, parts.thumb(self.top, room, wide));
        if alpha > 0.0 {
            let p = theme::palette(cx);
            let ink = style::c(p.panel_text);
            window.with_element_opacity(Some(alpha), |window| {
                if wide {
                    // Under the pointer: its track, faintly, as wide as
                    // the thumb is.
                    let track = style::snap(window, Bounds::from_corners(point(thumb.left(), parts.track.top()), point(thumb.right(), parts.track.bottom())));
                    window.paint_quad(gpui::fill(track, ink.opacity(0.08)).corner_radii(track.size.width / 2.0));
                }
                window.paint_quad(gpui::fill(thumb, ink.opacity(if wide { 0.55 } else { 0.4 })).corner_radii(thumb.size.width / 2.0));
            });
        }
        let at = (self.top / room).clamp(0.0, 1.0);
        agent::record(
            window,
            cx,
            agent::Node { role: "ScrollBar", name: Some("Scroll Bar".into()), value: Some(SharedString::from(format!("{at:.2}"))), bounds: parts.track, enabled: true, ..Default::default() },
        );
        if let Some(h) = &hitbox {
            window.set_cursor_style(CursorStyle::Arrow, h);
        }

        // The pointer near the edge (or over the bar), whether it shows
        // or not: shown, it stays.
        let st = state.clone();
        // (The view it's in drawn again, not the whole window: control.rs.)
        let view = window.current_view();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if !phase.capture() {
                return;
            }
            let (near, over) = (parts.near.contains(&e.position), parts.bar.contains(&e.position));
            let mut s = st.borrow_mut();
            if s.near != near || s.over != over {
                // (Gone from near the edge: it stays a moment, then fades.)
                if s.near && !near {
                    s.until = Some(Instant::now() + LINGER);
                }
                (s.near, s.over) = (near, over);
                drop(s);
                cx.notify(view);
            }
        });
        let Some(on_scroll) = self.on_scroll.clone() else { return };
        let (top, total, shown) = (self.top, self.total, self.shown);
        if let Some(hb) = hitbox {
            let (st, go) = (state.clone(), on_scroll.clone());
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble() || e.button != MouseButton::Left || !hb.is_hovered(window) {
                    return;
                }
                cx.stop_propagation();
                if thumb.contains(&e.position) {
                    st.borrow_mut().grab = Some(f32::from(e.position.y - thumb.top()));
                } else {
                    // A page towards the click.
                    let up = e.position.y < thumb.top();
                    go((top + if up { -shown } else { shown } * 0.9).clamp(0.0, total - shown), window, cx);
                }
                st.borrow_mut().until = Some(Instant::now() + LINGER);
                window.refresh();
            });
        }
        let (st, go) = (state.clone(), on_scroll);
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if !phase.capture() || !e.dragging() {
                return;
            }
            let Some(grab) = st.borrow().grab else { return };
            if parts.travel <= 0.0 {
                return;
            }
            let along = (f32::from(e.position.y - parts.track.top()) - grab).clamp(0.0, parts.travel);
            go(along / parts.travel * (total - shown), window, cx);
        });
        let st = state;
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, _| {
            if phase.capture() && e.button == MouseButton::Left && st.borrow_mut().grab.take().is_some() {
                st.borrow_mut().until = Some(Instant::now() + LINGER);
                window.refresh();
            }
        });
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
