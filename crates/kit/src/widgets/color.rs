//! BColorControl: grey, red, green and blue ramps from black, each with
//! a marker you click or drag (grey sets all three; the lane pressed in
//! keeps the drag, wherever it wanders), and Red/Green/Blue number fields
//! beside them, their text following the colour unless being typed in.
//! [`ColorWell`] is its sample.
//!
//! It keeps its fields with the window, by its id, so it's built with
//! the window to hand: `ColorControl::new("accent", rgb).on_change(..).build(cx)`.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use olo_interface::{
    Bounds, Child, Element, Hsla, InteractiveElement as _, IntoElement, MouseButton, Painter, ParentElement as _, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, div, point, px,
    rgb as hex,
};
use olo_kit_base::metrics as m;

use super::{TextControl, TextField, TextFieldEvent};
use crate::{style, theme};

/// Width of the number fields with their labels.
const FIELD_W: f32 = 96.0;
/// Three rows of fields: the control's height.
fn height() -> f32 {
    3.0 * m::CONTROL_HEIGHT + 2.0 * m::TIGHT
}

type OnChange = Rc<dyn Fn(&[u8; 3], &mut WindowCx)>;

/// What the control keeps with its window, by its id: its fields, the
/// lane a press began in, and the latest callback (the fields' typing
/// goes to it).
struct ColorState {
    fields: [Child<TextField>; 3],
    lane: Cell<Option<usize>>,
    rgb: Cell<[u8; 3]>,
    on_change: RefCell<Option<OnChange>>,
}

pub struct ColorControl {
    id: SharedString,
    rgb: [u8; 3],
    ramp_width: f32,
    enabled: bool,
    on_change: Option<OnChange>,
}

impl ColorControl {
    pub fn new(id: impl Into<SharedString>, rgb: [u8; 3]) -> Self {
        ColorControl { id: id.into(), rgb, ramp_width: 256.0 + 4.0, enabled: true, on_change: None }
    }

    /// Width of the ramps (the fields come after).
    pub fn ramp_width(mut self, w: f32) -> Self {
        self.ramp_width = w;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Told each new colour, as a marker is dragged or a number typed
    /// (`cx.msg_with(|rgb: &[u8; 3]| Msg::Colour(*rgb))`).
    pub fn on_change(mut self, f: impl Fn(&[u8; 3], &mut WindowCx) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// The control, in the window `cx` is (which keeps its fields, by
    /// its id).
    pub fn build(self, cx: &mut WindowCx) -> Element {
        let key = SharedString::from(format!("olo-color-{}", self.id));
        let state: Rc<ColorState> = cx.keyed_state(key, |cx| ColorState {
            fields: std::array::from_fn(|_| cx.new_view(|_| TextField::new().max_len(3))),
            lane: Cell::new(None),
            rgb: Cell::new([0; 3]),
            on_change: RefCell::new(None),
        });
        // A number typed is the colour's (heard once: the fields are
        // kept).
        if Rc::strong_count(&state) == 2 {
            for i in 0..3 {
                let typed = Rc::downgrade(&state);
                cx.subscribe_to(&state.fields[i], move |e: &TextFieldEvent, cx| {
                    let Some(st) = typed.upgrade() else { return };
                    if !matches!(e, TextFieldEvent::Changed) {
                        return;
                    }
                    let Ok(v) = st.fields[i].read(cx).text().trim().parse::<u32>() else { return };
                    let mut rgb = st.rgb.get();
                    rgb[i] = v.min(255) as u8;
                    let f = st.on_change.borrow().clone();
                    if rgb != st.rgb.get()
                        && let Some(f) = f
                    {
                        st.rgb.set(rgb);
                        f(&rgb, cx);
                    }
                });
            }
        }
        let ColorControl { id, rgb, ramp_width, enabled, on_change } = self;
        // The fields follow the colour unless one is being typed in.
        state.rgb.set(rgb);
        *state.on_change.borrow_mut() = on_change;
        for (i, field) in state.fields.iter().enumerate() {
            let focused = field.is_focused(cx);
            field.update(cx, |f, cx| {
                if !focused && f.text() != rgb[i].to_string() {
                    f.set_text(rgb[i].to_string(), cx);
                }
                f.set_enabled(enabled, cx);
            });
        }
        // The value under the pointer at `x`, the ramps at `b`.
        let value_at = |b: Bounds<Pixels>, x: Pixels, scale: f32| {
            let line = 2.0 * style::line_pixels(scale) / scale;
            let (left, width) = (f32::from(b.left()) + line, f32::from(b.size.width) - 2.0 * line);
            (((f32::from(x) - left) / (width - 1.0)).clamp(0.0, 1.0) * 255.0).round() as u8
        };
        let set = {
            let st = state.clone();
            move |lane: usize, v: u8, cx: &mut WindowCx| {
                let mut to = rgb;
                match lane {
                    0 => to = [v; 3],
                    l => to[l - 1] = v,
                }
                let f = st.on_change.borrow().clone();
                if to != rgb
                    && let Some(f) = f
                {
                    f(&to, cx);
                }
            }
        };
        let mut ramps = canvas(move |g, b| paint_ramps(g, b, rgb, enabled)).paint_key(style::key((rgb, enabled))).w(px(ramp_width)).h(px(height())).flex_shrink_0();
        if enabled {
            let (down, moved, up) = (state.clone(), state.clone(), state.clone());
            let (pressed, dragged) = (set.clone(), set);
            ramps = ramps
                .on_mouse_down(MouseButton::Left, move |e, cx| {
                    let b = cx.listener_bounds();
                    let lane = (0..4).find(|i| e.position.y < lanes(b, cx.scale_factor())[*i].bottom()).unwrap_or(3);
                    down.lane.set(Some(lane));
                    pressed(lane, value_at(b, e.position.x, cx.scale_factor()), cx);
                })
                .on_window_mouse_move(move |e, cx| {
                    if !e.dragging() {
                        return;
                    }
                    if let Some(lane) = moved.lane.get() {
                        dragged(lane, value_at(cx.listener_bounds(), e.position.x, cx.scale_factor()), cx);
                    }
                })
                .on_window_mouse_up(move |_, _| up.lane.set(None));
        }
        let numbers = div().flex().flex_col().gap(px(m::TIGHT)).w(px(FIELD_W)).children(
            state.fields.iter().zip(["Red:", "Green:", "Blue:"]).map(|(f, label)| TextControl::new(cx, f).label(label).divider(FIELD_W - 40.0).width(FIELD_W).into_element()).collect::<Vec<_>>(),
        );
        let _ = id;
        div().flex().flex_row().gap(px(m::SPACING)).child(ramps).child(numbers)
    }
}

/// The four lanes in the ramps' bounds `b`: whole device pixels tall,
/// the last taking what's left.
fn lanes(b: Bounds<Pixels>, s: f32) -> [Bounds<Pixels>; 4] {
    let line = 2.0 * style::line_pixels(s) / s;
    let snap = |v: Pixels| px((f32::from(v) * s).round() / s);
    let inner = Bounds::from_corners(point(snap(b.left()) + px(line), snap(b.top()) + px(line)), point(snap(b.right()) - px(line), snap(b.bottom()) - px(line)));
    let row = (f32::from(inner.size.height) * s / 4.0).floor() / s;
    std::array::from_fn(|i| {
        let top = inner.top() + px(i as f32 * row);
        let bottom = if i == 3 { inner.bottom() } else { top + px(row) };
        Bounds::from_corners(point(inner.left(), top), point(inner.right(), bottom))
    })
}

fn paint_ramps(g: &mut Painter, b: Bounds<Pixels>, rgb: [u8; 3], enabled: bool) {
    let p = theme::palette();
    let s = g.scale_factor();
    let lanes = lanes(b, s);
    let ends: [Hsla; 4] = [hex(0xffffff).into(), hex(0xff0000).into(), hex(0x00ff00).into(), hex(0x0000ff).into()];
    let black: Hsla = hex(0x000000).into();
    for (i, lane) in lanes.iter().enumerate() {
        let end = if enabled { ends[i] } else { ends[i].opacity(0.4) };
        style::hgradient(g, *lane, black, end);
        // The marker: where the value is (grey only when it is one).
        let v = match i {
            0 if rgb[0] == rgb[1] && rgb[1] == rgb[2] => Some(rgb[0]),
            0 => None,
            l => Some(rgb[l - 1]),
        };
        if let Some(v) = v {
            // A black line with a white one either side, on device pixels.
            let line = style::hairline(g);
            let x = px(((f32::from(lane.left()) + v as f32 / 255.0 * f32::from(lane.size.width - line)) * s).floor() / s);
            let mark = Bounds::from_corners(point(x - line, lane.top()), point(x + line * 2.0, lane.bottom()));
            style::fill_rect(g, mark, hex(0xffffff).into());
            style::vline(g, x, lane.top(), lane.bottom(), black);
        }
    }
    style::sunken(g, b, &p, false);
    g.agent(agent::Node {
        role: "ColorWell",
        name: Some("Colour".into()),
        value: Some(format!("{} {} {}", rgb[0], rgb[1], rgb[2]).into()),
        bounds: b,
        enabled,
        clickable: enabled,
        ..Default::default()
    });
}

/// A colour swatch in a sunken well (BColorControl's sample).
pub struct ColorWell {
    color: Hsla,
    size: f32,
}

impl ColorWell {
    /// `size` points square.
    pub fn new(color: Hsla, size: f32) -> Self {
        ColorWell { color, size }
    }
}

impl IntoElement for ColorWell {
    fn into_element(self) -> Element {
        let color = self.color;
        canvas(move |g, b| style::paint_swatch(g, b, color, &theme::palette())).paint_key(style::key(style::ink_key(Some(color)))).w(px(self.size)).h(px(self.size)).flex_shrink_0()
    }
}

/// The control's height (three rows of fields), for a well beside it.
pub fn color_control_height() -> f32 {
    height()
}
