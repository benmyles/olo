//! Tips that may reach past their window's edges, each in a popup of its
//! own (`WindowKind::Tip`: an xdg_popup that grabs nothing), on its
//! window's looper. One shows once the pointer has rested on its widget
//! a moment, at once if another was showing a moment ago, not after a
//! press; below the widget, else above, slid to stay on screen; in the
//! tooltip colours. A tip of words takes no input.
//!
//! The widget tells [`hover`] when the pointer comes and goes (its
//! `on_hover`), and [`pressed`] when it's pressed; [`retext`] changes
//! what its tip says while it shows (a live value), resized and placed
//! again for it.
//!
//! A tip can show a [`Card`] instead of words ([`hover_card`],
//! [`recard`]): a title and its value, a drawing, rows with levels, as
//! Deskbar's widgets show what they measure. It's sized before it opens
//! ([`Card::size`]), and agents read it in words ([`Card::text`]). A card
//! takes the pointer (Ben, 2026-11-14: "i should be able to move the
//! cursor anywhere in the tip card without the tip card disappearing --
//! it should require moving out of the tip card for it to go away"): it
//! stays a moment after the pointer leaves its widget ([`LINGER`]),
//! for as long as the pointer is on it, and goes a moment after it
//! leaves it ([`LEAVE_CARD`]) unless it's back on the widget. So it must
//! never open over other widgets (they couldn't be reached past it):
//! [`hover_card_beside`] opens it beside a row (Deskbar's, beside the
//! bar), and widgets the pointer crosses on its way there show theirs
//! only if it rests on them ([`SWITCH`]); others take over at once.
//!
//! Things with tips may be inside one another (a widget with a card, and
//! in it a label cut short, whose whole text is its tip): the pointer is
//! on both at once, and a single tip shows. Every one the pointer is on
//! is remembered ([`Tips::over`]) until it leaves it. A card of what's
//! around stands for all of it: words of something inside it don't take
//! its place, showing or waiting. And when the pointer leaves the inner
//! thing while still on the outer, the outer's tip waits to show again
//! (`resume`). It didn't: Deskbar's Token Pool widgets, whose names are
//! cut short, showed no card when the pointer came down across the name
//! (the name's tip took the card's wait, the pointer moved off the name
//! onto the bars under it, and nothing was waiting any more: Ben,
//! 2026-11-17, "sometimes when i hover over the widgets … the popover
//! doesn't appear and i have to like move the mouse away and back").
//!
//! A tip the compositor put away (its popup gone) is forgotten the next
//! time the pointer comes: it would otherwise stand for one still
//! showing, and the widget's tip never show again until a press.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

pub use olo_interface::TipPlace;
use olo_interface::{
    Bounds, Cx, Element, FontWeight, IntoElement as _, LocalWindow, ParentElement as _, Pixels, SharedString, Styled as _, TimerId, View, WindowCx, WindowId, WindowKind, WindowSpec, div, px, size,
};
use olo_kit_base::metrics as m;

use crate::{card::Card, style, theme};

/// How long the pointer rests before a tip shows, in milliseconds: the
/// desktop's setting (`Settings::tip_delay_ms`, Preferences ▸ Mouse &
/// Touchpad), followed with the look (`theme`'s watch of the settings);
/// its default until that has been heard, and outside Olo.
static DELAY_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(olo_protocol::TIP_DELAY_MS);

/// The wait is `ms` from now (the desktop's setting, as it is or as it
/// was just changed), no more than the most it may be.
pub fn set_delay(ms: u32) {
    DELAY_MS.store(ms.min(olo_protocol::TIP_DELAY_MOST_MS), std::sync::atomic::Ordering::Relaxed);
}

/// How long the pointer rests before a tip shows.
fn delay() -> Duration {
    Duration::from_millis(u64::from(DELAY_MS.load(std::sync::atomic::Ordering::Relaxed)))
}
/// A tip hidden this recently: the next shows at once.
const AGAIN: Duration = Duration::from_millis(500);
/// The widest a tip grows before it wraps.
const MAX_WIDTH: f32 = 360.0;
/// The most a text tip says, past which it's cut short with "…". A tip
/// is a glance, and the whole of a cut text can be long (a file's line):
/// wrapped at `MAX_WIDTH` it had no limit, a tip as tall as the text
/// (Tracker went down on a popup that width and 23,750 points tall, more
/// than the GPU makes a surface for, 2026-11-14).
const MAX_LINES: usize = 12;
const MAX_CHARS: usize = 600;
const PAD_X: f32 = m::TIGHT + 1.0;
const PAD_Y: f32 = 2.0;

/// What a tip shows: words, or a card.
#[derive(Clone)]
enum Body {
    Text(SharedString),
    Card(Rc<Card>),
}

impl Body {
    /// Words, as many as a tip says ([`MAX_LINES`], [`MAX_CHARS`]).
    fn text(t: SharedString) -> Body {
        Body::Text(clipped(t))
    }

    fn is_empty(&self) -> bool {
        matches!(self, Body::Text(t) if t.is_empty())
    }

    /// Its tip's size (points).
    fn size(&self) -> (f32, f32) {
        match self {
            Body::Text(t) => tip_size(t),
            Body::Card(c) => c.size(),
        }
    }

    fn same(&self, other: &Body) -> bool {
        match (self, other) {
            (Body::Text(a), Body::Text(b)) => a == b,
            (Body::Card(a), Body::Card(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

struct TipView {
    body: Body,
}

impl View for TipView {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let text = match &self.body {
            Body::Text(t) => t.clone(),
            // (The pointer on it keeps it: `card_hovered`.)
            Body::Card(card) => {
                use olo_interface::InteractiveElement as _;
                let card = card.clone();
                return div().id("tip-card").size_full().on_hover(|on: &bool, cx| card_hovered(cx, *on)).child(olo_interface::canvas(move |g, b| card.paint(g, b)).size_full()).into_element();
            }
        };
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
            .child(text.into_element())
    }
}

/// Where a tip opens: below what it's about (else above), or beside
/// `at` (Deskbar's cards: beside the bar, level with the widget).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    at: Bounds<Pixels>,
    side: TipPlace,
    /// It always waits for the pointer to rest on it ([`on_row`]).
    rests: bool,
}

/// The tip showing.
struct Shown {
    /// The window it's of, and its widget there.
    window: WindowId,
    key: SharedString,
    tip: LocalWindow<TipView>,
    /// A card: the pointer may move onto it, and it stays while it's on
    /// it.
    card: bool,
    /// Its widget, and the side it opened at (which way the pointer
    /// goes to reach it).
    anchor: Bounds<Pixels>,
    side: TipPlace,
}

/// A tip waiting to show.
struct Waiting {
    window: WindowId,
    key: SharedString,
    timer: TimerId,
    /// What it will say (a reading that comes while it waits changes it,
    /// `rebody`).
    body: Body,
    anchor: Bounds<Pixels>,
    place: Place,
}

/// The tip showing on a looper, the one waiting to, and when one last
/// went.
#[derive(Default)]
struct Tips {
    shown: Option<Shown>,
    waiting: Option<Waiting>,
    hidden_at: Option<Instant>,
    /// Pressed since the pointer came: no tip until it leaves.
    pressed: Option<(WindowId, SharedString)>,
    /// A card whose widget the pointer left (or the card itself): put
    /// away when this fires, unless the pointer came onto it, or back.
    lingering: Option<TimerId>,
    /// The pointer is on the card showing.
    on_card: bool,
    /// What the pointer is on that has a tip: each from its `hover` until
    /// it leaves it, the innermost last.
    over: Vec<Over>,
}

/// Something the pointer is on, and the tip it would show.
struct Over {
    window: WindowId,
    key: SharedString,
    body: Body,
    anchor: Bounds<Pixels>,
    place: Place,
}

/// The most things with tips the pointer is taken to be on at once (one
/// that never said it was left can't hold a place for good).
const OVER_MOST: usize = 8;

/// Whether `inner` lies within `outer` (to a point): a thing inside
/// another.
fn within(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    let slack = px(1.0);
    inner.left() >= outer.left() - slack && inner.right() <= outer.right() + slack && inner.top() >= outer.top() - slack && inner.bottom() <= outer.bottom() + slack
}

/// A card stays this long after the pointer leaves its widget: time to
/// move onto it.
const LINGER: Duration = Duration::from_millis(450);
/// And this long after it leaves the card: time to go back to the widget.
const LEAVE_CARD: Duration = Duration::from_millis(250);
/// While a card lingers, another widget the pointer crosses on the way to
/// it shows its own only if the pointer rests there this long.
const SWITCH: Duration = Duration::from_millis(300);

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
    on_side(element, key.into(), crate::words::of(text), TipPlace::Below, Reach::default(), false)
}

/// [`on`] for a row of a list: its tip always waits for the pointer to
/// rest on the row, the whole wait, and never shows "at once because
/// another was showing a moment ago". Controls in a bar are looked along
/// for what each is, and their tips follow the pointer once the first
/// has shown; rows are crossed on the way to something else, or read
/// one after another, and a tip that springs up under each as the
/// pointer passes covers the next rows (Ben, 2026-11-26, of the agents
/// in Rex's sidebar: "the tooltips when hovering over an agent are not
/// waiting the 500ms delay they appear immediately").
pub fn on_row(element: Element, key: impl Into<SharedString>, text: impl Into<SharedString>) -> Element {
    on_side(element, key.into(), crate::words::of(text), TipPlace::Below, Reach::default(), true)
}

/// [`on`], the tip above the element: for what is at a window's foot
/// (a status bar's marks), where a tip under it would lie on the
/// window's frame. Below it where there is no room above.
pub fn on_above(element: Element, key: impl Into<SharedString>, text: impl Into<SharedString>) -> Element {
    on_side(element, key.into(), crate::words::of(text), TipPlace::Above, Reach::default(), false)
}

/// [`on_above`], the tip placed over more than the element: one of a
/// row whose tips all open in one place (a status bar's marks: the
/// tip of each begins or ends where the row does, not where the mark
/// under the pointer is), and clear of what stands over the row.
pub fn on_above_of(element: Element, key: impl Into<SharedString>, text: impl Into<SharedString>, over: Reach) -> Element {
    on_side(element, key.into(), crate::words::of(text), TipPlace::Above, over, false)
}

/// What a tip above an element is placed over besides it
/// ([`on_above_of`]), in points: what is `before` and `after` the
/// element in its row, and how far `above` it the thing stands that
/// the tip keeps clear of (a bar's rule).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Reach {
    pub before: f32,
    pub after: f32,
    pub above: f32,
}

fn on_side(element: Element, key: SharedString, text: SharedString, side: TipPlace, over: Reach, rests: bool) -> Element {
    use olo_interface::InteractiveElement as _;
    let (rested, pressed_key) = (key.clone(), key.clone());
    element
        // (Its tip is about this key's thing: another's element in its
        // place, under a still pointer, is another to the pointer too.)
        .hover_key(key)
        .on_hover(move |hovered, cx| {
            let at = cx.listener_bounds();
            let place = Bounds::from_corners(olo_interface::point(at.left() - px(over.before), at.top() - px(over.above)), olo_interface::point(at.right() + px(over.after), at.bottom()));
            // (Above what is in the far half of its window, a tip
            // ends where that ends: begun there, most of it lay
            // outside the window.)
            let side = if side == TipPlace::Above && place.center().x > cx.viewport_size().width / 2.0 { TipPlace::AboveEnd } else { side };
            hover_body(cx, rested.clone(), Body::text(text.clone()), at, Place { at: place, side, rests }, *hovered)
        })
        .on_any_mouse_down(move |_, cx| pressed(cx, pressed_key.clone()))
}

/// The pointer came to (`hovered`) or left the widget `key` of the
/// window, whose tip says `text` and which is at `anchor` (in the
/// window).
pub fn hover(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>, anchor: Bounds<Pixels>, hovered: bool) {
    hover_body(cx, key.into(), Body::text(crate::words::of(text)), anchor, Place { at: anchor, side: TipPlace::Below, rests: false }, hovered);
}

/// [`hover`], the tip a card: `card` (shared, so it isn't copied as the
/// widget is drawn again), below the widget. The pointer may move onto
/// it: it stays while the pointer is on it, and goes once the pointer
/// has left it (and isn't back on the widget).
pub fn hover_card(cx: &mut WindowCx, key: impl Into<SharedString>, card: Rc<Card>, anchor: Bounds<Pixels>, hovered: bool) {
    hover_body(cx, key.into(), Body::Card(card), anchor, Place { at: anchor, side: TipPlace::Below, rests: false }, hovered);
}

/// [`hover_card`], the card opening beside `beside` (in the window) at
/// `side`, its top level with `beside`'s: Deskbar's, beside the bar
/// rather than over the widgets under the one it's about. On its way
/// there the pointer may cross other widgets: their tips wait a moment
/// ([`SWITCH`]) while the card lingers, so it isn't swapped for theirs.
#[allow(clippy::too_many_arguments)]
pub fn hover_card_beside(cx: &mut WindowCx, key: impl Into<SharedString>, card: Rc<Card>, anchor: Bounds<Pixels>, beside: Bounds<Pixels>, side: TipPlace, hovered: bool) {
    hover_body(cx, key.into(), Body::Card(card), anchor, Place { at: beside, side, rests: false }, hovered);
}

/// The key of the window's tip showing, if one is (a card the pointer
/// has left lingers: still showing).
pub fn shown(cx: &mut WindowCx) -> Option<SharedString> {
    let window = cx.window_id();
    forget_gone(cx);
    tips(cx).shown.as_ref().filter(|s| s.window == window).map(|s| s.key.clone())
}

/// The key of the tip showing on any of the looper's windows.
pub fn shown_anywhere(cx: &mut WindowCx) -> Option<SharedString> {
    forget_gone(cx);
    tips(cx).shown.as_ref().map(|s| s.key.clone())
}

/// Whether widget `b` lies on the way from `a` to a card opened at
/// `side` of it: crossed by a pointer going there.
fn on_the_way(a: Bounds<Pixels>, side: TipPlace, b: Bounds<Pixels>) -> bool {
    let across = b.top() < a.bottom() && b.bottom() > a.top();
    let along = b.left() < a.right() && b.right() > a.left();
    match side {
        TipPlace::Left => across && b.right() <= a.left() + px(1.0),
        TipPlace::Right => across && b.left() >= a.right() - px(1.0),
        TipPlace::Below => along && b.top() >= a.bottom() - px(1.0),
        TipPlace::Above | TipPlace::AboveEnd => along && b.bottom() <= a.top() + px(1.0),
    }
}

fn hover_body(cx: &mut WindowCx, key: SharedString, text: Body, anchor: Bounds<Pixels>, place: Place, hovered: bool) {
    let parent = cx.window_id();
    forget_gone(cx);
    if !hovered {
        let tips = tips(cx);
        tips.over.retain(|o| !(o.window == parent && o.key == key));
        let waiting = tips.waiting.take_if(|w| w.window == parent && w.key == key).map(|w| w.timer);
        if tips.pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
            tips.pressed = None;
        }
        let shown = tips.shown.as_ref().filter(|s| s.window == parent && s.key == key).map(|s| s.card);
        if let Some(timer) = waiting {
            cx.looper().cancel(timer);
        }
        match shown {
            // A card stays a moment, for the pointer to reach it.
            Some(true) => linger(cx, LINGER),
            Some(false) => hide(cx),
            None => {}
        }
        resume(cx, parent, shown == Some(false));
        return;
    }
    let tips_now = tips(cx);
    // The pointer is on it from now, whatever shows.
    if !text.is_empty() {
        tips_now.over.retain(|o| !(o.window == parent && o.key == key));
        if tips_now.over.len() == OVER_MOST {
            tips_now.over.remove(0);
        }
        tips_now.over.push(Over { window: parent, key: key.clone(), body: text.clone(), anchor, place });
    }
    // Back on the widget whose tip shows: it stays.
    if tips_now.shown.as_ref().is_some_and(|s| s.window == parent && s.key == key) {
        if let Some(timer) = tips_now.lingering.take() {
            cx.looper().cancel(timer);
        }
        return;
    }
    if text.is_empty() {
        return;
    }
    // Words of something inside what a card is about (showing, or
    // waiting to) while the pointer is still on that: the card stands.
    if matches!(text, Body::Text(_)) {
        let still_on = |k: &SharedString| tips_now.over.iter().any(|o| o.window == parent && o.key == *k);
        let around_shown = tips_now.shown.as_ref().is_some_and(|s| s.card && s.window == parent && tips_now.lingering.is_none() && within(s.anchor, anchor) && still_on(&s.key));
        let around_waiting = tips_now.waiting.as_ref().is_some_and(|w| matches!(w.body, Body::Card(_)) && w.window == parent && within(w.anchor, anchor) && still_on(&w.key));
        if around_shown || around_waiting {
            return;
        }
    }
    // A card lingering, this widget on the pointer's way to it: this
    // one's shows only if the pointer rests here.
    let passing = tips_now.lingering.is_some() && tips_now.shown.as_ref().is_some_and(|s| s.card && s.window == parent && on_the_way(s.anchor, s.side, anchor));
    let again = at_once(place.rests, passing, tips_now.shown.is_some(), tips_now.hidden_at.map(|t| t.elapsed()));
    if !passing {
        hide(cx);
    }
    if again {
        show(cx, key, text, anchor, place);
        return;
    }
    wait(cx, parent, key, text, anchor, place, if passing && !place.rests { SWITCH } else { delay() });
}

/// The pointer left something of `parent` and no tip shows or waits:
/// the innermost thing it is still on there waits to show its own (at
/// once if the one left was showing, `was_showing`, or one was a moment
/// ago).
fn resume(cx: &mut WindowCx, parent: WindowId, was_showing: bool) {
    let tips_now = tips(cx);
    if tips_now.shown.is_some() || tips_now.waiting.is_some() {
        return;
    }
    let Some(o) = tips_now.over.iter().rev().find(|o| o.window == parent) else { return };
    let (key, body, anchor, place) = (o.key.clone(), o.body.clone(), o.anchor, o.place);
    if tips_now.pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
        return;
    }
    if at_once(place.rests, false, was_showing, tips_now.hidden_at.map(|t| t.elapsed())) {
        show(cx, key, body, anchor, place);
        return;
    }
    wait(cx, parent, key, body, anchor, place, delay());
}

/// Whether a tip shows the moment the pointer comes to its widget, not
/// after the wait: when another is showing or was a moment ago (`shown`,
/// `hidden` how long since), unless the pointer is only passing on its
/// way to a card (`passing`), and never a row's (`rests`: [`on_row`]).
fn at_once(rests: bool, passing: bool, shown: bool, hidden: Option<Duration>) -> bool {
    !rests && !passing && (shown || hidden.is_some_and(|ago| ago < AGAIN))
}

/// `key`'s tip shows in `after`, unless the pointer leaves it first.
fn wait(cx: &mut WindowCx, parent: WindowId, key: SharedString, body: Body, anchor: Bounds<Pixels>, place: Place, after: Duration) {
    if let Some(w) = tips(cx).waiting.take() {
        cx.looper().cancel(w.timer);
    }
    let k = key.clone();
    let timer = cx.looper().after(after, move |looper| {
        let waiting = looper.state_mut::<Tips>().and_then(|tips| tips.waiting.take_if(|w| w.window == parent && w.key == k));
        if let Some(w) = waiting {
            olo_interface::with_window_on(looper, parent, |cx| {
                hide(cx);
                show(cx, w.key, w.body, w.anchor, w.place);
            });
        }
    });
    tips(cx).waiting = Some(Waiting { window: parent, key, timer, body, anchor, place });
}

/// The card showing goes in `after`, unless the pointer comes onto it
/// (or back to its widget) first.
fn linger(cx: &mut WindowCx, after: Duration) {
    if let Some(timer) = tips(cx).lingering.take() {
        cx.looper().cancel(timer);
    }
    let timer = cx.looper().after(after, |looper| {
        let Some(tips) = looper.state_mut::<Tips>() else { return };
        tips.lingering = None;
        let Some(window) = tips.shown.as_ref().filter(|_| !tips.on_card).map(|s| s.window) else { return };
        olo_interface::with_window_on(looper, window, hide);
    });
    tips(cx).lingering = Some(timer);
}

/// The pointer came onto the card showing, or left it.
fn card_hovered(cx: &mut WindowCx, on: bool) {
    let tips = tips(cx);
    tips.on_card = on;
    if on {
        if let Some(timer) = tips.lingering.take() {
            cx.looper().cancel(timer);
        }
    } else {
        linger(cx, LEAVE_CARD);
    }
}

/// The tip of widget `key` of the window now, without the pause (⌘ held
/// over cut text, `cut`).
pub fn show_now(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>, anchor: Bounds<Pixels>) {
    let (key, text, parent) = (key.into(), Body::text(crate::words::of(text)), cx.window_id());
    forget_gone(cx);
    let tips = tips(cx);
    let waiting = tips.waiting.take().map(|w| w.timer);
    tips.pressed = None;
    let showing = tips.shown.as_ref().is_some_and(|s| s.window == parent && s.key == key);
    if let Some(timer) = waiting {
        cx.looper().cancel(timer);
    }
    if showing || text.is_empty() {
        return;
    }
    hide(cx);
    show(cx, key, text, anchor, Place { at: anchor, side: TipPlace::Below, rests: false });
}

/// The widget `key` of the window was pressed: its tip goes, and doesn't
/// come back until the pointer leaves it.
pub fn pressed(cx: &mut WindowCx, key: impl Into<SharedString>) {
    let parent = cx.window_id();
    let tips = tips(cx);
    let waiting = tips.waiting.take().map(|w| w.timer);
    tips.pressed = Some((parent, key.into()));
    if let Some(timer) = waiting {
        cx.looper().cancel(timer);
    }
    hide(cx);
}

/// The tip of widget `key` of the window says `text` from now: if it's
/// showing, drawn again, resized and placed again for it; if it's
/// waiting to show, it shows that (else nothing: [`hover`] takes the
/// text as it shows).
pub fn retext(cx: &mut WindowCx, key: impl Into<SharedString>, text: impl Into<SharedString>) {
    rebody(cx, key.into(), Body::text(crate::words::of(text)));
}

/// [`retext`] for a card's tip: it shows `card` from now (resized for
/// it, if it grew or shrank).
pub fn recard(cx: &mut WindowCx, key: impl Into<SharedString>, card: Rc<Card>) {
    rebody(cx, key.into(), Body::Card(card));
}

fn rebody(cx: &mut WindowCx, key: SharedString, text: Body) {
    let parent = cx.window_id();
    forget_gone(cx);
    let tips_now = tips(cx);
    if let Some(o) = tips_now.over.iter_mut().find(|o| o.window == parent && o.key == key) {
        o.body = text.clone();
    }
    if let Some(w) = tips_now.waiting.as_mut().filter(|w| w.window == parent && w.key == key) {
        w.body = text;
        return;
    }
    let Some(tip) = tips_now.shown.as_ref().filter(|s| s.window == parent && s.key == key).map(|s| s.tip) else { return };
    if text.is_empty() {
        hide(cx);
        return;
    }
    let (w, h) = text.size();
    tip.update(cx, |v, cx| {
        if !v.body.same(&text) {
            let was = v.body.size();
            v.body = text;
            if was != (w, h) {
                cx.resize_window(size(px(w), px(h)));
            }
            cx.invalidate();
        }
    });
}

/// The tip it shows, if its popup has gone (put away by the compositor,
/// or closed some other way): forgotten.
fn forget_gone(cx: &mut WindowCx) {
    let Some(tip) = tips(cx).shown.as_ref().map(|s| s.tip) else { return };
    let gone = !tip.is_open(cx);
    if gone {
        let tips = tips(cx);
        tips.shown = None;
        tips.on_card = false;
        tips.hidden_at = Some(Instant::now());
        if let Some(timer) = tips.lingering.take() {
            cx.looper().cancel(timer);
        }
    }
}

/// Puts the tip showing away.
pub fn hide(cx: &mut WindowCx) {
    let tips = tips(cx);
    tips.on_card = false;
    let lingering = tips.lingering.take();
    if let Some(shown) = tips.shown.take() {
        tips.hidden_at = Some(Instant::now());
        shown.tip.close(cx);
    }
    if let Some(timer) = lingering {
        cx.looper().cancel(timer);
    }
}

fn show(cx: &mut WindowCx, key: SharedString, text: Body, anchor: Bounds<Pixels>, place: Place) {
    let parent = cx.window_id();
    if tips(cx).pressed.as_ref().is_some_and(|(w, k)| *w == parent && *k == key) {
        return;
    }
    let (w, h) = text.size();
    // (Named for agents: a card by its title. A card takes the pointer:
    // it may be moved onto and rested on.)
    let (title, card) = match &text {
        Body::Card(card) => (card.title.to_string(), true),
        Body::Text(_) => (String::new(), false),
    };
    let spec = WindowSpec { kind: WindowKind::Tip { parent, anchor: place.at, place: place.side, takes_pointer: card }, focus: false, ..WindowSpec::new(&title, w, h) };
    if let Some(tip) = cx.open_window_here(spec, move |_| TipView { body: text }) {
        tips(cx).shown = Some(Shown { window: parent, key, tip, card, anchor, side: place.side });
    }
}

/// `t` as a tip says it: its first [`MAX_LINES`] lines and
/// [`MAX_CHARS`] characters, "…" where it's cut.
fn clipped(t: SharedString) -> SharedString {
    let mut out = String::new();
    let (mut chars, mut cut) = (0, false);
    for (i, line) in t.split('\n').enumerate() {
        if i == MAX_LINES {
            cut = true;
            break;
        }
        if i > 0 {
            out.push('\n');
            chars += 1;
        }
        let n = line.chars().count();
        if chars + n > MAX_CHARS {
            out.extend(line.chars().take(MAX_CHARS - chars));
            cut = true;
            break;
        }
        out.push_str(line);
        chars += n;
    }
    if !cut {
        return t;
    }
    out.truncate(out.trim_end().len());
    out.push('…');
    out.into()
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
    let most = MAX_WIDTH - 2.0 * PAD_X - frame;
    let mut inner = one_line.ceil().min(most);
    // Lines are wrapped by their letters' advances, which add up to a
    // little more than the shaped line where pairs of letters are set
    // closer: a line that fits is given the point or two that takes,
    // or it was sized as two lines and drawn as one ("Connected, with
    // Olo’s typing helper" had an empty line under it, 2026-11-23).
    let mut wrapper = olo_interface::text_system().line_wrapper(font.clone(), px(style::PLAIN_SIZE));
    let mut wraps = |width: f32| {
        text.lines().any(|l| f32::from(style::text_width(l.to_owned(), style::PLAIN_SIZE, font.clone())) <= most && wrapper.wrap_line(&[olo_interface::LineFragment::text(l)], px(width)).count() > 0)
    };
    while inner < most && wraps(inner) {
        inner += 1.0;
    }
    // A tip that is wrapped is as wide as its longest line, not as
    // wide as a tip may be: a first line that ended well short of
    // that (the next word a long one, or kept whole with what follows
    // it) left a strip of empty tip at its end (Gemini's review of
    // Rex's status marks, 2026-11-23: seven points before the words
    // and forty after them).
    if inner >= most {
        let lines = paragraph_height(text, inner);
        let mut longest_at = |width: f32| {
            let mut longest: f32 = 0.0;
            for l in text.lines() {
                let mut from = 0;
                let ends: Vec<usize> = wrapper.wrap_line(&[olo_interface::LineFragment::text(l)], px(width)).map(|b| b.ix).chain([l.len()]).collect();
                for to in ends {
                    longest = longest.max(f32::from(style::text_width(l[from..to].trim_end().to_owned(), style::PLAIN_SIZE, font.clone())));
                    from = to;
                }
            }
            longest
        };
        // (Narrower, its lines may break elsewhere and be shorter
        // still: "…To install it, choose" and a menu's path under it
        // became "…To install it," and "choose" with the path, both
        // shorter. Until it is no narrower.)
        loop {
            let mut fitted = longest_at(inner).ceil().min(inner);
            // (By the advances it is wrapped by, a point or two more.)
            while fitted < inner && paragraph_height(text, fitted) > lines {
                fitted += 1.0;
            }
            if fitted >= inner {
                break;
            }
            inner = fitted;
        }
    }
    let text_h = paragraph_height(text, inner);
    ((inner + 2.0 * PAD_X + frame).ceil(), (text_h + 2.0 * PAD_Y + frame).ceil())
}

#[cfg(test)]
mod tests {
    /// A bar's control follows another's tip at once; a row of a list
    /// waits for the pointer to rest on it whatever showed a moment ago.
    #[test]
    fn a_rows_tip_always_waits() {
        use std::time::Duration;
        let a_moment = Some(Duration::from_millis(100));
        // (A control: at once while another shows, or did a moment ago.)
        assert!(super::at_once(false, false, true, None));
        assert!(super::at_once(false, false, false, a_moment));
        assert!(!super::at_once(false, false, false, Some(Duration::from_millis(600))));
        assert!(!super::at_once(false, false, false, None));
        assert!(!super::at_once(false, true, true, a_moment), "passing on the way to a card");
        // (A row: never.)
        assert!(!super::at_once(true, false, true, a_moment));
        assert!(!super::at_once(true, false, false, a_moment));
        // (The wait is the desktop's, within its most.)
        super::set_delay(250);
        assert_eq!(super::delay(), Duration::from_millis(250));
        super::set_delay(999_999);
        assert_eq!(super::delay(), Duration::from_millis(u64::from(olo_protocol::TIP_DELAY_MOST_MS)));
        super::set_delay(olo_protocol::TIP_DELAY_MS);
    }

    use super::*;

    /// A thing inside another, to a point: a widget's name in its widget.
    #[test]
    fn a_thing_within_another() {
        let b = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: olo_interface::point(px(x), px(y)), size: size(px(w), px(h)) };
        let widget = b(8.0, 182.0, 52.0, 36.0);
        assert!(within(widget, b(8.0, 184.0, 52.0, 12.0)), "its name, as wide as it is");
        assert!(within(widget, b(7.5, 182.0, 53.0, 12.0)), "to a point");
        assert!(!within(widget, b(62.0, 182.0, 52.0, 36.0)), "the widget beside it");
        assert!(!within(widget, b(8.0, 160.0, 164.0, 22.0)), "the row above");
        assert!(within(widget, widget));
    }

    /// A tip of one line that fits is one line tall: as tall as any
    /// other of one line, whatever its letters (its width is the shaped
    /// line's, which pairs of letters set closer than their advances
    /// add up to: sized by one and wrapped by the other, "Connected,
    /// with Olo’s typing helper" had room for a second line under it).
    #[test]
    fn a_tip_of_one_line_is_one_line_tall() {
        let one = tip_size("Open").1;
        for text in ["Connected, with Olo’s typing helper", "Connected", "Hasn’t answered for 4 seconds", "Bold (⌘B)", "AVAVAVAV To Ty. WAVE, Yo. fi fl ffi", "Tailscale: you@far, away"] {
            let (w, h) = tip_size(text);
            assert_eq!(h, one, "{text:?} is {w} wide and {h} tall");
        }
        // (And one too long for a line is taller, and no wider than a tip grows.)
        let (w, h) = tip_size("Connected. Olo’s typing helper was added and will be used the next time you connect.");
        assert!(h > one && w <= MAX_WIDTH, "{w} x {h}");
    }

    /// A tip that is wrapped is as wide as its longest line: a first
    /// line ending well short of the most a tip may be (the words
    /// after it kept whole) leaves no strip of empty tip after it.
    #[test]
    fn a_wrapped_tip_is_as_wide_as_its_longest_line() {
        let two = tip_size("Open\nSave").1;
        let font = style::ui_font(FontWeight::NORMAL);
        let wide = |l: &str| f32::from(style::text_width(l.to_owned(), style::PLAIN_SIZE, font.clone()));
        // (A menu's path joined with no-break spaces, as Rex's status
        // marks say it: the whole of it is on the second line. The
        // second: narrower, it breaks a word sooner, and both lines
        // are shorter than the first line was.)
        for (first, second) in [
            ("Olo’s bridge isn’t on “far”. To install it, choose", "Agents ▸ Install Olo’s Bridge on This Host."),
            ("Olo’s skill for agents isn’t on “far”. To install it,", "choose Agents ▸ Install Olo’s Skill for Agents."),
        ] {
            let path = second.rsplit_once("choose ").map_or(second, |(_, path)| path);
            let text = format!("{first} {second}").replace(path, &path.replace(' ', "\u{a0}"));
            let (w, h) = tip_size(&text);
            assert_eq!(h, two, "two lines: {w} x {h}");
            let all = wide(first).max(wide(second)) + 2.0 * PAD_X + 2.0;
            assert!(w >= all.floor() && w <= all + 3.0, "{w} wide for lines of {} and {}: {text:?}", wide(first), wide(second));
        }
    }

    #[test]
    fn a_tip_says_only_so_much() {
        // Short: as it is.
        assert_eq!(clipped("Open (⌘O)".into()).as_ref(), "Open (⌘O)");
        // A file's worth: cut short, whatever its shape.
        let long: String = "word ".repeat(20_000);
        let c = clipped(long.into());
        assert!(c.chars().count() <= MAX_CHARS + 1 && c.ends_with('…'));
        let many: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        let c = clipped(many.into());
        assert_eq!(c.lines().count(), MAX_LINES);
        assert!(c.ends_with('…'));
    }
}
