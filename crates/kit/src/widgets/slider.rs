//! BSlider: a label and the value above a sunken bar filled to the
//! thumb, a raised block thumb with a grip line and a soft shadow, the
//! limits under the ends. A press or a drag sets the value; with the
//! keyboard (a click gives it, or Tab), the arrows step it and Home and
//! End go to the ends. The app keeps the value and is told each change
//! (`on_change`), as it is dragged.

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, Pixels, SharedString, Styled as _, WindowCx, agent, control_canvas, fill, point, px,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

const THUMB_W: f32 = 11.0;
const THUMB_H: f32 = 17.0;
/// The thumb's row, room for its shadow under it: on the two-point grid,
/// so what comes after a slider is on device pixels.
const THUMB_ROW: f32 = 20.0;
const BAR_H: f32 = 6.0;

type OnChange = Rc<dyn Fn(&f32, &mut WindowCx)>;

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
    /// Its label, if it has one (an undo step's name).
    pub fn title(&self) -> Option<SharedString> {
        self.label.clone()
    }

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

    /// Told each new value, as it's dragged
    /// (`cx.msg_with(|v: &f32| Msg::Volume(*v))`).
    pub fn on_change(mut self, f: impl Fn(&f32, &mut WindowCx) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// The thumb's track in a slider's bounds `b`, its label's row `top`
/// tall.
fn track_in(b: Bounds<Pixels>, top: f32) -> Bounds<Pixels> {
    Bounds::from_corners(point(b.left() + px(THUMB_W / 2.0), b.top() + px(top)), point(b.right() - px(THUMB_W / 2.0), b.top() + px(top + THUMB_H)))
}

impl IntoElement for Slider {
    fn into_element(self) -> Element {
        let top = if self.label.is_some() || self.value_text.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let bottom = if self.limits.is_some() { m::LABEL_ROW } else { 0.0 };
        let height = top + THUMB_ROW + bottom;
        let look = style::key((&self.label, &self.value_text, &self.limits, self.value.to_bits(), self.lo.to_bits(), self.hi.to_bits(), style::ink_key(self.fill), self.enabled));
        let Slider { id, value, lo, hi, label, value_text, limits, fill: bar_fill, step, width, enabled, on_change } = self;
        let snap_step = move |v: f32| if step > 0.0 { (v / step).round() * step } else { v };
        let key_step = if step > 0.0 { step } else { (hi - lo) / 100.0 };
        // The value under the pointer at `x`, the slider at `b`.
        let to_value = move |b: Bounds<Pixels>, x: Pixels| {
            let track = track_in(b, top);
            let t = (f32::from(x - track.left()) / f32::from(track.size.width)).clamp(0.0, 1.0);
            snap_step(lo + t * (hi - lo)).clamp(lo, hi)
        };
        let mut el = control_canvas(move |g, b, state| {
            let p = theme::palette();
            let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
            let font = style::ui_font(FontWeight::NORMAL);
            let track = track_in(b, top);
            if let Some(l) = &label {
                let r = style::label(g, b.left(), b.top() + px(m::LABEL_ROW / 2.0), style::Align::Left, l.clone(), style::PLAIN_SIZE, font.clone(), ink);
                if state.focused {
                    style::hline_above(g, r.left(), r.right(), r.bottom(), style::c(p.keyboard_navigation));
                }
            }
            if let Some(t) = &value_text {
                style::label(g, b.right(), b.top() + px(m::LABEL_ROW / 2.0), style::Align::Right, t.clone(), style::PLAIN_SIZE, font.clone(), ink);
            }
            // The bar.
            let s = g.scale_factor();
            let bar_top = px(((f32::from(track.center().y) - BAR_H / 2.0) * s).round() / s);
            let bar = Bounds::from_corners(point(b.left(), bar_top), point(b.right(), bar_top + px(BAR_H)));
            // A value outside the range (the setting's range grew elsewhere)
            // is drawn at the end it's past, never off the track.
            let t = if hi > lo { ((value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
            let thumb_x = style::snap_coord(g, track.left() + track.size.width * t);
            let color = bar_fill.unwrap_or(style::c(p.control_highlight));
            let color = if enabled { color } else { color.opacity(0.4) };
            let in_bar = style::inset(g, bar, 1.0);
            style::fill_rect(g, in_bar, style::shade(&p, p.panel_background, 1.08));
            let filled = Bounds::from_corners(in_bar.origin, point(thumb_x, in_bar.bottom()));
            let lower = {
                let c = olo_interface::Rgba::from(color);
                let k = 0.85;
                Hsla::from(olo_interface::Rgba { r: c.r * k, g: c.g * k, b: c.b * k, a: 1.0 })
            };
            if filled.size.width > px(0.0) {
                style::vgradient(g, filled, color, if enabled { lower } else { color });
            }
            style::bevel(g, bar, style::shade(&p, p.panel_background, 1.45), style::c(p.shine));
            // The thumb, a soft shadow lifting it off the bar.
            let thumb = style::snap_centred(g, point(thumb_x + px(0.5), track.center().y), THUMB_W, THUMB_H);
            let base = p.control_background;
            // (Held: dragged, wherever the pointer has wandered.)
            let pressed = *g.node_state::<bool>();
            let (tt, tb) = if pressed { (style::shade(&p, base, 1.15), style::shade(&p, base, 1.08)) } else { (style::tint(base, 0.8), style::shade(&p, base, 1.06)) };
            let line = style::hairline(g);
            g.paint_quad(fill(Bounds { origin: point(thumb.left() + line, thumb.top() + line), size: thumb.size }, style::c(p.drop_shadow)).corner_radii(px(1.0)));
            let in_thumb = style::inset(g, thumb, 1.0);
            style::vgradient(g, in_thumb, tt, tb);
            style::frame_rect(g, thumb, style::shade(&p, p.panel_background, 1.6));
            style::bevel(g, in_thumb, style::c(p.shine), style::shade(&p, base, 1.2));
            // The grip line.
            let gx = px((f32::from(thumb.center().x) * s).floor() / s);
            style::vline_left_of(g, gx, thumb.top() + px(4.0), thumb.bottom() - px(4.0), style::shade(&p, base, 1.35));
            style::vline(g, gx, thumb.top() + px(4.0), thumb.bottom() - px(4.0), style::c(p.shine));
            if let Some((min, max)) = &limits {
                let y = b.bottom() - px(m::LABEL_ROW / 2.0 - 1.0);
                style::label(g, b.left(), y, style::Align::Left, min.clone(), style::SMALL_SIZE, font.clone(), ink);
                style::label(g, b.right(), y, style::Align::Right, max.clone(), style::SMALL_SIZE, font.clone(), ink);
            }
            g.agent(agent::Node { role: "Slider", name: label.clone(), value: Some(format!("{value}").into()), bounds: b, enabled, clickable: enabled, focused: state.focused, ..Default::default() });
        })
        .paint_key(look)
        .id(SharedString::from(format!("slider-{id}")))
        .h(px(height))
        .flex_shrink_0();
        el = match width {
            Some(w) => el.w(px(w)),
            None => el.w_full(),
        };
        if !enabled {
            return el;
        }
        let (pressed, dragged, keys) = (on_change.clone(), on_change.clone(), on_change);
        el.focusable()
            .tab_stop()
            .on_mouse_down(MouseButton::Left, move |e, cx| {
                *cx.node_state::<bool>() = true;
                cx.repaint();
                let v = to_value(cx.listener_bounds(), e.position.x);
                if let Some(f) = &pressed
                    && v != value
                {
                    f(&v, cx);
                }
            })
            .on_drag_move(move |e, cx| {
                if !e.dragging() || !*cx.node_state::<bool>() {
                    return;
                }
                let v = to_value(cx.listener_bounds(), e.position.x);
                if let Some(f) = &dragged
                    && v != value
                {
                    f(&v, cx);
                }
            })
            .on_window_mouse_up(|e, cx| {
                if e.button == MouseButton::Left && std::mem::take(cx.node_state::<bool>()) {
                    cx.repaint();
                }
            })
            .on_key_down(move |e: &KeyDownEvent, cx| {
                let v = match e.keystroke.key.as_str() {
                    "left" | "down" => value - key_step,
                    "right" | "up" => value + key_step,
                    "home" => lo,
                    "end" => hi,
                    _ => return,
                };
                let v = v.clamp(lo, hi);
                if let Some(f) = &keys
                    && v != value
                {
                    f(&v, cx);
                }
                cx.stop_propagation();
            })
    }
}
