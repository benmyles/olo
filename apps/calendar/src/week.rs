//! A week's days side by side, or one day: the days' names over what's
//! on for a whole day (bars across their days), and under that the hours,
//! each event where its time puts it and as tall as it lasts, side by
//! side where they overlap, the present a line across today.

use jiff::civil::Date;
use olo_kit::{
    Bounds, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Styled as _, agent, div, point, px, size, style, theme,
    widgets::{ScrollView, scroll::SCROLL_BAR},
};

use crate::{Calendar, Key, Mode, Msg, layout, words};

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
    let strip = layout::row(&v.shown, first, n, usize::MAX, true).lanes.clamp(1, MOST);
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
            for col in 0..n {
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let cell = Bounds::from_corners(point(xs[col], b.top()), point(xs[col + 1], b.bottom()));
                // (Today among the week's days; a day by itself needs no telling apart.)
                if day == v.today && n > 1 {
                    style::fill_rect(g, cell, style::c(p.list_selected_background).opacity(0.3));
                }
                // "Wed" and its number (today's on the look's mark colour).
                let name = if n == 1 { words::weekday_name(day).to_owned() } else { words::weekday_name(day)[..3].to_owned() };
                let x = xs[col] + px(EDGE + PAD);
                // (The name on the line of its number, which is the larger.)
                let r = style::label_on_baseline(g, x, style::baseline_at(g, mid, style::PLAIN_SIZE), style::Align::Left, name, style::DETAIL_SIZE, regular.clone(), quiet);
                let number = day.day().to_string();
                let nx = r.right() + px(6.0);
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
            style::label(g, b.left() + px(GUTTER - 8.0), top + px(LANE / 2.0), style::Align::Right, "all day", style::SMALL_SIZE, regular.clone(), quiet);
            for piece in &laid.pieces {
                let s = &v.shown[piece.item];
                let y = top + px(piece.lane as f32 * LANE + 1.0);
                let left = xs[piece.from] + if piece.before { px(0.0) } else { px(EDGE) };
                let right = xs[piece.to + 1] - if piece.after { px(0.0) } else { px(EDGE) };
                let r = style::snap(g, Bounds::from_corners(point(left, y), point(right, y + px(BAR))));
                let chosen = v.selected.as_ref().is_some_and(|key| *key == s.key());
                let hue = p.hue(s.hue);
                let fill = if s.free && !chosen { olo_theme::mix(hue, p.document_background, 0.45) } else { hue };
                style::fill_rect(g, r, style::c(fill));
                if chosen {
                    style::frame_rect(g, r, text);
                }
                let said = if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) };
                style::label_fit(
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
                let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                let name = format!("{}, {}, {calendar}", s.title, words::when(s.start, s.end, s.all_day, v.h24));
                g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                parts.push((r, Part::Event(s.key())));
            }
            for (col, more) in laid.more.iter().enumerate().filter(|(_, more)| **more > 0) {
                let y = top + px((strip - 1) as f32 * LANE + 1.0 + BAR / 2.0);
                style::label(g, xs[col] + px(EDGE + PAD), y, style::Align::Left, format!("+{more} more"), style::DETAIL_SIZE, regular.clone(), quiet);
            }
            // A line under it all, heavier: the hours start here.
            style::hline_above(g, b.left(), b.right(), b.bottom(), style::c(olo_theme::mix(p.document_text, p.document_background, 0.7)));
            let state = g.node_state::<State>();
            (state.parts, state.origin) = (parts, b.origin);
        })
        .key_by(key)
        .w_full()
        .h(px(head))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| select(v, e, false, cx)));
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
                    let fill = if (s.free || s.tentative) && !chosen { olo_theme::mix(hue, p.document_background, 0.45) } else { hue };
                    style::fill_rect(g, r, style::c(fill));
                    if chosen {
                        style::frame_rect(g, r, text);
                        style::frame_rect(g, style::inset(g, r, 1.0), text);
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
                        if tall < 30.0 {
                            // One line, in its middle: its title, then its start.
                            let mid = r.center().y;
                            let t = style::label_fit(g, x, mid, style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, bold.clone(), ink, room);
                            let after = t.right() + px(6.0);
                            if r.right() - after > px(40.0) {
                                style::label_fit(g, after, mid, style::Align::Left, words::time(s.start.time(), v.h24), style::DETAIL_SIZE, regular.clone(), ink, r.right() - after - px(PAD));
                            }
                        } else {
                            style::label_fit(g, x, r.top() + px(9.0), style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, bold.clone(), ink, room);
                            style::label_fit(g, x, r.top() + px(9.0 + 14.0), style::Align::Left, when.clone(), style::SMALL_SIZE, regular.clone(), ink, room);
                            if tall >= 46.0 && !s.location.is_empty() {
                                style::label_fit(g, x, r.top() + px(9.0 + 28.0), style::Align::Left, s.location.to_string(), style::SMALL_SIZE, regular.clone(), ink, room);
                            }
                        }
                    });
                    let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                    let name = format!("{}, {}, {calendar}", s.title, words::when(s.start, s.end, s.all_day, v.h24));
                    g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                    parts.push((r, Part::Event(s.key())));
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
        })
        .key_by(key)
        .w_full()
        .h(px(24.0 * HOUR))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| select(v, e, true, cx)));
    let p = theme::palette();
    div().size_full().flex().flex_col().child(top).child(div().flex_1().min_h_0().child(ScrollView::new(&v.hours_scroll).border(false).background(p.document_background).child(hours))).into_element()
}

/// Where the hours start out scrolled to: a little before the working
/// day (or before now, if that's earlier).
pub fn starting_offset(now_hour: i8) -> f32 {
    // (Its first hour's name whole under the days' names.)
    ((now_hour.min(8) as f32 - 1.0) * HOUR - 12.0).max(0.0)
}
