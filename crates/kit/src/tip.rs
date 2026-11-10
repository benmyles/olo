//! Tips that may reach past their window's edges, each in a popup of its
//! own (`WindowKind::Tip`: an xdg_popup that takes no input and grabs
//! nothing), on its window's looper. One shows once the pointer has
//! rested on its widget a moment, at once if another was showing a
//! moment ago, not after a press; below the widget, else above, slid to
//! stay on screen; in the tooltip colours.
//!
//! The widget tells [`hover`] when the pointer comes and goes (its
//! `on_hover`), and [`pressed`] when it's pressed; [`retext`] changes
//! what its tip says while it shows (a live value: Deskbar's widgets),
//! resized and placed again for it.

use std::time::{Duration, Instant};

use olo_interface::{
    Bounds, Cx, Element, FontWeight, IntoElement as _, LocalWindow, ParentElement as _, Pixels, SharedString, Styled as _, TimerId, View, WindowCx, WindowId, WindowKind, WindowSpec, div, px, size,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// How long the pointer rests before a tip shows.
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

impl View for TipView {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let s = cx.scale_factor();
        div()
            .size_full()
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .bg(style::c(p.tooltip_background))
            .border(px(style::line_pixels(s) / s))
            .border_color(style::shade(&p, p.tooltip_background, 1.4))
            .text_color(style::c(p.tooltip_text))
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(olo_interface::relative(style::LINE_HEIGHT))
            .child(self.text.clone().into_element())
    }
}

/// The tip showing on a looper, the one waiting to, and when one last
/// went.
#[derive(Default)]
struct Tips {
    shown: Option<(WindowId, SharedString, LocalWindow<TipView>)>,
    waiting: Option<(WindowId, SharedString, TimerId)>,
    hidden_at: Option<Instant>,
    /// Pressed since the pointer came: no tip until it leaves.
    pressed: Option<(WindowId, SharedString)>,
}

fn tips<'a>(cx: &'a mut WindowCx) -> &'a mut Tips {
    let looper = cx.looper();
    if looper.state::<Tips>().is_none() {
        looper.set_state(Tips::default());
    }
    looper.state_mut::<Tips>().expect("set above")
}

/// `element` with a tip saying `text` (shown once the pointer has
/// rested on it, not after a press on it), by `key` (its id: what tells
/// it from the window's other tips).
pub fn on(element: Element, key: impl Into<SharedString>, text: impl Into<SharedString>) -> Element {
    use olo_interface::InteractiveElement as _;
    let (key, text) = (key.into(), text.into());
    let (rested, pressed_key) = (key.clone(), key);
    element
        .on_hover(move |hovered, cx| {
            let at = cx.listener_bounds();
            hover(cx, rested.clone(), text.clone(), at, *hovered)
        })
        .on_any_mouse_down(move |_, cx| pressed(cx, pressed_key.clone()))
}

/// The pointer came to (`hovered`) or left the widget `key` of the
/// window, whose tip says `text` and which is at `anchor` (in the
/// window).
pub fn hover(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>, anchor: Bounds<Pixels>, hovered: bool) {
    let (key, text, parent) = (key.into(), text.into(), cx.window_id());
    if !hovered {
        let tips = tips(cx);
        let waiting = tips.waiting.take_if(|(w, k, _)| *w == parent && *k == key).map(|(.., timer)| timer);
        if tips.pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
            tips.pressed = None;
        }
        let shown = tips.shown.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key);
        if let Some(timer) = waiting {
            cx.looper().cancel(timer);
        }
        if shown {
            hide(cx);
        }
        return;
    }
    let tips_now = tips(cx);
    if tips_now.shown.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key) || text.is_empty() {
        return;
    }
    let again = tips_now.shown.is_some() || tips_now.hidden_at.is_some_and(|t| t.elapsed() < AGAIN);
    hide(cx);
    if again {
        show(cx, key, text, anchor);
        return;
    }
    if let Some((.., timer)) = tips(cx).waiting.take() {
        cx.looper().cancel(timer);
    }
    let (k, t) = (key.clone(), text);
    let timer = cx.looper().after(DELAY, move |looper| {
        let waiting = looper.state_mut::<Tips>().and_then(|tips| tips.waiting.take_if(|(w, wk, _)| *w == parent && *wk == k));
        if waiting.is_some() {
            olo_interface::with_window_on(looper, parent, |cx| show(cx, k, t, anchor));
        }
    });
    tips(cx).waiting = Some((parent, key, timer));
}

/// The tip of widget `key` of the window now, without the pause (⌘ held
/// over cut text, `cut`).
pub fn show_now(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>, anchor: Bounds<Pixels>) {
    let (key, text, parent) = (key.into(), text.into(), cx.window_id());
    let tips = tips(cx);
    let waiting = tips.waiting.take().map(|(.., timer)| timer);
    tips.pressed = None;
    let showing = tips.shown.as_ref().is_some_and(|(w, k, _)| *w == parent && *k == key);
    if let Some(timer) = waiting {
        cx.looper().cancel(timer);
    }
    if showing || text.is_empty() {
        return;
    }
    hide(cx);
    show(cx, key, text, anchor);
}

/// The widget `key` of the window was pressed: its tip goes, and doesn't
/// come back until the pointer leaves it.
pub fn pressed(cx: &mut WindowCx, key: impl Into<SharedString>) {
    let parent = cx.window_id();
    let tips = tips(cx);
    let waiting = tips.waiting.take().map(|(.., timer)| timer);
    tips.pressed = Some((parent, key.into()));
    if let Some(timer) = waiting {
        cx.looper().cancel(timer);
    }
    hide(cx);
}

/// The tip of widget `key` of the window says `text` from now: if it's
/// showing, drawn again, resized and placed again for it (else nothing:
/// [`hover`] takes the text as it shows).
pub fn retext(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>) {
    let (key, text, parent) = (key.into(), text.into(), cx.window_id());
    let Some(tip) = tips(cx).shown.as_ref().filter(|(w, k, _)| *w == parent && *k == key).map(|(_, _, t)| *t) else { return };
    if text.is_empty() {
        hide(cx);
        return;
    }
    let (w, h) = tip_size(&text);
    tip.update(cx, |v, cx| {
        if v.text != text {
            v.text = text;
            cx.resize_window(size(px(w), px(h)));
            cx.invalidate();
        }
    });
}

/// Puts the tip showing away.
pub fn hide(cx: &mut WindowCx) {
    let tips = tips(cx);
    if let Some((_, _, tip)) = tips.shown.take() {
        tips.hidden_at = Some(Instant::now());
        tip.close(cx);
    }
}

fn show(cx: &mut WindowCx, key: SharedString, text: SharedString, anchor: Bounds<Pixels>) {
    let parent = cx.window_id();
    if tips(cx).pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
        return;
    }
    let (w, h) = tip_size(&text);
    let spec = WindowSpec { kind: WindowKind::Tip { parent, anchor }, focus: false, ..WindowSpec::new("", w, h) };
    if let Some(tip) = cx.open_window_here(spec, move |_| TipView { text }) {
        tips(cx).shown = Some((parent, key, tip));
    }
}

/// The height of `text` wrapped to `width` in the plain font, for sizing
/// a window before it opens.
pub fn paragraph_height(text: &str, width: f32) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let mut wrapper = olo_interface::text_system().line_wrapper(font, px(style::PLAIN_SIZE));
    let lines: usize = text.split('\n').map(|line| 1 + wrapper.wrap_line(&[olo_interface::LineFragment::text(line)], px(width)).count()).sum();
    (lines as f32 * style::PLAIN_SIZE * style::LINE_HEIGHT).ceil()
}

/// A tip's size for `text`: one line as wide as it is, up to
/// [`MAX_WIDTH`], wrapped past it; on whole points.
fn tip_size(text: &str) -> (f32, f32) {
    let font = style::ui_font(FontWeight::NORMAL);
    // (Shaped as it's drawn: a character its font hasn't, a menu's ⌘,
    // comes from another and still has its width. By its characters'
    // advances in the one font, "Open (⌘O)" came out a ⌘ too narrow,
    // and wrapped.)
    let one_line = text.lines().map(|l| f32::from(style::text_width(l.to_owned(), style::PLAIN_SIZE, font.clone()))).fold(0.0, f32::max);
    // (A hairline frame each side; two points: whole device pixels.)
    let frame = 2.0;
    let inner = one_line.ceil().min(MAX_WIDTH - 2.0 * PAD_X - frame);
    let text_h = paragraph_height(text, inner);
    ((inner + 2.0 * PAD_X + frame).ceil(), (text_h + 2.0 * PAD_Y + frame).ceil())
}
