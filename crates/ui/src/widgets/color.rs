//! BColorControl: grey, red, green and blue
//! ramps from black, each with a marker you click or drag (grey sets all
//! three; the lane pressed in keeps the drag, wherever it wanders), and
//! Red/Green/Blue number fields beside them, their text following the
//! colour unless being typed in. [`ColorWell`] is its sample.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Entity, HitboxBehavior, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, RenderOnce, SharedString,
    Styled as _, Window, canvas, div, point, px, rgb as hex,
};
use olo_kit_base::metrics as m;

use super::{TextControl, TextField, TextFieldEvent};
use crate::{agent, style, theme};

/// Width of the number fields with their labels.
const FIELD_W: f32 = 96.0;
/// Three rows of fields: the control's height.
fn height() -> f32 {
    3.0 * m::CONTROL_HEIGHT + 2.0 * m::TIGHT
}

type OnChange = Rc<dyn Fn([u8; 3], &mut Window, &mut App)>;

/// The control's state from frame to frame: its fields, the lane a press
/// began in, and the latest callback (the fields' typing goes to it).
struct ColorState {
    fields: [Entity<TextField>; 3],
    lane: Option<usize>,
    rgb: [u8; 3],
    on_change: Option<OnChange>,
}

#[derive(IntoElement)]
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

    /// Told each new colour, as a marker is dragged or a number typed.
    pub fn on_change(mut self, f: impl Fn([u8; 3], &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for ColorControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state: Entity<ColorState> = window.use_keyed_state(SharedString::from(format!("olo-color-{}", self.id)), cx, |_, cx| {
            let fields = std::array::from_fn(|_| cx.new(|cx| TextField::new(cx).max_len(3)));
            for (i, f) in fields.iter().enumerate() {
                cx.subscribe(f, move |st: &mut ColorState, field, e: &TextFieldEvent, cx| {
                    if !matches!(e, TextFieldEvent::Changed) {
                        return;
                    }
                    let Ok(v) = field.read(cx).text().trim().parse::<u32>() else { return };
                    let mut rgb = st.rgb;
                    rgb[i] = v.min(255) as u8;
                    if rgb != st.rgb
                        && let Some(f) = st.on_change.clone()
                    {
                        st.rgb = rgb;
                        // (The window is the one the field is in: its
                        // callback runs after this update.)
                        cx.defer(move |cx| {
                            if let Some(w) = cx.active_window() {
                                let _ = w.update(cx, |_, window, cx| f(rgb, window, cx));
                            }
                        });
                    }
                })
                .detach();
            }
            ColorState { fields, lane: None, rgb: [0; 3], on_change: None }
        });
        let ColorControl { id, rgb, ramp_width, enabled, on_change } = self;
        // The fields follow the colour unless one is being typed in.
        let fields = state.update(cx, |st, cx| {
            st.rgb = rgb;
            st.on_change = on_change.clone();
            for (i, f) in st.fields.iter().enumerate() {
                let focused = f.read(cx).is_focused(window);
                f.update(cx, |f, cx| {
                    if !focused && f.text() != rgb[i].to_string() {
                        f.set_text(rgb[i].to_string(), cx);
                    }
                    f.set_enabled(enabled, cx);
                });
            }
            st.fields.clone()
        });
        let paint_state = state.clone();
        let ramps = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |b, hitbox, window, cx| {
                let p = theme::palette(cx);
                let inner = style::inset(window, b, 2.0);
                // Four lanes whole device pixels tall, the last taking what's left.
                let s = window.scale_factor();
                let row = (f32::from(inner.size.height) * s / 4.0).floor() / s;
                let lanes: [Bounds<Pixels>; 4] = std::array::from_fn(|i| {
                    let top = inner.top() + px(i as f32 * row);
                    let bottom = if i == 3 { inner.bottom() } else { top + px(row) };
                    Bounds::from_corners(point(inner.left(), top), point(inner.right(), bottom))
                });
                let ends: [Hsla; 4] = [hex(0xffffff).into(), hex(0xff0000).into(), hex(0x00ff00).into(), hex(0x0000ff).into()];
                let black: Hsla = hex(0x000000).into();
                for (i, lane) in lanes.iter().enumerate() {
                    let end = if enabled { ends[i] } else { ends[i].opacity(0.4) };
                    style::hgradient(window, *lane, black, end);
                    // The marker: where the value is (grey only when it is one).
                    let v = match i {
                        0 if rgb[0] == rgb[1] && rgb[1] == rgb[2] => Some(rgb[0]),
                        0 => None,
                        l => Some(rgb[l - 1]),
                    };
                    if let Some(v) = v {
                        // A black line with a white one either side, on device pixels.
                        let line = style::hairline(window);
                        let x = px(((f32::from(lane.left()) + v as f32 / 255.0 * f32::from(lane.size.width - line)) * s).floor() / s);
                        let mark = Bounds::from_corners(point(x - line, lane.top()), point(x + line * 2.0, lane.bottom()));
                        style::fill_rect(window, mark, hex(0xffffff).into());
                        style::vline(window, x, lane.top(), lane.bottom(), black);
                    }
                }
                style::sunken(window, b, &p, false);
                agent::record(
                    window,
                    cx,
                    agent::Node {
                        role: "ColorWell",
                        name: Some("Colour".into()),
                        value: Some(format!("{} {} {}", rgb[0], rgb[1], rgb[2]).into()),
                        bounds: b,
                        enabled,
                        clickable: enabled,
                        ..Default::default()
                    },
                );
                if !enabled {
                    return;
                }
                let value_at = move |x: Pixels| ((f32::from(x - inner.left()) / (f32::from(inner.size.width) - 1.0)).clamp(0.0, 1.0) * 255.0).round() as u8;
                let set = move |lane: usize, x: Pixels, st: &Entity<ColorState>, window: &mut Window, cx: &mut App| {
                    let v = value_at(x);
                    let mut to = rgb;
                    match lane {
                        0 => to = [v; 3],
                        l => to[l - 1] = v,
                    }
                    let f = st.read(cx).on_change.clone();
                    if to != rgb
                        && let Some(f) = f
                    {
                        f(to, window, cx);
                    }
                };
                let (st, hb) = (paint_state.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left || !hb.is_hovered(window) {
                        return;
                    }
                    let lane = lanes.iter().position(|r| e.position.y < r.bottom()).unwrap_or(3);
                    st.update(cx, |s, _| s.lane = Some(lane));
                    set(lane, e.position.x, &st, window, cx);
                });
                let st = paint_state.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() || !e.dragging() {
                        return;
                    }
                    if let Some(lane) = st.read(cx).lane {
                        set(lane, e.position.x, &st, window, cx);
                    }
                });
                let st = paint_state.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && st.read(cx).lane.is_some() {
                        st.update(cx, |s, _| s.lane = None);
                    }
                });
            },
        )
        .w(px(ramp_width))
        .h(px(height()));
        let numbers = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .w(px(FIELD_W))
            .children(fields.into_iter().zip(["Red:", "Green:", "Blue:"]).map(|(f, label)| TextControl::new(f).label(label).divider(FIELD_W - 40.0).width(FIELD_W)));
        let _ = id;
        div().flex().flex_row().gap(px(m::SPACING)).child(ramps).child(numbers)
    }
}

/// A colour swatch in a sunken well (BColorControl's sample).
#[derive(IntoElement)]
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

impl RenderOnce for ColorWell {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let color = self.color;
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, style::inset(window, b, 2.0), color);
                style::sunken(window, b, &p, false);
            },
        )
        .w(px(self.size))
        .h(px(self.size))
        .flex_shrink_0()
    }
}

/// The control's height (three rows of fields), for a well beside it.
pub fn color_control_height() -> f32 {
    height()
}
