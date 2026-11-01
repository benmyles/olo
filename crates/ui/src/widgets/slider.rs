//! BSlider, as the egui kit draws it: a label and the value above a
//! sunken bar filled to the thumb, a raised block thumb with a grip line
//! and a soft shadow, the limits under the ends. A press or a drag sets
//! the value; with the keyboard (a click gives it), the arrows step it
//! and Home and End go to the ends. The app keeps the value and is told
//! each change (`on_change`), as it is dragged.

use std::rc::Rc;

use gpui::{
    App, Bounds, Entity, FocusHandle, FontWeight, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas, div, point, px,
};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

const THUMB_W: f32 = 11.0;
const THUMB_H: f32 = 17.0;
/// The thumb's row, room for its shadow under it: on the two-point grid,
/// so what comes after a slider is on device pixels.
const THUMB_ROW: f32 = 20.0;
const BAR_H: f32 = 6.0;

type OnChange = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// A slider's state from frame to frame: the keyboard's handle, and
/// whether it's being dragged.
struct SliderState {
    focus: FocusHandle,
    dragging: bool,
}

#[derive(IntoElement)]
pub struct Slider {
    id: SharedString,
    value: f32,
    lo: f32,
    hi: f32,
    label: Option<SharedString>,
    value_text: Option<SharedString>,
    limits: Option<(SharedString, SharedString)>,
    fill: Option<Hsla>,
    step: f32,
    width: Option<f32>,
    enabled: bool,
    on_change: Option<OnChange>,
}

impl Slider {
    pub fn new(id: impl Into<SharedString>, value: f32, range: std::ops::RangeInclusive<f32>) -> Slider {
        Slider { id: id.into(), value, lo: *range.start(), hi: *range.end(), label: None, value_text: None, limits: None, fill: None, step: 0.0, width: None, enabled: true, on_change: None }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Text at the right above the bar ("75 %").
    pub fn value_text(mut self, text: impl Into<SharedString>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Labels under the ends of the bar.
    pub fn limits(mut self, min: impl Into<SharedString>, max: impl Into<SharedString>) -> Self {
        self.limits = Some((min.into(), max.into()));
        self
    }

    /// The colour of the bar left of the thumb (else the highlight).
    pub fn fill(mut self, color: Hsla) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_change(mut self, f: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state: Entity<SliderState> = window.use_keyed_state(SharedString::from(format!("olo-slider-{}", self.id)), cx, |_, cx| SliderState { focus: cx.focus_handle().tab_stop(true), dragging: false });
        let focus = state.read(cx).focus.clone();
        let top = if self.label.is_some() || self.value_text.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let bottom = if self.limits.is_some() { m::LABEL_ROW } else { 0.0 };
        let height = top + THUMB_ROW + bottom;
        let Slider { value, lo, hi, label, value_text, limits, fill, step, enabled, on_change, .. } = self;
        let snap_step = move |v: f32| if step > 0.0 { (v / step).round() * step } else { v };
        let keys_change = on_change.clone();
        let key_step = if step > 0.0 { step } else { (hi - lo) / 100.0 };
        let focused = focus.is_focused(window);
        let paint_state = state.clone();
        let el = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |b, hitbox, window, cx| {
                let p = theme::palette(cx);
                let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
                let font = style::ui_font(FontWeight::NORMAL);
                let track = Bounds::from_corners(point(b.left() + px(THUMB_W / 2.0), b.top() + px(top)), point(b.right() - px(THUMB_W / 2.0), b.top() + px(top + THUMB_H)));
                let to_value = move |x: Pixels| {
                    let t = (f32::from(x - track.left()) / f32::from(track.size.width)).clamp(0.0, 1.0);
                    snap_step(lo + t * (hi - lo)).clamp(lo, hi)
                };
                if let Some(l) = &label {
                    let r = style::label(window, cx, b.left(), b.top() + px(m::LABEL_ROW / 2.0), style::Align::Left, l.clone(), style::PLAIN_SIZE, font.clone(), ink);
                    if focused {
                        style::hline_above(window, r.left(), r.right(), r.bottom(), style::c(p.keyboard_navigation));
                    }
                }
                if let Some(t) = &value_text {
                    style::label(window, cx, b.right(), b.top() + px(m::LABEL_ROW / 2.0), style::Align::Right, t.clone(), style::PLAIN_SIZE, font.clone(), ink);
                }
                // The bar.
                let s = window.scale_factor();
                let bar_top = px(((f32::from(track.center().y) - BAR_H / 2.0) * s).round() / s);
                let bar = Bounds::from_corners(point(b.left(), bar_top), point(b.right(), bar_top + px(BAR_H)));
                let t = if hi > lo { (value - lo) / (hi - lo) } else { 0.0 };
                let thumb_x = style::snap_coord(window, track.left() + track.size.width * t);
                let fill = fill.unwrap_or(style::c(p.control_highlight));
                let fill = if enabled { fill } else { fill.opacity(0.4) };
                let in_bar = style::inset(window, bar, 1.0);
                style::fill_rect(window, in_bar, style::shade(&p, p.panel_background, 1.08));
                let filled = Bounds::from_corners(in_bar.origin, point(thumb_x, in_bar.bottom()));
                let lower = {
                    let c = gpui::Rgba::from(fill);
                    let k = 0.85;
                    Hsla::from(gpui::Rgba { r: c.r * k, g: c.g * k, b: c.b * k, a: 1.0 })
                };
                if filled.size.width > px(0.0) {
                    style::vgradient(window, filled, fill, if enabled { lower } else { fill });
                }
                style::bevel(window, bar, style::shade(&p, p.panel_background, 1.45), style::c(p.shine));
                // The thumb, a soft shadow lifting it off the bar.
                let thumb = style::snap_centred(window, point(thumb_x + px(0.5), track.center().y), THUMB_W, THUMB_H);
                let base = p.control_background;
                let pressed = paint_state.read(cx).dragging;
                let (tt, tb) = if pressed { (style::shade(&p, base, 1.15), style::shade(&p, base, 1.08)) } else { (style::tint(base, 0.8), style::shade(&p, base, 1.06)) };
                let line = style::hairline(window);
                window.paint_quad(gpui::fill(Bounds { origin: point(thumb.left() + line, thumb.top() + line), size: thumb.size }, style::c(p.drop_shadow)).corner_radii(px(1.0)));
                let in_thumb = style::inset(window, thumb, 1.0);
                style::vgradient(window, in_thumb, tt, tb);
                style::frame_rect(window, thumb, style::shade(&p, p.panel_background, 1.6));
                style::bevel(window, in_thumb, style::c(p.shine), style::shade(&p, base, 1.2));
                // The grip line.
                let gx = px((f32::from(thumb.center().x) * s).floor() / s);
                style::vline_left_of(window, gx, thumb.top() + px(4.0), thumb.bottom() - px(4.0), style::shade(&p, base, 1.35));
                style::vline(window, gx, thumb.top() + px(4.0), thumb.bottom() - px(4.0), style::c(p.shine));
                if let Some((min, max)) = &limits {
                    let y = b.bottom() - px(m::LABEL_ROW / 2.0 - 1.0);
                    style::label(window, cx, b.left(), y, style::Align::Left, min.clone(), style::SMALL_SIZE, font.clone(), ink);
                    style::label(window, cx, b.right(), y, style::Align::Right, max.clone(), style::SMALL_SIZE, font.clone(), ink);
                }
                agent::record(window, cx, agent::Node { role: "Slider", name: label.clone(), value: Some(format!("{value}").into()), bounds: b, enabled, clickable: enabled, focused, ..Default::default() });
                if !enabled {
                    return;
                }

                let (st, f, hb) = (paint_state.clone(), on_change.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left || !hb.is_hovered(window) {
                        return;
                    }
                    let focus = st.read(cx).focus.clone();
                    window.focus(&focus);
                    st.update(cx, |s, cx| {
                        s.dragging = true;
                        cx.notify();
                    });
                    let v = to_value(e.position.x);
                    if let Some(f) = &f
                        && v != value
                    {
                        f(v, window, cx);
                    }
                });
                let (st, f) = (paint_state.clone(), on_change.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() || !e.dragging() || !st.read(cx).dragging {
                        return;
                    }
                    let v = to_value(e.position.x);
                    if let Some(f) = &f
                        && v != value
                    {
                        f(v, window, cx);
                    }
                });
                let st = paint_state.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && e.button == MouseButton::Left && st.read(cx).dragging {
                        st.update(cx, |s, cx| {
                            s.dragging = false;
                            cx.notify();
                        });
                    }
                });
            },
        )
        .size_full();
        let mut root = div().id(SharedString::from(format!("slider-{}", self.id))).h(px(height)).flex_shrink_0().track_focus(&focus).child(el);
        root = match self.width {
            Some(w) => root.w(px(w)),
            None => root.w_full(),
        };
        if enabled {
            root = root.on_key_down(move |e: &KeyDownEvent, window, cx| {
                let v = match e.keystroke.key.as_str() {
                    "left" | "down" => value - key_step,
                    "right" | "up" => value + key_step,
                    "home" => lo,
                    "end" => hi,
                    _ => return,
                };
                let v = v.clamp(lo, hi);
                if let Some(f) = &keys_change
                    && v != value
                {
                    f(v, window, cx);
                }
                cx.stop_propagation();
            });
        }
        root
    }
}
