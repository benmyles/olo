//! Tips that may reach past their window's edges, each in a popup of its
//! own (`WindowKind::Tip`: an xdg_popup that takes no input and grabs
//! nothing), for windows too small to hold them: Deskbar's tray, whose
//! window is narrower than what its icons say. As the egui kit's `tip`:
//! one shows once the pointer has rested on its widget a moment, at once
//! if another was showing a moment ago, not after a press; below the
//! widget, else above, slid to stay on screen; in the tooltip colours.
//! (`widgets::tooltip` is GPUI's own, inside its window, for the rest.)
//!
//! The widget tells [`hover`] when the pointer comes and goes (its
//! `on_hover`), and [`pressed`] when it's pressed.

use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FontWeight, IntoElement, ParentElement as _, Pixels, Render, SharedString, Styled as _, Task, Window, WindowBounds, WindowHandle, WindowKind, div, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// How long the pointer rests before a tip shows (egui's tooltip delay).
const DELAY: Duration = Duration::from_millis(500);
/// A tip hidden this recently: the next shows at once.
const AGAIN: Duration = Duration::from_millis(500);
/// The widest a tip grows before it wraps.
const MAX_WIDTH: f32 = 360.0;
const PAD_X: f32 = m::TIGHT + 1.0;
const PAD_Y: f32 = 2.0;

struct TipView {
    text: SharedString,
}

impl Render for TipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        div()
            .size_full()
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .bg(style::c(p.tooltip_background))
            .border(style::hairline(window))
            .border_color(style::shade(&p, p.tooltip_background, 1.4))
            .text_color(style::c(p.tooltip_text))
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(gpui::relative(style::LINE_HEIGHT))
            .child(self.text.clone())
    }
}

/// The tip showing, the one waiting to, and when one last went.
#[derive(Default)]
struct Tips {
    shown: Option<(AnyWindowHandle, SharedString, WindowHandle<TipView>)>,
    waiting: Option<(AnyWindowHandle, SharedString, Task<()>)>,
    hidden_at: Option<Instant>,
    /// Pressed since the pointer came: no tip until it leaves.
    pressed: Option<(AnyWindowHandle, SharedString)>,
}

impl gpui::Global for Tips {}

/// The pointer came to (`hovered`) or left the widget `key` of `window`,
/// whose tip says `text` and which is at `anchor` (in the window).
pub fn hover(window: &Window, cx: &mut App, key: impl Into<SharedString>, text: impl Into<SharedString>, anchor: Bounds<Pixels>, hovered: bool) {
    let (key, text, parent) = (key.into(), text.into(), window.window_handle());
    let tips = cx.default_global::<Tips>();
    if !hovered {
        if tips.waiting.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key) {
            tips.waiting = None;
        }
        if tips.pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
            tips.pressed = None;
        }
        if tips.shown.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key) {
            hide(cx);
        }
        return;
    }
    if tips.shown.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key) || text.is_empty() {
        return;
    }
    let again = tips.shown.is_some() || tips.hidden_at.is_some_and(|t| t.elapsed() < AGAIN);
    hide(cx);
    if again {
        show(cx, parent, key, text, anchor);
        return;
    }
    let (k, t) = (key.clone(), text);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(DELAY).await;
        let _ = cx.update(|cx| {
            let tips = cx.default_global::<Tips>();
            if tips.waiting.as_ref().is_some_and(|(w, wk, _)| *w == parent && *wk == k) {
                tips.waiting = None;
                show(cx, parent, k, t, anchor);
            }
        });
    });
    cx.default_global::<Tips>().waiting = Some((parent, key, task));
}

/// The widget `key` of `window` was pressed: its tip goes, and doesn't
/// come back until the pointer leaves it.
pub fn pressed(window: &Window, cx: &mut App, key: impl Into<SharedString>) {
    let tips = cx.default_global::<Tips>();
    tips.waiting = None;
    tips.pressed = Some((window.window_handle(), key.into()));
    hide(cx);
}

/// Puts the tip showing away.
pub fn hide(cx: &mut App) {
    let tips = cx.default_global::<Tips>();
    if let Some((_, _, tip)) = tips.shown.take() {
        tips.hidden_at = Some(Instant::now());
        let _ = tip.update(cx, |_, window, _| window.remove_window());
    }
}

fn show(cx: &mut App, parent: AnyWindowHandle, key: SharedString, text: SharedString, anchor: Bounds<Pixels>) {
    if cx.default_global::<Tips>().pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
        return;
    }
    let (w, h) = tip_size(cx, &text);
    let options = gpui::WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(w), px(h)) })),
        kind: WindowKind::Tip { parent, anchor },
        app_id: Some(crate::app::app_id(cx)),
        focus: false,
        ..Default::default()
    };
    if let Ok(tip) = cx.open_window(options, move |_, cx| cx.new(|_| TipView { text })) {
        cx.default_global::<Tips>().shown = Some((parent, key, tip));
    }
}

/// A tip's size for `text`: one line as wide as it is, up to
/// [`MAX_WIDTH`], wrapped past it; on whole points.
fn tip_size(cx: &App, text: &str) -> (f32, f32) {
    let font = style::ui_font(FontWeight::NORMAL);
    let one_line = text.lines().map(|l| style::text_width_app(cx, l, style::PLAIN_SIZE, &font)).fold(0.0, f32::max);
    // (A hairline frame each side; two points: whole device pixels.)
    let frame = 2.0;
    let inner = one_line.ceil().min(MAX_WIDTH - 2.0 * PAD_X - frame);
    let text_h = crate::widgets::paragraph_height(cx, text, inner);
    ((inner + 2.0 * PAD_X + frame).ceil(), (text_h + 2.0 * PAD_Y + frame).ceil())
}
