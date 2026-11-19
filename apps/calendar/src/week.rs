//! A week's days side by side, or one day: the days' names over what's
//! on for a whole day (bars across their days), and under that the hours,
//! each event where its time puts it and as tall as it lasts, side by
//! side where they overlap, the present a line across today.

use jiff::civil::Date;
use olo_kit::{
    Bounds, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Styled as _, agent, div, point,
    px, size, style, theme,
    widgets::{ScrollView, scroll::SCROLL_BAR},
};

use crate::{Calendar, Drag, Key, Mode, Msg, layout, words};

/// An hour's height.
pub const HOUR: f32 = 48.0;
/// The hours' names' column.
const GUTTER: f32 = 56.0;
/// The days' names' row.
const NAMES: f32 = 28.0;
/// A whole-day event's row and how tall it is in it (the month's).
const LANE: f32 = 17.0;
const BAR: f32 = 15.0;
/// The most rows of whole-day events shown.
const MOST: usize = 4;
const EDGE: f32 = 3.0;
const PAD: f32 = 4.0;
/// An event dragged among the hours goes by quarter hours, once the
/// pointer has gone this far from where it pressed.
const STEP: i32 = 15;
pub const DRAG_AFTER: f32 = 4.0;
/// An event's bottom edge, to drag its end by: this much of it (less of
/// a short one: a third).
const GRIP: f32 = 5.0;

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Event(Key),
    Day(Date),
}

#[derive(Default)]
struct State {
    parts: Vec<(Bounds<Pixels>, Part)>,
    /// Where the drawing's corner was when they were noted.
    origin: olo_kit::Point<Pixels>,
    /// The hours' days: the columns' edges, and the first day (for a
    /// drag: which day and minute the pointer is over).
    cols: Vec<Pixels>,
    first: Option<Date>,
    /// The events' bottom edges that their ends are dragged by, each
    /// with its day.
    grips: Vec<(Bounds<Pixels>, Key, Date)>,
}

/// The pointer moved with an event pressed: where the event would go
/// now (the day under the pointer, the quarter hour its top is nearest),
/// drawn again only when that changes.
fn dragged(v: &mut Calendar, e: &MouseMoveEvent, cx: &mut Cx<Calendar>) {
    let Some(drag) = &v.drag else { return };
    let now = cx.listener_bounds();
    let state = cx.node_state::<State>();
    let at = crate::view::in_drawing(e.position, state.origin, now);
    let far = f32::from(at.x - drag.from.x).abs().max(f32::from(at.y - drag.from.y).abs()) >= DRAG_AFTER;
    if drag.to.is_none() && !far {
        return;
    }
    let (Some(first), true) = (state.first, state.cols.len() >= 2) else { return };
    let top = f32::from(at.y - state.origin.y) / HOUR * 60.0 - drag.grab;
    let (day, start) = match drag.end {
        // Its end: on its own day, where the pointer is down it.
        Some(day) => (day, quarter_end(top, if day == drag.was.0 { drag.was.1 } else { 0 })),
        None => {
            let col = state.cols.windows(2).position(|c| at.x < c[1]).unwrap_or(state.cols.len() - 2);
            (layout::day_of(layout::day_number(first) + col as i32), quarter(top))
        }
    };
    if drag.to != Some((day, start)) {
        if let Some(d) = &mut v.drag {
            d.to = Some((day, start));
        }
        cx.invalidate();
    }
}

/// The pointer moved with an event of the whole-day strip pressed: the
/// day it's over.
fn day_dragged(v: &mut Calendar, e: &MouseMoveEvent, cx: &mut Cx<Calendar>) {
    if v.day_drag.is_none() {
        return;
    }
    let now = cx.listener_bounds();
    let state = cx.node_state::<State>();
    let at = crate::view::in_drawing(e.position, state.origin, now);
    let (Some(first), true) = (state.first, state.cols.len() >= 2) else { return };
    let col = state.cols.windows(2).position(|c| at.x < c[1]).unwrap_or(state.cols.len() - 2);
    v.day_drag_over(at, layout::day_of(layout::day_number(first) + col as i32), cx);
}

/// The start a dragged event takes with its top `minutes` into the day:
/// the nearest quarter hour, within the day.
fn quarter(minutes: f32) -> i32 {
    (((minutes / STEP as f32).round() as i32) * STEP).clamp(0, 24 * 60 - STEP)
}

/// The end a dragged end takes with the pointer `minutes` into its day:
/// the nearest quarter hour, a quarter hour after `start` (where the
/// event starts on that day) at the least, midnight at the most.
fn quarter_end(minutes: f32, start: i32) -> i32 {
    (((minutes / STEP as f32).round() as i32) * STEP).clamp(start + STEP, 24 * 60)
}

/// The time `minutes` into `day` (1440: the next day's first).
fn time_on(day: Date, minutes: i32) -> jiff::civil::DateTime {
    if minutes >= 24 * 60 { layout::day_of(layout::day_number(day) + 1).at(0, 0, 0, 0) } else { day.at((minutes / 60) as i8, (minutes % 60) as i8, 0, 0) }
}

/// The button let go: an event dragged somewhere else goes there; one
/// whose end was dragged ends there.
fn dropped(v: &mut Calendar, _: &MouseUpEvent, cx: &mut Cx<Calendar>) {
    let Some(drag) = v.drag.take() else { return };
    let Some((day, start)) = drag.to else { return };
    cx.invalidate();
    if drag.end.is_some() {
        // (Where it ended already: nothing; `stretch_event` sees to it.)
        cx.send(Msg::Stretch(drag.key, time_on(day, start)));
        return;
    }
    if (day, start) != drag.was {
        cx.send(Msg::Move(drag.key, day.at((start / 60) as i8, (start % 60) as i8, 0, 0)));
    }
}

/// The days it shows: the first, and how many.
fn days(v: &Calendar) -> (Date, usize) {
    match v.settings.mode {
        Mode::Day => (v.cursor, 1),
        _ => (layout::week_of(v.cursor, v.week_start()), 7),
    }
}

/// The columns' edges across `b`, right of the hours' names.
fn columns(g: &olo_kit::Painter, b: Bounds<Pixels>, n: usize) -> Vec<Pixels> {
    let left = b.left() + px(GUTTER);
    let w = f32::from(b.right() - left);
    (0..=n).map(|i| style::snap_coord(g, left + px(w * i as f32 / n as f32))).collect()
}

/// A press on the days' names and whole-day strip (`hours` false) or
/// among the hours: an event selected, or opened (twice); a day gone to,
/// or (twice) a new event made on it, at the half hour under the
/// pointer among the hours, for the whole day above them.
fn select(v: &mut Calendar, e: &MouseDownEvent, hours: bool, cx: &mut Cx<Calendar>) {
    let now = cx.listener_bounds();
    let state = cx.node_state::<State>();
    let at = crate::view::in_drawing(e.position, state.origin, now);
    let over = state.parts.iter().rev().find(|(r, _)| r.contains(&at)).map(|(_, part)| part.clone());
    // An event pressed among the hours may be dragged to another time
    // or day (one in a calendar that can be written to).
    let pressed = match &over {
        Some(Part::Event(key)) if hours && e.click_count == 1 && v.changeable(key) => {
            state.parts.iter().rev().find(|(r, part)| r.contains(&at) && matches!(part, Part::Event(k) if k == key)).map(|(r, _)| *r)
        }
        _ => None,
    };
    if let (Some(r), Some(Part::Event(key))) = (pressed, &over)
        && let Some(s) = v.shown.iter().find(|s| s.key() == *key)
    {
        let minutes = |t: jiff::civil::DateTime| t.hour() as i32 * 60 + t.minute() as i32;
        let lasts = s.end.since(s.start).ok().and_then(|d| d.total(jiff::Unit::Minute).ok()).unwrap_or(60.0) as f32;
        // (By its bottom edge: its end is dragged, not the event.)
        let end = state.grips.iter().find(|(grip, k, _)| k == key && grip.contains(&at)).map(|(.., day)| *day);
        let grab = if end.is_some() { 0.0 } else { f32::from(at.y - r.top()) / HOUR * 60.0 };
        v.drag = Some(Drag { key: key.clone(), from: at, grab, lasts, was: (s.start.date(), minutes(s.start)), to: None, end });
    }
    // One pressed in the whole-day strip, to other days.
    if let (false, 1, Some(Part::Event(key))) = (hours, e.click_count, &over)
        && let Some(first) = state.first
        && state.cols.len() >= 2
        && let Some((r, _)) = state.parts.iter().rev().find(|(r, part)| r.contains(&at) && matches!(part, Part::Event(k) if k == key))
    {
        let col = state.cols.windows(2).position(|c| at.x < c[1]).unwrap_or(state.cols.len() - 2);
        let lane = (f32::from(r.top() - state.origin.y - px(NAMES)) / LANE).round().max(0.0) as usize;
        v.day_drag_start(key, at, layout::day_of(layout::day_number(first) + col as i32), lane);
    }
    match over {
        Some(Part::Event(key)) if e.click_count >= 2 => cx.send(Msg::Open(key)),
        Some(Part::Event(key)) => cx.send(Msg::Select(Some(key))),
        Some(Part::Day(day)) if e.click_count >= 2 => {
            if hours {
                let down = f32::from(e.position.y - now.top()).max(0.0);
                let half_hours = ((down / HOUR * 2.0).floor() as i8).clamp(0, 47);
                cx.send(Msg::New(day.at(half_hours / 2, (half_hours % 2) * 30, 0, 0), false));
            } else {
                cx.send(Msg::New(day.at(0, 0, 0, 0), true));
            }
        }
        Some(Part::Day(day)) => {
            cx.send(Msg::Select(None));
            v.go(day, cx);
        }
        None => {}
    }
}

pub fn view(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
    let (first, n) = days(v);
    // (Empty, the all-day row keeps a lane to drop on, except in a window
    // a third of a screen high, where the hours want every point.)
    let short = f32::from(cx.viewport_size().height) < SHORT;
    let strip = layout::row(&v.shown, first, n, usize::MAX, true).lanes.clamp(if short { 0 } else { 1 }, MOST);
    // (A whole number of device pixels at every scale, quarters too:
    // what scrolls under it starts on one.)
    let head = ((NAMES + strip as f32 * LANE + 3.0) / 4.0).ceil() * 4.0;
    let key = |v: &Calendar| style::key((v.cursor, v.settings.mode, v.today, v.week_start(), v.shown_version, &v.selected, v.h24));
    // The days' names, and what's on all day.
    let top = cx
        .canvas(move |v: &Calendar, g, b| {
            let p = theme::palette();
            let b = style::snap(g, b);
            let (text, quiet) = (style::c(p.document_text), style::secondary_text(&p));
            let line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.88));
            let (regular, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
            let (first, n) = days(v);
            // (As wide as the hours under it: not over their scroll bar.)
            let xs = columns(g, Bounds::from_corners(b.origin, point(b.right() - px(SCROLL_BAR), b.bottom())), n);
            let mut parts: Vec<(Bounds<Pixels>, Part)> = Vec::new();
            olo_kit::review::own_grid(g, b);
            let mid = b.top() + px(NAMES / 2.0);
            // The days' headings' form: the longest every column has room
            // for ("Wed 18", "W 18", "18"), with the widest of the
            // week's names and numbers, today's in its pill.
            let heading = {
                let narrowest = (0..n).map(|col| f32::from(xs[col + 1] - xs[col])).fold(f32::MAX, f32::min);
                let width = |t: &str, size: f32, font: &olo_kit::Font| f32::from(style::text_width(t.to_owned(), size, font.clone()));
                let numbers = (0..n)
                    .map(|col| layout::day_of(layout::day_number(first) + col as i32))
                    .map(|d| width(&d.day().to_string(), style::PLAIN_SIZE, &bold) + if d == v.today { 10.0 } else { 0.0 })
                    .fold(0.0, f32::max);
                let names = |len: usize| (0..n).map(|col| width(&words::weekday_name(layout::day_of(layout::day_number(first) + col as i32))[..len], style::DETAIL_SIZE, &regular)).fold(0.0, f32::max);
                let fits = |name: f32, gap: f32| EDGE + PAD + name + gap + numbers + PAD <= narrowest;
                // ("W 18" centred in its column, with a little room each
                // side: at its left inset it didn't fit a third of a
                // screen's columns, and the names went.)
                if fits(names(3), 6.0) {
                    Heading::Short
                } else if names(1) + 2.0 + numbers + 2.0 <= narrowest {
                    Heading::Initial
                } else {
                    Heading::Number
                }
            };
            for col in 0..n {
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let cell = Bounds::from_corners(point(xs[col], b.top()), point(xs[col + 1], b.bottom()));
                // (Today among the week's days; a day by itself needs no telling apart.)
                if day == v.today && n > 1 {
                    style::fill_rect(g, cell, style::c(p.list_selected_background).opacity(0.3));
                }
                // "Wed" and its number (today's on the look's mark colour);
                // in columns too narrow for that (a third of a screen),
                // "W" and its number, or the number alone: the same in
                // every column (`heading`).
                let full = words::weekday_name(day);
                let number = day.day().to_string();
                let (name, gap) = match heading {
                    _ if n == 1 => (full.to_owned(), 6.0),
                    Heading::Short => (full[..3].to_owned(), 6.0),
                    // (Today's pill starts a point before its number: a
                    // wider gap, or "W" ran into it.)
                    Heading::Initial => (full[..1].to_owned(), if day == v.today { 4.0 } else { 2.0 }),
                    Heading::Number => (String::new(), 0.0),
                };
                let x = match heading {
                    Heading::Initial if n > 1 => {
                        let name_w = f32::from(style::text_width(name.clone(), style::DETAIL_SIZE, regular.clone()));
                        let number_w = f32::from(style::text_width(number.clone(), style::PLAIN_SIZE, bold.clone())) + if day == v.today { 10.0 } else { 0.0 };
                        xs[col] + px(((f32::from(xs[col + 1] - xs[col]) - name_w - gap - number_w) / 2.0).round())
                    }
                    _ => xs[col] + px(EDGE + PAD),
                };
                // (The name on the line of its number, which is the larger.)
                let r = if name.is_empty() {
                    Bounds { origin: point(x, mid), size: size(px(0.0), px(0.0)) }
                } else {
                    style::label_on_baseline(g, x, style::baseline_at(g, mid, style::PLAIN_SIZE), style::Align::Left, name, style::DETAIL_SIZE, regular.clone(), quiet)
                };
                let nx = r.right() + px(gap);
                let shown = if day == v.today {
                    let width = (f32::from(style::text_width(number.clone(), style::PLAIN_SIZE, bold.clone())) + 10.0).max(18.0);
                    let pill = style::snap(g, Bounds { origin: point(nx - px(1.0), mid - px(9.0)), size: size(px(width), px(18.0)) });
                    g.paint_quad(olo_kit::fill(pill, style::c(p.control_mark)).corner_radii(px(9.0)));
                    style::label(g, pill.center().x, mid, style::Align::Center, number, style::PLAIN_SIZE, bold.clone(), style::c(p.ink_on(p.control_mark)))
                } else {
                    style::label(g, nx, mid, style::Align::Left, number, style::PLAIN_SIZE, bold.clone(), text)
                };
                g.agent(agent::Node {
                    role: "ColumnHeader",
                    name: Some(words::long_day(day).into()),
                    bounds: r.union(&shown),
                    enabled: true,
                    clickable: true,
                    selected: Some(day == v.cursor),
                    ..Default::default()
                });
                parts.push((cell, Part::Day(day)));
            }
            for x in &xs[..n] {
                style::vline(g, *x, b.top() + px(NAMES), b.bottom(), line);
            }
            // What's on all day, across its days.
            let laid = layout::row(&v.shown, first, n, strip, true);
            let top = b.top() + px(NAMES);
            if strip > 0 {
                style::label(g, b.left() + px(GUTTER - 8.0), top + px(LANE / 2.0), style::Align::Right, "all day", style::SMALL_SIZE, regular.clone(), quiet);
            }
            for piece in &laid.pieces {
                let s = &v.shown[piece.item];
                let y = top + px(piece.lane as f32 * LANE + 1.0);
                let left = xs[piece.from] + if piece.before { px(0.0) } else { px(EDGE) };
                let right = xs[piece.to + 1] - if piece.after { px(0.0) } else { px(EDGE) };
                let r = style::snap(g, Bounds::from_corners(point(left, y), point(right, y + px(BAR))));
                let chosen = v.selected.as_ref().is_some_and(|key| *key == s.key());
                let hue = p.hue(s.hue);
                // (Being dragged to other days: where it was, faint.)
                let moving = v.day_drag.as_ref().is_some_and(|d| d.to.is_some() && d.key == s.key());
                let (ground, lined) = crate::view::ground(&p, s, chosen);
                let fill = if moving { olo_theme::mix(hue, p.document_background, 0.7) } else { ground };
                style::fill_rect(g, r, style::c(fill));
                if chosen && !moving {
                    style::frame_rect(g, r, text);
                } else if lined && !moving {
                    style::frame_rect(g, r, style::c(hue));
                }
                let said = if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) };
                let t = style::label_fit(
                    g,
                    xs[piece.from] + px(EDGE + PAD),
                    r.center().y,
                    style::Align::Left,
                    said,
                    style::DETAIL_SIZE,
                    regular.clone(),
                    style::c(p.ink_on(fill)),
                    (r.size.width - px(2.0 * PAD)).max(px(0.0)),
                );
                crate::view::struck(g, s, t, style::c(p.ink_on(fill)));
                let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                let name = format!("{}, {}, {calendar}{}", s.title, words::when(s.start, s.end, s.all_day, v.h24), crate::view::going_said(s));
                g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                parts.push((r, Part::Event(s.key())));
            }
            for (col, more) in laid.more.iter().enumerate().filter(|(_, more)| **more > 0) {
                let y = top + px((strip - 1) as f32 * LANE + 1.0 + BAR / 2.0);
                style::label(g, xs[col] + px(EDGE + PAD), y, style::Align::Left, format!("+{more} more"), style::DETAIL_SIZE, regular.clone(), quiet);
            }
            // One being dragged to other days: where it would be, on those
            // of them that show.
            if let Some(drag) = &v.day_drag
                && let Some(to) = drag.to
                && let Some(s) = v.shown.iter().find(|s| s.key() == drag.key)
            {
                let from = layout::day_number(to) - layout::day_number(first);
                let (a, z) = (from.max(0), (from + drag.days - 1).min(n as i32 - 1));
                if a <= z {
                    let y = top + px(drag.lane.min(strip.saturating_sub(1)) as f32 * LANE + 1.0);
                    let left = xs[a as usize] + if a > from { px(0.0) } else { px(EDGE) };
                    let right = xs[z as usize + 1] - if z < from + drag.days - 1 { px(0.0) } else { px(EDGE) };
                    let r = style::snap(g, Bounds::from_corners(point(left, y), point(right, y + px(BAR))));
                    let fill = p.hue(s.hue);
                    style::fill_rect(g, r, style::c(fill));
                    style::frame_rect(g, r, text);
                    let said = if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) };
                    let x = xs[a as usize] + px(EDGE + PAD);
                    style::label_fit(g, x, r.center().y, style::Align::Left, said, style::DETAIL_SIZE, regular.clone(), style::c(p.ink_on(fill)), (r.size.width - px(2.0 * PAD)).max(px(0.0)));
                }
            }
            // A line under it all, heavier: the hours start here.
            style::hline_above(g, b.left(), b.right(), b.bottom(), style::c(olo_theme::mix(p.document_text, p.document_background, 0.7)));
            let state = g.node_state::<State>();
            (state.parts, state.origin) = (parts, b.origin);
            (state.cols, state.first) = (xs, Some(first));
        })
        .key_by(move |v: &Calendar| style::key((key(v), v.day_drag.as_ref().and_then(|d| d.to))))
        .w_full()
        .h(px(head))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| select(v, e, false, cx)))
        // (One of its events pressed is dragged to other days.)
        .on_drag_move(cx.listener(day_dragged))
        .on_window_mouse_up(cx.listener(|v: &mut Calendar, _: &MouseUpEvent, cx| v.day_drag_drop(cx)));
    // The hours.
    let hours = cx
        .canvas(|v: &Calendar, g, b| {
            let p = theme::palette();
            let b = style::snap(g, b);
            let (text, quiet) = (style::c(p.document_text), style::secondary_text(&p));
            let line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.88));
            let faint = style::c(olo_theme::mix(p.document_text, p.document_background, 0.95));
            let (regular, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
            let (first, n) = days(v);
            let xs = columns(g, b, n);
            let mut parts: Vec<(Bounds<Pixels>, Part)> = Vec::new();
            let mut grips: Vec<(Bounds<Pixels>, Key, Date)> = Vec::new();
            olo_kit::review::own_grid(g, b);
            let y_at = |minutes: f32| b.top() + px(minutes / 60.0 * HOUR);
            for col in 0..n {
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let cell = Bounds::from_corners(point(xs[col], b.top()), point(xs[col + 1], b.bottom()));
                if day == v.today && n > 1 {
                    style::fill_rect(g, cell, style::c(p.list_selected_background).opacity(0.3));
                }
                parts.push((cell, Part::Day(day)));
            }
            // The hours' lines and names; the half hours fainter.
            for h in 0..24 {
                let y = y_at(h as f32 * 60.0);
                if h > 0 {
                    style::hline(g, xs[0], b.right(), y, line);
                    let r = style::label(g, b.left() + px(GUTTER - 8.0), y, style::Align::Right, words::hour(h, v.h24), style::SMALL_SIZE, regular.clone(), quiet);
                    g.agent(agent::Node::label(words::hour(h, v.h24), r));
                }
                style::hline(g, xs[0], b.right(), y_at(h as f32 * 60.0 + 30.0), faint);
            }
            for x in &xs[..n] {
                style::vline(g, *x, b.top(), b.bottom(), line);
            }
            // Each day's events, side by side where they overlap.
            for col in 0..n {
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let (left, right) = (xs[col] + px(1.0), xs[col + 1] - px(7.0));
                let width = f32::from(right - left);
                for slot in layout::day(&v.shown, day) {
                    let s = &v.shown[slot.item];
                    let r = style::snap(g, Bounds::from_corners(point(left + px(width * slot.left), y_at(slot.from) + px(1.0)), point(left + px(width * slot.right) - px(1.0), y_at(slot.to))));
                    let chosen = v.selected.as_ref().is_some_and(|key| *key == s.key());
                    let hue = p.hue(s.hue);
                    // (Being dragged elsewhere: where it was, faint.)
                    let moving = v.drag.as_ref().is_some_and(|d| d.to.is_some() && d.key == s.key());
                    let (ground, lined) = crate::view::ground(&p, s, chosen);
                    let fill = if moving { olo_theme::mix(hue, p.document_background, 0.7) } else { ground };
                    style::fill_rect(g, r, style::c(fill));
                    if chosen && !moving {
                        style::frame_rect(g, r, text);
                        style::frame_rect(g, style::inset(g, r, 1.0), text);
                    } else if lined && !moving {
                        style::frame_rect(g, r, style::c(hue));
                    }
                    let ink = style::c(p.ink_on(fill));
                    let (x, room) = (r.left() + px(PAD), (r.size.width - px(2.0 * PAD)).max(px(0.0)));
                    let tall = f32::from(r.size.height);
                    // From when to when, or only from when where that's all there's room for.
                    let whole = words::when(s.start, s.end, false, v.h24);
                    let when = if style::text_width(whole.clone(), style::SMALL_SIZE, regular.clone()) <= room { whole } else { words::time(s.start.time(), v.h24) };
                    // (Its words are set by its own time, not level with its neighbours'.)
                    olo_kit::review::own_grid(g, r);
                    g.with_content_mask(Some(olo_kit::ContentMask { bounds: r }), |g| {
                        if moving && v.drag.as_ref().is_some_and(|d| d.end.is_some()) {
                            // (Its end being dragged: how long it was, faint,
                            // without its words; they're on what it would be.)
                        } else if tall < 30.0 {
                            // One line, in its middle: its title, then its start.
                            let mid = r.center().y;
                            let t = style::label_fit(g, x, mid, style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, bold.clone(), ink, room);
                            crate::view::struck(g, s, t, ink);
                            // (Its start after it only whole: cut, "9:3…"
                            // said nothing the hours beside it don't.)
                            let after = t.right() + px(6.0);
                            let start = words::time(s.start.time(), v.h24);
                            if style::text_width(start.clone(), style::DETAIL_SIZE, regular.clone()) <= r.right() - after - px(PAD) {
                                style::label(g, after, mid, style::Align::Left, start, style::DETAIL_SIZE, regular.clone(), ink);
                            }
                        } else {
                            let t = style::label_fit(g, x, r.top() + px(9.0), style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, bold.clone(), ink, room);
                            crate::view::struck(g, s, t, ink);
                            // (Its time only whole: "7…" under "R…" said
                            // nothing; Codex, 2026-11-19.)
                            if style::text_width(when.clone(), style::SMALL_SIZE, regular.clone()) <= room {
                                style::label(g, x, r.top() + px(9.0 + 14.0), style::Align::Left, when.clone(), style::SMALL_SIZE, regular.clone(), ink);
                            }
                            if tall >= 46.0 && !s.location.is_empty() {
                                style::label_fit(g, x, r.top() + px(9.0 + 28.0), style::Align::Left, s.location.to_string(), style::SMALL_SIZE, regular.clone(), ink, room);
                            }
                        }
                    });
                    let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                    let name = format!("{}, {}, {calendar}{}", s.title, words::when(s.start, s.end, s.all_day, v.h24), crate::view::going_said(s));
                    g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                    parts.push((r, Part::Event(s.key())));
                    // Its bottom edge drags its end, where it ends on this
                    // day (and its calendar can be written to).
                    let ends_here = s.end.date() == day || s.end == layout::day_of(layout::day_number(day) + 1).at(0, 0, 0, 0);
                    if ends_here && v.changeable(&s.key()) {
                        let grip = Bounds::from_corners(point(r.left(), r.bottom() - px(GRIP.min(tall / 3.0))), r.bottom_right());
                        g.set_cursor_style(olo_kit::CursorStyle::ResizeUpDown, grip);
                        grips.push((grip, s.key(), day));
                    }
                }
            }
            // An event being dragged: where it would go, the day's whole
            // width, with the times it would have.
            if let Some(drag) = &v.drag
                && let Some((day, start)) = drag.to
                && let Some(s) = v.shown.iter().find(|s| s.key() == drag.key)
            {
                let col = layout::day_number(day) - layout::day_number(first);
                if (0..n as i32).contains(&col) {
                    let col = col as usize;
                    let (left, right) = (xs[col] + px(1.0), xs[col + 1] - px(7.0));
                    // (Its end dragged: from where it starts on that day
                    // to where it would end; the cursor its edge's, held.)
                    let (top, bottom) = match drag.end {
                        Some(_) => (if day == drag.was.0 { drag.was.1 as f32 } else { 0.0 }, start as f32),
                        None => (start as f32, start as f32 + drag.lasts.max(STEP as f32)),
                    };
                    if drag.end.is_some() {
                        g.set_cursor_style(olo_kit::CursorStyle::ResizeUpDown, b.dilate(px(4096.0)));
                    }
                    let r = style::snap(g, Bounds::from_corners(point(left, y_at(top) + px(1.0)), point(right - px(1.0), y_at(bottom))));
                    let fill = p.hue(s.hue);
                    style::fill_rect(g, r, style::c(fill));
                    style::frame_rect(g, r, text);
                    style::frame_rect(g, style::inset(g, r, 1.0), text);
                    let ink = style::c(p.ink_on(fill));
                    let (from, to) = match drag.end {
                        Some(_) => (s.start, time_on(day, start)),
                        None => {
                            let from = day.at((start / 60) as i8, (start % 60) as i8, 0, 0);
                            (from, from.checked_add(jiff::Span::new().minutes(drag.lasts as i64)).unwrap_or(from))
                        }
                    };
                    let (x, room) = (r.left() + px(PAD), (r.size.width - px(2.0 * PAD)).max(px(0.0)));
                    olo_kit::review::own_grid(g, r);
                    g.with_content_mask(Some(olo_kit::ContentMask { bounds: r }), |g| {
                        // Its time only where it fits whole: cut ("7…"), it
                        // said nothing the hours beside it don't.
                        let whole = |t: &str, size: f32, font: &olo_kit::Font| style::text_width(t.to_owned(), size, font.clone()) <= room;
                        if f32::from(r.size.height) < 30.0 {
                            let both = format!("{}  {}", s.title, words::time(from.time(), v.h24));
                            let said = if whole(&both, style::DETAIL_SIZE, &bold) { both } else { s.title.to_string() };
                            style::label_fit(g, x, r.center().y, style::Align::Left, said, style::DETAIL_SIZE, bold.clone(), ink, room);
                        } else {
                            style::label_fit(g, x, r.top() + px(9.0), style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, bold.clone(), ink, room);
                            let when = words::when(from, to, false, v.h24);
                            if whole(&when, style::SMALL_SIZE, &regular) {
                                style::label_fit(g, x, r.top() + px(9.0 + 14.0), style::Align::Left, when, style::SMALL_SIZE, regular.clone(), ink, room);
                            }
                        }
                    });
                }
            }
            // Now: a line across today, drawn again as the minute turns.
            let now = crate::now(&v.zone);
            let today = layout::day_number(now.date()) - layout::day_number(first);
            if (0..n as i32).contains(&today) {
                let minutes = now.hour() as f32 * 60.0 + now.minute() as f32;
                let y = style::snap_coord(g, y_at(minutes));
                let mark = style::c(p.failure);
                let (x0, x1) = (xs[today as usize], xs[today as usize + 1]);
                style::fill_rect(g, Bounds::from_corners(point(x0, y - px(1.0)), point(x1, y + px(1.0))), mark);
                let dot = style::snap_centred(g, point(x0, y), 7.0, 7.0);
                g.paint_quad(olo_kit::fill(dot, mark).corner_radii(px(3.5)));
                g.paint_again_within((60 - now.second() as i64).max(1) as f64);
            }
            let state = g.node_state::<State>();
            (state.parts, state.origin) = (parts, b.origin);
            (state.cols, state.first, state.grips) = (xs, Some(first), grips);
        })
        .key_by(move |v: &Calendar| style::key((key(v), v.drag.as_ref().and_then(|d| d.to))))
        .w_full()
        .h(px(24.0 * HOUR))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| select(v, e, true, cx)))
        // An event pressed is dragged to another time or day (followed
        // out of the hours too), and goes there when the button is let go.
        .on_drag_move(cx.listener(dragged))
        .on_window_mouse_up(cx.listener(dropped));
    let p = theme::palette();
    div().size_full().flex().flex_col().child(top).child(div().flex_1().min_h_0().child(ScrollView::new(&v.hours_scroll).border(false).background(p.document_background).child(hours))).into_element()
}

/// Where the hours start out scrolled to: a little before the working
/// day (or before now, if that's earlier).
/// The days' headings' form, the same in every column.
#[derive(Clone, Copy)]
enum Heading {
    /// "Wed 18".
    Short,
    /// "W 18".
    Initial,
    /// "18".
    Number,
}

/// A window shorter than this (a third of a screen's height): the empty
/// all-day row gives its lane to the hours.
const SHORT: f32 = 300.0;

pub fn starting_offset(now_hour: i8) -> f32 {
    // (Its first hour's name whole under the days' names.)
    ((now_hour.min(8) as f32 - 1.0) * HOUR - 12.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dragged event starts on the quarter hour nearest its top, never
    /// before the day begins nor so late that it starts the next.
    #[test]
    fn dragged_by_quarter_hours() {
        for (minutes, start) in [(600.0, 600), (607.0, 600), (608.0, 615), (622.4, 615), (-40.0, 0), (0.0, 0), (1439.0, 1425), (5000.0, 1425)] {
            assert_eq!(quarter(minutes), start, "{minutes}");
        }
    }

    /// A dragged end is on the quarter hour nearest the pointer: never
    /// less than a quarter hour after the start, midnight at the latest
    /// (the next day's first minute).
    #[test]
    fn an_end_dragged_by_quarter_hours() {
        for (minutes, start, end) in [(660.0, 600, 660), (652.0, 600, 645), (653.0, 600, 660), (600.0, 600, 615), (200.0, 600, 615), (1436.0, 600, 1440), (9000.0, 600, 1440), (5.0, 0, 15)] {
            assert_eq!(quarter_end(minutes, start), end, "{minutes} from {start}");
        }
        let day = jiff::civil::date(2026, 11, 18);
        assert_eq!(time_on(day, 645), day.at(10, 45, 0, 0));
        assert_eq!(time_on(day, 1440), jiff::civil::date(2026, 11, 19).at(0, 0, 0, 0));
    }
}
